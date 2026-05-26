use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use async_trait::async_trait;
use aws_sdk_s3::Client;
use aws_sdk_s3::types::{Delete, ObjectIdentifier};
use serde::Serialize;
use tokio::sync::RwLock;

use crate::config::GcConfig;
use crate::error::OrbChrysaError;
use crate::raft::RaftInstance;
use crate::raft::state_machine::StateMachineData;

#[derive(Debug, Clone)]
pub struct GcBlobObject {
    key: String,
    digest: String,
    last_modified: Option<SystemTime>,
}

impl GcBlobObject {
    pub fn new(key: String, digest: String, last_modified: Option<SystemTime>) -> Self {
        Self {
            key,
            digest,
            last_modified,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct GcDeleteOutcome {
    deleted: usize,
    errors: usize,
}

#[async_trait]
pub trait GcBlobStore: Send + Sync + 'static {
    async fn list_blob_objects(&self) -> Result<Vec<GcBlobObject>, OrbChrysaError>;
    async fn delete_blob_batch(&self, keys: &[String]) -> Result<GcDeleteOutcome, OrbChrysaError>;
}

pub struct S3GcBlobStore {
    client: Client,
    bucket: String,
}

impl S3GcBlobStore {
    pub fn new(client: Client, bucket: String) -> Self {
        Self { client, bucket }
    }
}

#[async_trait]
impl GcBlobStore for S3GcBlobStore {
    async fn list_blob_objects(&self) -> Result<Vec<GcBlobObject>, OrbChrysaError> {
        let mut objects = Vec::new();
        let mut continuation_token: Option<String> = None;

        loop {
            let mut req = self
                .client
                .list_objects_v2()
                .bucket(&self.bucket)
                .prefix("blobs/");
            if let Some(ref token) = continuation_token {
                req = req.continuation_token(token);
            }
            let resp = req
                .send()
                .await
                .map_err(|e| OrbChrysaError::S3(e.to_string()))?;

            for obj in resp.contents() {
                let Some(key) = obj.key() else { continue };
                let Some(digest) = s3_key_to_digest(key) else {
                    continue;
                };
                let last_modified = obj
                    .last_modified()
                    .and_then(|lm| u64::try_from(lm.secs()).ok())
                    .map(|secs| SystemTime::UNIX_EPOCH + Duration::from_secs(secs));
                objects.push(GcBlobObject::new(key.to_string(), digest, last_modified));
            }

            if resp.is_truncated() == Some(true) {
                continuation_token = resp.next_continuation_token().map(|s| s.to_string());
            } else {
                break;
            }
        }

        Ok(objects)
    }

    async fn delete_blob_batch(&self, keys: &[String]) -> Result<GcDeleteOutcome, OrbChrysaError> {
        let objects: Vec<ObjectIdentifier> = keys
            .iter()
            .filter_map(|k| ObjectIdentifier::builder().key(k).build().ok())
            .collect();

        let delete = Delete::builder()
            .set_objects(Some(objects))
            .build()
            .map_err(|e| OrbChrysaError::Internal(e.to_string()))?;

        let resp = self
            .client
            .delete_objects()
            .bucket(&self.bucket)
            .delete(delete)
            .send()
            .await
            .map_err(|e| OrbChrysaError::S3(e.to_string()))?;

        let errors = resp.errors().len();
        if errors > 0 {
            let first = &resp.errors()[0];
            tracing::warn!(
                count = errors,
                first_key = first.key().unwrap_or("?"),
                first_msg = first.message().unwrap_or("?"),
                "GC: some blob deletions failed"
            );
        }

        Ok(GcDeleteOutcome {
            deleted: keys.len().saturating_sub(errors),
            errors,
        })
    }
}

pub async fn run_gc_loop<B: GcBlobStore>(
    raft: Arc<RaftInstance>,
    node_id: u64,
    state: Arc<RwLock<StateMachineData>>,
    blob_store: B,
    status: Arc<RwLock<GcStatus>>,
    config: GcConfig,
) {
    loop {
        tokio::time::sleep(Duration::from_secs(config.interval_secs)).await;

        let still_leader = {
            let raft = raft.clone();
            move || raft.metrics().borrow().current_leader == Some(node_id)
        };
        if !still_leader() {
            continue;
        }

        match run_gc_sweep(&state, &blob_store, &config, still_leader).await {
            Ok(stats) => {
                *status.write().await = stats.clone();
                tracing::info!(
                    last_run_at = stats.last_run_at,
                    duration_ms = stats.duration_ms,
                    scanned = stats.scanned,
                    candidates = stats.candidates,
                    deleted = stats.deleted,
                    skipped_referenced = stats.skipped_referenced,
                    skipped_young = stats.skipped_young,
                    delete_errors = stats.delete_errors,
                    dry_run = stats.dry_run,
                    leadership_lost = stats.leadership_lost,
                    "GC sweep completed"
                );
            }
            Err(e) => {
                status.write().await.last_error = Some(e.to_string());
                tracing::warn!(err = %e, "GC sweep failed");
            }
        }
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct GcStatus {
    pub last_run_at: u64,
    pub duration_ms: u64,
    pub scanned: usize,
    pub candidates: usize,
    pub deleted: usize,
    pub skipped_referenced: usize,
    pub skipped_young: usize,
    pub delete_errors: usize,
    pub dry_run: bool,
    pub leadership_lost: bool,
    pub last_error: Option<String>,
}

async fn run_gc_sweep<B, F>(
    state: &RwLock<StateMachineData>,
    blob_store: &B,
    config: &GcConfig,
    mut still_leader: F,
) -> Result<GcStatus, OrbChrysaError>
where
    B: GcBlobStore,
    F: FnMut() -> bool,
{
    let started = Instant::now();
    let mut stats = GcStatus {
        last_run_at: crate::store::metadata::now_epoch(),
        dry_run: config.dry_run,
        ..GcStatus::default()
    };
    let now = SystemTime::now();
    let grace_period = Duration::from_secs(config.grace_period_secs);
    let objects = blob_store.list_blob_objects().await?;
    stats.scanned = objects.len();

    let ref_counts = {
        let data = state.read().await;
        data.blob_ref_counts.clone()
    };

    let mut to_delete = Vec::new();
    for object in objects {
        if ref_counts.get(&object.digest).copied().unwrap_or(0) > 0 {
            stats.skipped_referenced += 1;
            continue;
        }

        let old_enough = object
            .last_modified
            .and_then(|modified| now.duration_since(modified).ok())
            .map(|age| age >= grace_period)
            .unwrap_or(false);
        if !old_enough {
            stats.skipped_young += 1;
            continue;
        }

        to_delete.push(object);
    }

    let fresh_ref_counts = {
        let data = state.read().await;
        data.blob_ref_counts.clone()
    };
    let before_final_check = to_delete.len();
    to_delete.retain(|object| fresh_ref_counts.get(&object.digest).copied().unwrap_or(0) == 0);
    stats.skipped_referenced += before_final_check.saturating_sub(to_delete.len());
    stats.candidates = to_delete.len();

    if config.dry_run {
        tracing::info!(count = to_delete.len(), "GC dry run: would delete blobs");
        stats.duration_ms = elapsed_ms(started);
        return Ok(stats);
    }

    for chunk in to_delete.chunks(1000) {
        if !still_leader() {
            stats.leadership_lost = true;
            break;
        }
        let keys: Vec<String> = chunk.iter().map(|object| object.key.clone()).collect();
        let outcome = blob_store.delete_blob_batch(&keys).await?;
        stats.deleted += outcome.deleted;
        stats.delete_errors += outcome.errors;
    }

    stats.duration_ms = elapsed_ms(started);
    Ok(stats)
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn s3_key_to_digest(key: &str) -> Option<String> {
    // "blobs/sha256/ab/abcdef0123..." -> "sha256:abcdef0123..."
    let parts: Vec<&str> = key.splitn(4, '/').collect();
    if parts.len() == 4 && parts[0] == "blobs" && parts[3].starts_with(parts[2]) {
        Some(format!("{}:{}", parts[1], parts[3]))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::{Duration, SystemTime};

    use async_trait::async_trait;
    use tokio::sync::{Mutex, RwLock};

    use super::*;

    #[derive(Clone, Default)]
    struct FakeGcBlobStore {
        objects: Arc<Mutex<Vec<GcBlobObject>>>,
        deleted: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl GcBlobStore for FakeGcBlobStore {
        async fn list_blob_objects(&self) -> Result<Vec<GcBlobObject>, OrbChrysaError> {
            Ok(self.objects.lock().await.clone())
        }

        async fn delete_blob_batch(
            &self,
            keys: &[String],
        ) -> Result<GcDeleteOutcome, OrbChrysaError> {
            self.deleted.lock().await.extend(keys.iter().cloned());
            Ok(GcDeleteOutcome {
                deleted: keys.len(),
                errors: 0,
            })
        }
    }

    fn old_object(digest: &str) -> GcBlobObject {
        let hex = digest.strip_prefix("sha256:").unwrap_or(digest);
        GcBlobObject::new(
            format!("blobs/sha256/{}/{}", &hex[..2], hex),
            digest.to_string(),
            Some(SystemTime::now() - Duration::from_secs(7200)),
        )
    }

    fn young_object(digest: &str) -> GcBlobObject {
        let hex = digest.strip_prefix("sha256:").unwrap_or(digest);
        GcBlobObject::new(
            format!("blobs/sha256/{}/{}", &hex[..2], hex),
            digest.to_string(),
            Some(SystemTime::now()),
        )
    }

    fn digest(byte: char) -> String {
        format!("sha256:{}", byte.to_string().repeat(64))
    }

    #[tokio::test]
    async fn gc_keeps_referenced_and_young_blobs() {
        let referenced = digest('a');
        let young = digest('b');
        let state = RwLock::new(StateMachineData {
            blob_ref_counts: [(referenced.clone(), 1)].into_iter().collect(),
            ..StateMachineData::default()
        });
        let store = FakeGcBlobStore::default();
        *store.objects.lock().await = vec![old_object(&referenced), young_object(&young)];

        let stats = run_gc_sweep(&state, &store, &GcConfig::default(), || true)
            .await
            .expect("gc succeeds");

        assert_eq!(stats.deleted, 0);
        assert_eq!(stats.skipped_referenced, 1);
        assert_eq!(stats.skipped_young, 1);
        assert!(store.deleted.lock().await.is_empty());
    }

    #[tokio::test]
    async fn gc_deletes_old_unreferenced_blobs() {
        let unreferenced = digest('c');
        let state = RwLock::new(StateMachineData::default());
        let store = FakeGcBlobStore::default();
        *store.objects.lock().await = vec![old_object(&unreferenced)];

        let stats = run_gc_sweep(&state, &store, &GcConfig::default(), || true)
            .await
            .expect("gc succeeds");

        assert_eq!(stats.candidates, 1);
        assert_eq!(stats.deleted, 1);
        assert_eq!(store.deleted.lock().await.len(), 1);
    }

    #[tokio::test]
    async fn dry_run_deletes_nothing() {
        let state = RwLock::new(StateMachineData::default());
        let store = FakeGcBlobStore::default();
        *store.objects.lock().await = vec![old_object(&digest('d'))];
        let config = GcConfig {
            dry_run: true,
            ..GcConfig::default()
        };

        let stats = run_gc_sweep(&state, &store, &config, || true)
            .await
            .expect("gc succeeds");

        assert_eq!(stats.candidates, 1);
        assert_eq!(stats.deleted, 0);
        assert!(store.deleted.lock().await.is_empty());
    }

    #[tokio::test]
    async fn leadership_loss_aborts_before_delete_batch() {
        let state = RwLock::new(StateMachineData::default());
        let store = FakeGcBlobStore::default();
        *store.objects.lock().await = vec![old_object(&digest('e'))];

        let stats = run_gc_sweep(&state, &store, &GcConfig::default(), || false)
            .await
            .expect("gc succeeds");

        assert!(stats.leadership_lost);
        assert_eq!(stats.deleted, 0);
        assert!(store.deleted.lock().await.is_empty());
    }
}

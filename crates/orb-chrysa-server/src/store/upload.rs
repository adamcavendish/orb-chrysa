use aws_sdk_s3::Client;
use aws_sdk_s3::primitives::ByteStream;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::error::OrbChrysaError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadSession {
    repo: String,
    pub offset: u64,
}

impl UploadSession {
    pub fn belongs_to(&self, repo: &str) -> bool {
        self.repo == repo
    }
}

#[derive(Debug, Clone)]
pub struct UploadTracker {
    backend: UploadTrackerBackend,
}

#[derive(Debug, Clone)]
enum UploadTrackerBackend {
    Memory(Arc<RwLock<HashMap<String, UploadSession>>>),
    S3(S3UploadTracker),
}

#[derive(Debug, Clone)]
struct S3UploadTracker {
    client: Client,
    bucket: String,
}

impl Default for UploadTracker {
    fn default() -> Self {
        Self {
            backend: UploadTrackerBackend::Memory(Arc::new(RwLock::new(HashMap::new()))),
        }
    }
}

impl UploadTracker {
    pub fn s3(client: Client, bucket: String) -> Self {
        Self {
            backend: UploadTrackerBackend::S3(S3UploadTracker { client, bucket }),
        }
    }

    pub async fn create(
        &self,
        session_id: String,
        repo: String,
    ) -> Result<UploadSession, OrbChrysaError> {
        let session = UploadSession { repo, offset: 0 };
        match &self.backend {
            UploadTrackerBackend::Memory(sessions) => {
                sessions.write().await.insert(session_id, session.clone());
            }
            UploadTrackerBackend::S3(tracker) => {
                tracker.write(&session_id, &session).await?;
            }
        }
        Ok(session)
    }

    pub async fn get(&self, session_id: &str) -> Result<Option<UploadSession>, OrbChrysaError> {
        match &self.backend {
            UploadTrackerBackend::Memory(sessions) => {
                Ok(sessions.read().await.get(session_id).cloned())
            }
            UploadTrackerBackend::S3(tracker) => tracker.read(session_id).await,
        }
    }

    pub async fn update_offset(&self, session_id: &str, offset: u64) -> Result<(), OrbChrysaError> {
        match &self.backend {
            UploadTrackerBackend::Memory(sessions) => {
                if let Some(session) = sessions.write().await.get_mut(session_id) {
                    session.offset = offset;
                }
                Ok(())
            }
            UploadTrackerBackend::S3(tracker) => {
                if let Some(mut session) = tracker.read(session_id).await? {
                    session.offset = offset;
                    tracker.write(session_id, &session).await?;
                }
                Ok(())
            }
        }
    }

    pub async fn remove(&self, session_id: &str) -> Result<(), OrbChrysaError> {
        match &self.backend {
            UploadTrackerBackend::Memory(sessions) => {
                sessions.write().await.remove(session_id);
                Ok(())
            }
            UploadTrackerBackend::S3(tracker) => tracker.remove(session_id).await,
        }
    }
}

impl S3UploadTracker {
    fn key(session_id: &str) -> String {
        format!("uploads/{}/session.json", session_id)
    }

    async fn write(&self, session_id: &str, session: &UploadSession) -> Result<(), OrbChrysaError> {
        let bytes = serde_json::to_vec(session)
            .map_err(|e| OrbChrysaError::S3(format!("encode upload session: {e}")))?;
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(Self::key(session_id))
            .body(ByteStream::from(bytes))
            .send()
            .await
            .map_err(|e| OrbChrysaError::S3(e.to_string()))?;
        Ok(())
    }

    async fn read(&self, session_id: &str) -> Result<Option<UploadSession>, OrbChrysaError> {
        let result = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(Self::key(session_id))
            .send()
            .await;
        let output = match result {
            Ok(output) => output,
            Err(sdk_err) => {
                let service_err = sdk_err.into_service_error();
                if service_err.is_no_such_key() {
                    return Ok(None);
                }
                return Err(OrbChrysaError::S3(service_err.to_string()));
            }
        };
        let bytes = output
            .body
            .collect()
            .await
            .map_err(|e| OrbChrysaError::S3(e.to_string()))?
            .to_vec();
        let session = serde_json::from_slice(&bytes)
            .map_err(|e| OrbChrysaError::S3(format!("decode upload session: {e}")))?;
        Ok(Some(session))
    }

    async fn remove(&self, session_id: &str) -> Result<(), OrbChrysaError> {
        self.client
            .delete_object()
            .bucket(&self.bucket)
            .key(Self::key(session_id))
            .send()
            .await
            .map_err(|e| OrbChrysaError::S3(e.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn upload_session_tracks_repo_and_offset() {
        let tracker = UploadTracker::default();
        let session = tracker
            .create("session-1".to_string(), "repo/name".to_string())
            .await
            .unwrap();

        assert!(session.belongs_to("repo/name"));
        assert!(!session.belongs_to("other/repo"));
        assert_eq!(session.offset, 0);

        tracker.update_offset("session-1", 42).await.unwrap();
        let session = tracker
            .get("session-1")
            .await
            .unwrap()
            .expect("session exists");
        assert_eq!(session.offset, 42);
    }
}

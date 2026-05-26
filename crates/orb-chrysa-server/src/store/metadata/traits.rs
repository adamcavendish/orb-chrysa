use async_trait::async_trait;

use super::types::*;
use crate::error::OrbChrysaError;
use crate::oci::digest::Digest;

#[async_trait]
pub trait ManifestStore: Send + Sync + 'static {
    async fn get_manifest(
        &self,
        name: &str,
        reference: &str,
    ) -> Result<Option<ManifestEntry>, OrbChrysaError>;

    async fn put_manifest(
        &self,
        name: &str,
        reference: &str,
        entry: ManifestEntry,
    ) -> Result<(), OrbChrysaError>;

    async fn delete_manifest(&self, name: &str, digest: &Digest) -> Result<(), OrbChrysaError>;

    async fn list_tags(
        &self,
        name: &str,
        n: Option<usize>,
        last: Option<&str>,
    ) -> Result<Vec<String>, OrbChrysaError>;

    async fn list_repositories(
        &self,
        n: Option<usize>,
        last: Option<&str>,
    ) -> Result<Vec<String>, OrbChrysaError>;

    async fn list_repository_summaries(&self) -> Result<Vec<RepositorySummary>, OrbChrysaError>;

    async fn list_manifest_summaries(
        &self,
        name: &str,
    ) -> Result<Vec<ManifestSummary>, OrbChrysaError>;

    async fn delete_tag(
        &self,
        name: &str,
        digest: &Digest,
        tag: &str,
    ) -> Result<bool, OrbChrysaError>;

    async fn delete_repository(&self, name: &str) -> Result<DeleteCounts, OrbChrysaError>;

    async fn delete_manifests(
        &self,
        name: &str,
        digests: &[Digest],
    ) -> Result<DeleteCounts, OrbChrysaError>;

    async fn list_referrers(
        &self,
        name: &str,
        subject_digest: &Digest,
        artifact_type: Option<&str>,
    ) -> Result<Vec<ReferrerEntry>, OrbChrysaError>;

    async fn mount_blob(
        &self,
        source_repo: &str,
        dest_repo: &str,
        digest: &Digest,
    ) -> Result<(), OrbChrysaError>;

    async fn record_blob_delete_request(
        &self,
        digest: &Digest,
    ) -> Result<BlobDeleteStatus, OrbChrysaError>;

    async fn blob_lifecycle_status(
        &self,
        digest: &Digest,
    ) -> Result<BlobLifecycleStatus, OrbChrysaError>;

    async fn clear_blob_delete_request(&self, digest: &Digest) -> Result<(), OrbChrysaError>;

    /// Return all blob reference counts (used by GC).
    #[allow(dead_code)]
    async fn blob_ref_counts(
        &self,
    ) -> Result<std::collections::BTreeMap<String, u64>, OrbChrysaError>;
}

// ── Mirror configuration ──────────────────────────────────────────────

#[async_trait]
#[async_trait]
pub trait MirrorConfigStore: Send + Sync + 'static {
    // Mirror rule CRUD
    async fn list_mirror_rules(&self) -> Result<Vec<MirrorRule>, OrbChrysaError>;
    async fn get_mirror_rule(&self, id: &str) -> Result<Option<MirrorRule>, OrbChrysaError>;
    async fn put_mirror_rule(&self, rule: MirrorRule) -> Result<(), OrbChrysaError>;
    async fn delete_mirror_rule(&self, id: &str) -> Result<(), OrbChrysaError>;

    async fn trigger_mirror_rule(&self, id: &str) -> Result<Option<SyncJob>, OrbChrysaError>;

    // Proxy cache CRUD
    async fn list_proxy_caches(&self) -> Result<Vec<ProxyCache>, OrbChrysaError>;
    async fn get_proxy_cache(&self, id: &str) -> Result<Option<ProxyCache>, OrbChrysaError>;
    async fn put_proxy_cache(&self, cache: ProxyCache) -> Result<(), OrbChrysaError>;
    async fn delete_proxy_cache(&self, id: &str) -> Result<(), OrbChrysaError>;
    async fn trigger_proxy_cache_warm(&self, id: &str) -> Result<Option<SyncJob>, OrbChrysaError>;

    // Warm image CRUD
    async fn list_warm_images(&self) -> Result<Vec<WarmImage>, OrbChrysaError>;
    async fn get_warm_image(&self, id: &str) -> Result<Option<WarmImage>, OrbChrysaError>;
    async fn put_warm_image(&self, image: WarmImage) -> Result<(), OrbChrysaError>;
    async fn delete_warm_image(&self, id: &str) -> Result<(), OrbChrysaError>;
}

// ── Sync job execution tracking ───────────────────────────────────────

#[async_trait]
#[async_trait]
pub trait JobStore: Send + Sync + 'static {
    async fn list_sync_jobs(&self) -> Result<Vec<SyncJob>, OrbChrysaError>;
    async fn get_sync_job(&self, id: &str) -> Result<Option<SyncJob>, OrbChrysaError>;
    async fn put_sync_job(&self, job: SyncJob) -> Result<(), OrbChrysaError>;
    async fn delete_sync_job(&self, id: &str) -> Result<(), OrbChrysaError>;
    async fn claim_sync_job(&self, id: &str, node_id: &str) -> Result<bool, OrbChrysaError>;
    async fn trigger_sync_job(&self, id: &str) -> Result<bool, OrbChrysaError>;

    // Sync job runs
    async fn list_sync_job_runs(
        &self,
        job_id: &str,
        limit: usize,
    ) -> Result<Vec<SyncJobRun>, OrbChrysaError>;
    async fn put_sync_job_run(&self, run: SyncJobRun) -> Result<(), OrbChrysaError>;
}

// ── Personal access tokens ────────────────────────────────────────────

#[async_trait]
#[async_trait]
pub trait TokenStore: Send + Sync + 'static {
    async fn list_personal_access_tokens(
        &self,
        subject: &str,
    ) -> Result<Vec<PersonalAccessToken>, OrbChrysaError>;
    async fn get_personal_access_token_by_hash(
        &self,
        token_hash: &str,
    ) -> Result<Option<PersonalAccessToken>, OrbChrysaError>;
    async fn put_personal_access_token(
        &self,
        token: PersonalAccessToken,
    ) -> Result<(), OrbChrysaError>;
    async fn delete_personal_access_token(
        &self,
        id: &str,
        subject: &str,
    ) -> Result<bool, OrbChrysaError>;
}

// ── Helm charts ───────────────────────────────────────────────────────

#[async_trait]
#[async_trait]
pub trait HelmStore: Send + Sync + 'static {
    async fn list_helm_charts(&self) -> Result<Vec<HelmChart>, OrbChrysaError>;
    async fn list_helm_chart_versions(
        &self,
        name: &str,
    ) -> Result<Option<Vec<HelmChartVersion>>, OrbChrysaError>;
}

// ── Domain supertrait aliases ─────────────────────────────────────────

/// OCI registry core: manifest CRUD + mirror config + blob lifecycle.
pub trait RegistryStore: ManifestStore + MirrorConfigStore {}
impl<T: ManifestStore + MirrorConfigStore> RegistryStore for T {}

/// Admin API: mirror rules, proxy caches, warm images, sync jobs, helm.
pub trait AdminStore: MirrorConfigStore + JobStore + HelmStore {}
impl<T: MirrorConfigStore + JobStore + HelmStore> AdminStore for T {}

/// Scheduler: mirror config + sync job execution + manifest reads.
pub trait SchedulerStore: ManifestStore + MirrorConfigStore + JobStore {}
impl<T: ManifestStore + MirrorConfigStore + JobStore> SchedulerStore for T {}

// ── Supertrait for backward compatibility ─────────────────────────────

pub trait MetadataStore:
    ManifestStore + MirrorConfigStore + JobStore + TokenStore + HelmStore
{
}

impl<T: ManifestStore + MirrorConfigStore + JobStore + TokenStore + HelmStore> MetadataStore for T {}

#[derive(Debug, Clone)]
pub struct ReferrerEntry {
    pub digest: Digest,
    pub media_type: String,
    pub size: u64,
    pub artifact_type: Option<String>,
    pub annotations: Option<serde_json::Value>,
}

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::oci::digest::Digest;

pub(crate) fn now_epoch() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Build manifest summaries from a repository's manifests and tags.
///
/// Shared by both StateMachineData and InMemoryMetadataStore to
/// guarantee identical output from both read paths.
pub(crate) fn build_manifest_summaries(
    repo_manifests: &BTreeMap<String, ManifestEntry>,
    repo_tags: Option<&BTreeMap<String, String>>,
) -> Vec<ManifestSummary> {
    let mut by_digest: BTreeMap<String, Vec<String>> = BTreeMap::new();
    if let Some(repo_tags) = repo_tags {
        for (tag, digest) in repo_tags {
            by_digest
                .entry(digest.clone())
                .or_default()
                .push(tag.clone());
        }
    }

    let mut summaries = Vec::new();
    for (digest, entry) in repo_manifests {
        let body = serde_json::from_slice(&entry.body).unwrap_or(serde_json::Value::Null);
        let mut tags = by_digest.remove(digest).unwrap_or_default();
        tags.sort();
        summaries.push(ManifestSummary {
            digest: digest.clone(),
            media_type: entry.content_type.clone(),
            artifact_type: entry.artifact_type.clone(),
            size_bytes: entry.size_bytes,
            created_at: entry.created_at,
            last_modified: entry.last_modified,
            tags,
            subject: entry.subject.as_ref().map(ToString::to_string),
            annotations: entry.annotations.clone(),
            config_summary: entry.config_summary.clone(),
            body,
        });
    }
    summaries
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestEntry {
    #[serde(with = "crate::oci::digest::serde_string")]
    pub digest: Digest,
    pub content_type: String,
    pub body: Vec<u8>,
    #[serde(default, with = "crate::oci::digest::serde_string_vec")]
    pub referenced_blobs: Vec<Digest>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::oci::digest::serde_string_opt"
    )]
    pub subject: Option<Digest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<serde_json::Value>,
    #[serde(default)]
    pub size_bytes: u64,
    #[serde(default)]
    pub created_at: u64,
    #[serde(default)]
    pub last_modified: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_summary: Option<serde_json::Value>,
}

impl ManifestEntry {
    /// Build a `ManifestEntry` from a parsed OCI manifest JSON value.
    /// The caller should provide the content type, raw body, and already-checked
    /// referenced blob digests.
    pub fn from_parsed_json(
        parsed: &serde_json::Value,
        content_type: String,
        body: Vec<u8>,
        referenced_blobs: Vec<Digest>,
    ) -> Self {
        let size_bytes = body.len() as u64;
        let digest = Digest::sha256(&body);
        let subject = crate::oci::manifest::extract_subject_digest(parsed);
        let artifact_type = crate::oci::manifest::extract_artifact_type(parsed);
        let annotations = crate::oci::manifest::extract_annotations(parsed);
        let config_summary = crate::oci::manifest::extract_config_summary(parsed);
        let now = now_epoch();
        Self {
            digest,
            content_type,
            body,
            referenced_blobs,
            subject,
            artifact_type,
            annotations,
            size_bytes,
            created_at: now,
            last_modified: now,
            config_summary,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum MirrorDirection {
    #[default]
    Pull,
    Push,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MirrorStrategy {
    #[default]
    All,
    Latest {
        count: u32,
    },
    Pattern {
        pattern: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum OutboundProxyProtocol {
    #[default]
    None,
    Http,
    Https,
    Socks4,
    Socks5,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct OutboundProxy {
    #[serde(default)]
    pub protocol: OutboundProxyProtocol,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OutboundProxyPublic {
    pub protocol: OutboundProxyProtocol,
    pub url: Option<String>,
    pub username_configured: bool,
    pub password_configured: bool,
}

impl From<&OutboundProxy> for OutboundProxyPublic {
    fn from(proxy: &OutboundProxy) -> Self {
        Self {
            protocol: proxy.protocol.clone(),
            url: proxy.url.clone(),
            username_configured: proxy.username.as_ref().is_some_and(|v| !v.is_empty()),
            password_configured: proxy.password.as_ref().is_some_and(|v| !v.is_empty()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonalAccessToken {
    pub id: String,
    pub subject: String,
    pub name: String,
    pub token_hash: String,
    pub token_prefix: String,
    pub scopes: Vec<String>,
    pub created_at: u64,
    #[serde(default)]
    pub last_used_at: Option<u64>,
    #[serde(default)]
    pub expires_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MirrorRule {
    pub id: String,
    #[serde(default)]
    pub direction: MirrorDirection,
    pub local_prefix: String,
    pub upstream_registry: String,
    #[serde(default)]
    pub upstream_prefix: Option<String>,
    #[serde(default)]
    pub schedule: Option<String>,
    #[serde(default)]
    pub strategy: MirrorStrategy,
    #[serde(default)]
    pub plain_http: bool,
    #[serde(default)]
    pub insecure_tls: bool,
    #[serde(default)]
    pub outbound_proxy: OutboundProxy,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub created_at: u64,
}

/// UI-safe mirror rule — credentials are never serialized.
#[derive(Debug, Clone, Serialize)]
pub struct MirrorRulePublic {
    pub id: String,
    pub direction: MirrorDirection,
    pub local_prefix: String,
    pub upstream_registry: String,
    pub upstream_prefix: Option<String>,
    pub schedule: Option<String>,
    pub strategy: MirrorStrategy,
    pub plain_http: bool,
    pub insecure_tls: bool,
    pub outbound_proxy: OutboundProxyPublic,
    pub username_configured: bool,
    pub password_configured: bool,
    pub created_at: u64,
}

impl From<&MirrorRule> for MirrorRulePublic {
    fn from(r: &MirrorRule) -> Self {
        Self {
            id: r.id.clone(),
            direction: r.direction.clone(),
            local_prefix: r.local_prefix.clone(),
            upstream_registry: r.upstream_registry.clone(),
            upstream_prefix: r.upstream_prefix.clone(),
            schedule: r.schedule.clone(),
            strategy: r.strategy.clone(),
            plain_http: r.plain_http,
            insecure_tls: r.insecure_tls,
            outbound_proxy: OutboundProxyPublic::from(&r.outbound_proxy),
            username_configured: r.username.as_ref().is_some_and(|v| !v.is_empty()),
            password_configured: r.password.as_ref().is_some_and(|v| !v.is_empty()),
            created_at: r.created_at,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WarmFilter {
    None,
    All,
    Latest { count: u32, sort_by: WarmSortBy },
    Pattern { pattern: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum WarmSortBy {
    Created,
    #[default]
    Pushed,
    Pulled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyCache {
    pub id: String,
    pub local_prefix: String,
    pub upstream_registry: String,
    #[serde(default)]
    pub upstream_prefix: Option<String>,
    #[serde(default)]
    pub warm_filters: Vec<WarmFilter>,
    #[serde(default)]
    pub warm_schedule: Option<String>,
    #[serde(default)]
    pub plain_http: bool,
    #[serde(default)]
    pub insecure_tls: bool,
    #[serde(default)]
    pub outbound_proxy: OutboundProxy,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub created_at: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProxyCachePublic {
    pub id: String,
    pub local_prefix: String,
    pub upstream_registry: String,
    pub upstream_prefix: Option<String>,
    pub warm_filters: Vec<WarmFilter>,
    pub warm_schedule: Option<String>,
    pub plain_http: bool,
    pub insecure_tls: bool,
    pub outbound_proxy: OutboundProxyPublic,
    pub username_configured: bool,
    pub password_configured: bool,
    pub created_at: u64,
}

impl From<&ProxyCache> for ProxyCachePublic {
    fn from(cache: &ProxyCache) -> Self {
        Self {
            id: cache.id.clone(),
            local_prefix: cache.local_prefix.clone(),
            upstream_registry: cache.upstream_registry.clone(),
            upstream_prefix: cache.upstream_prefix.clone(),
            warm_filters: cache.warm_filters.clone(),
            warm_schedule: cache.warm_schedule.clone(),
            plain_http: cache.plain_http,
            insecure_tls: cache.insecure_tls,
            outbound_proxy: OutboundProxyPublic::from(&cache.outbound_proxy),
            username_configured: cache.username.as_ref().is_some_and(|v| !v.is_empty()),
            password_configured: cache.password.as_ref().is_some_and(|v| !v.is_empty()),
            created_at: cache.created_at,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WarmImage {
    pub id: String,
    pub image: String,
    pub tags: Vec<String>,
    #[serde(default = "default_warm_interval")]
    pub interval_secs: u64,
}

fn default_warm_interval() -> u64 {
    3600
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncJob {
    pub id: String,
    #[serde(default)]
    pub kind: SyncJobKind,
    #[serde(default)]
    pub rule_id: Option<String>,
    #[serde(default)]
    pub rule_name: Option<String>,
    pub image: String,
    pub tags: Vec<String>,
    pub interval_secs: u64,
    pub status: SyncJobStatus,
    pub claimed_by: Option<String>,
    pub claimed_at: Option<u64>,
    pub last_run_at: Option<u64>,
    pub next_run_at: u64,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SyncJobKind {
    #[default]
    LegacyWarm,
    Mirror,
    ProxyCache,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum SyncJobStatus {
    Idle,
    Running,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncJobRun {
    pub id: String,
    pub job_id: String,
    pub node_id: String,
    pub started_at: u64,
    pub finished_at: Option<u64>,
    pub status: SyncRunStatus,
    pub tags_synced: Vec<String>,
    pub tags_failed: Vec<(String, String)>,
}

pub(crate) fn proxy_cache_warm_job(cache: &ProxyCache, now: u64) -> SyncJob {
    SyncJob {
        id: format!("{}-warm-{}", cache.id, now),
        kind: SyncJobKind::ProxyCache,
        rule_id: Some(cache.id.clone()),
        rule_name: Some(cache.id.clone()),
        image: cache.local_prefix.clone(),
        tags: warm_filter_labels(&cache.warm_filters),
        interval_secs: 0,
        status: SyncJobStatus::Idle,
        claimed_by: None,
        claimed_at: None,
        last_run_at: None,
        next_run_at: now,
        last_error: None,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum SyncRunStatus {
    Running,
    Succeeded,
    PartialFailure,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HelmChart {
    pub name: String,
    pub description: String,
    pub latest_version: String,
    pub versions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HelmChartVersion {
    pub name: String,
    pub version: String,
    pub app_version: Option<String>,
    pub description: String,
    pub created: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RepositorySummary {
    pub name: String,
    pub tag_count: usize,
    pub manifest_count: usize,
    pub size_bytes: u64,
    pub last_modified: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestSummary {
    pub digest: String,
    pub media_type: String,
    pub artifact_type: Option<String>,
    pub size_bytes: u64,
    pub created_at: u64,
    pub last_modified: u64,
    pub tags: Vec<String>,
    pub subject: Option<String>,
    pub annotations: Option<serde_json::Value>,
    pub config_summary: Option<serde_json::Value>,
    pub body: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteCounts {
    pub deleted_manifests: usize,
    pub deleted_tags: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BlobDeleteStatus {
    pub digest: String,
    pub referenced: bool,
    pub ref_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BlobLifecycleStatus {
    pub digest: String,
    pub referenced: bool,
    pub ref_count: u64,
    pub delete_requested: bool,
}

pub(crate) fn mirror_strategy_labels(strategy: &MirrorStrategy) -> Vec<String> {
    match strategy {
        MirrorStrategy::All => vec!["all".to_string()],
        MirrorStrategy::Latest { count } => vec![format!("latest {}", count)],
        MirrorStrategy::Pattern { pattern } => vec![pattern.clone()],
    }
}

pub(crate) fn warm_filter_labels(filters: &[WarmFilter]) -> Vec<String> {
    filters
        .iter()
        .filter_map(|filter| match filter {
            WarmFilter::None => None,
            WarmFilter::All => Some("all".to_string()),
            WarmFilter::Latest { count, .. } => Some(format!("latest {}", count)),
            WarmFilter::Pattern { pattern } => Some(pattern.clone()),
        })
        .collect()
}

pub(crate) fn mirror_rule_job(
    rule: &MirrorRule,
    id: String,
    now: u64,
    interval_secs: u64,
) -> SyncJob {
    SyncJob {
        id,
        kind: SyncJobKind::Mirror,
        rule_id: Some(rule.id.clone()),
        rule_name: Some(rule.id.clone()),
        image: rule.local_prefix.clone(),
        tags: mirror_strategy_labels(&rule.strategy),
        interval_secs,
        status: SyncJobStatus::Idle,
        claimed_by: None,
        claimed_at: None,
        last_run_at: None,
        next_run_at: now,
        last_error: None,
    }
}

pub(crate) fn sync_job_blocks_trigger(
    job: &SyncJob,
    kind: SyncJobKind,
    rule_id: &str,
    now: u64,
) -> bool {
    job.kind == kind
        && job.rule_id.as_deref() == Some(rule_id)
        && (job.status == SyncJobStatus::Running
            || (job.interval_secs == 0 && job.next_run_at <= now))
}

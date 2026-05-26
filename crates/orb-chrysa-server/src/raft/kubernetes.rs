use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use thiserror::Error;
use tokio::time::MissedTickBehavior;

use crate::config::RaftKubernetesConfig;

use super::RaftInstance;
use super::membership;

const SERVICE_ACCOUNT_TOKEN_PATH: &str = "/var/run/secrets/kubernetes.io/serviceaccount/token";
const SERVICE_ACCOUNT_CA_PATH: &str = "/var/run/secrets/kubernetes.io/serviceaccount/ca.crt";
const KUBERNETES_API_BASE: &str = "https://kubernetes.default.svc";

#[derive(Debug, Error)]
enum KubernetesError {
    #[error("failed to read service account token: {0}")]
    Token(std::io::Error),
    #[error("failed to read service account CA: {0}")]
    Ca(std::io::Error),
    #[error("failed to parse service account CA: {0}")]
    CaParse(String),
    #[error("failed to build Kubernetes API client: {0}")]
    Client(String),
    #[error("Kubernetes API request failed: {0}")]
    Http(String),
    #[error("Kubernetes API JSON response failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("StatefulSet spec.replicas is missing")]
    MissingReplicas,
}

#[derive(Debug, Deserialize)]
struct StatefulSet {
    spec: StatefulSetSpec,
}

#[derive(Debug, Deserialize)]
struct StatefulSetSpec {
    replicas: Option<u32>,
}

struct KubernetesClient {
    client: aioduct::TokioClient,
    namespace: String,
    statefulset_name: String,
}

impl KubernetesClient {
    fn from_service_account(config: &RaftKubernetesConfig) -> Result<Self, KubernetesError> {
        let ca_pem = std::fs::read(SERVICE_ACCOUNT_CA_PATH).map_err(KubernetesError::Ca)?;
        let roots = aioduct::Certificate::from_pem(&ca_pem)
            .map_err(|e| KubernetesError::CaParse(e.to_string()))?;
        let client = aioduct::TokioClient::builder()
            .timeout(Duration::from_secs(5))
            .add_root_certificates(&roots)
            .build()
            .map_err(|e| KubernetesError::Client(e.to_string()))?;

        Ok(Self {
            client,
            namespace: config.namespace.clone(),
            statefulset_name: config.statefulset_name.clone(),
        })
    }

    async fn fetch_desired_replicas(&self) -> Result<u32, KubernetesError> {
        let url = format!(
            "{KUBERNETES_API_BASE}/apis/apps/v1/namespaces/{}/statefulsets/{}",
            self.namespace, self.statefulset_name
        );
        let token = read_service_account_token()?;
        let resp = self
            .client
            .get(&url)
            .map_err(|e| KubernetesError::Http(e.to_string()))?
            .bearer_auth(&token)
            .header_str("accept", "application/json")
            .map_err(|e| KubernetesError::Http(e.to_string()))?
            .send()
            .await
            .map_err(|e| KubernetesError::Http(e.to_string()))?
            .error_for_status()
            .map_err(|e| KubernetesError::Http(e.to_string()))?;

        let bytes = resp
            .bytes()
            .await
            .map_err(|e| KubernetesError::Http(e.to_string()))?;
        let sts: StatefulSet = serde_json::from_slice(&bytes)?;
        sts.spec.replicas.ok_or(KubernetesError::MissingReplicas)
    }
}

fn read_service_account_token() -> Result<String, KubernetesError> {
    std::fs::read_to_string(SERVICE_ACCOUNT_TOKEN_PATH)
        .map(|token| token.trim().to_string())
        .map_err(KubernetesError::Token)
}

pub(crate) async fn reconcile_statefulset_replicas(
    raft: Arc<RaftInstance>,
    config: RaftKubernetesConfig,
) {
    if !config.enabled {
        return;
    }

    let client = match KubernetesClient::from_service_account(&config) {
        Ok(client) => client,
        Err(e) => {
            tracing::warn!(err = %e, "Kubernetes Raft membership reconciler disabled");
            return;
        }
    };

    let mut interval = tokio::time::interval(Duration::from_secs(config.reconcile_seconds));
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop {
        interval.tick().await;
        if let Err(e) = reconcile_once(&raft, &client).await {
            tracing::warn!(err = %e, "Kubernetes Raft membership reconciliation failed");
        }
    }
}

async fn reconcile_once(
    raft: &Arc<RaftInstance>,
    client: &KubernetesClient,
) -> Result<(), KubernetesError> {
    let replicas = client.fetch_desired_replicas().await?;
    let desired_voters = desired_voters_for_replicas(replicas);
    if desired_voters.is_empty() {
        tracing::warn!("ignoring Kubernetes StatefulSet replica count of zero");
        return Ok(());
    }

    let metrics = raft.metrics().borrow().clone();
    if metrics.current_leader != Some(metrics.id) {
        return Ok(());
    }

    let current_voters: BTreeSet<u64> = metrics.membership_config.voter_ids().collect();
    drop(metrics);

    if desired_voters == current_voters || !desired_voters.is_subset(&current_voters) {
        return Ok(());
    }

    membership::replace_voters(raft, desired_voters, "kubernetes_statefulset_scale_down")
        .await
        .map_err(KubernetesError::Http)
}

pub(crate) fn desired_voters_for_replicas(replicas: u32) -> BTreeSet<u64> {
    (1..=u64::from(replicas)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desired_voters_match_statefulset_ordinals() {
        assert_eq!(desired_voters_for_replicas(0), BTreeSet::new());
        assert_eq!(desired_voters_for_replicas(1), BTreeSet::from([1]));
        assert_eq!(desired_voters_for_replicas(3), BTreeSet::from([1, 2, 3]));
    }
}

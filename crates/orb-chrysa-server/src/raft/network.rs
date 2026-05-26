use std::future::Future;
use std::io;
use std::io::BufReader;
use std::sync::Arc;
use std::sync::Once;

use axum_server::tls_rustls::RustlsConfig;
use openraft::error::{NetworkError, RPCError, RaftError, ReplicationClosed, StreamingError};
use openraft::network::{RPCOption, RaftNetwork, RaftNetworkFactory};
use openraft::raft::{
    AppendEntriesRequest, AppendEntriesResponse, SnapshotResponse, VoteRequest, VoteResponse,
};
use openraft::storage::Snapshot;
use openraft::{BasicNode, Vote};

use super::{RaftInstance, Request, Response as RaftResponse, TypeConfig};
use crate::config::RaftTlsConfig;

static RUSTLS_PROVIDER: Once = Once::new();

fn install_rustls_provider() {
    RUSTLS_PROVIDER.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

fn bc_encode<T: serde::Serialize>(val: &T) -> Result<Vec<u8>, serde_json::Error> {
    serde_json::to_vec(val)
}

fn bc_decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, serde_json::Error> {
    serde_json::from_slice(bytes)
}

pub struct NetworkFactory {
    pub tls: Option<Arc<RaftTlsConfig>>,
}

impl RaftNetworkFactory<TypeConfig> for NetworkFactory {
    type Network = NetworkClient;

    async fn new_client(&mut self, _target: u64, node: &BasicNode) -> Self::Network {
        let client = build_rpc_client(self.tls.as_deref());
        NetworkClient {
            addr: node.addr.clone(),
            tls: self.tls.clone(),
            client,
        }
    }
}

pub struct NetworkClient {
    addr: String,
    tls: Option<Arc<RaftTlsConfig>>,
    client: Result<aioduct::TokioClient, String>,
}

fn net_err(msg: &str) -> RPCError<u64, BasicNode, RaftError<u64>> {
    RPCError::Network(NetworkError::new(&AnyErr(msg.to_string())))
}

#[derive(Debug)]
struct AnyErr(String);
impl std::fmt::Display for AnyErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for AnyErr {}

impl RaftNetwork<TypeConfig> for NetworkClient {
    async fn append_entries(
        &mut self,
        rpc: AppendEntriesRequest<TypeConfig>,
        _option: RPCOption,
    ) -> Result<AppendEntriesResponse<u64>, RPCError<u64, BasicNode, RaftError<u64>>> {
        let scheme = if self.tls.is_some() { "https" } else { "http" };
        let url = format!("{}://{}/raft/append", scheme, self.addr);
        let body = bc_encode(&rpc).map_err(|e| net_err(&e.to_string()))?;

        let client = self.client.as_ref().map_err(|e| net_err(e))?;
        let resp = send_rpc(client, &url, &body)
            .await
            .map_err(|e| net_err(&e.to_string()))?;

        bc_decode(&resp).map_err(|e| net_err(&e.to_string()))
    }

    async fn vote(
        &mut self,
        rpc: VoteRequest<u64>,
        _option: RPCOption,
    ) -> Result<VoteResponse<u64>, RPCError<u64, BasicNode, RaftError<u64>>> {
        let scheme = if self.tls.is_some() { "https" } else { "http" };
        let url = format!("{}://{}/raft/vote", scheme, self.addr);
        let body = bc_encode(&rpc).map_err(|e| net_err(&e.to_string()))?;

        let client = self.client.as_ref().map_err(|e| net_err(e))?;
        let resp = send_rpc(client, &url, &body)
            .await
            .map_err(|e| net_err(&e.to_string()))?;

        bc_decode(&resp).map_err(|e| net_err(&e.to_string()))
    }

    async fn full_snapshot(
        &mut self,
        vote: Vote<u64>,
        snapshot: Snapshot<TypeConfig>,
        _cancel: impl Future<Output = ReplicationClosed> + Send + 'static,
        _option: RPCOption,
    ) -> Result<SnapshotResponse<u64>, StreamingError<TypeConfig, openraft::error::Fatal<u64>>>
    {
        let scheme = if self.tls.is_some() { "https" } else { "http" };
        let url = format!("{}://{}/raft/snapshot", scheme, self.addr);

        let snapshot_bytes = snapshot.snapshot.into_inner();
        let meta_bytes = bc_encode(&snapshot.meta).map_err(|e| {
            StreamingError::StorageError(openraft::StorageError::IO {
                source: openraft::StorageIOError::write_snapshot(None, &AnyErr(e.to_string())),
            })
        })?;

        let payload = SnapshotPayload {
            vote,
            meta: meta_bytes,
            data: snapshot_bytes,
        };
        let body = bc_encode(&payload).map_err(|e| {
            StreamingError::StorageError(openraft::StorageError::IO {
                source: openraft::StorageIOError::write_snapshot(None, &AnyErr(e.to_string())),
            })
        })?;

        let client = self
            .client
            .as_ref()
            .map_err(|e| StreamingError::Network(NetworkError::new(&AnyErr(e.clone()))))?;
        let resp = send_rpc(client, &url, &body)
            .await
            .map_err(|e| StreamingError::Network(NetworkError::new(&AnyErr(e.to_string()))))?;

        bc_decode(&resp)
            .map_err(|e| StreamingError::Network(NetworkError::new(&AnyErr(e.to_string()))))
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct SnapshotPayload {
    vote: Vote<u64>,
    meta: Vec<u8>,
    data: Vec<u8>,
}

async fn send_rpc(
    client: &aioduct::TokioClient,
    url: &str,
    body: &[u8],
) -> Result<Vec<u8>, String> {
    let resp = client
        .post(url)
        .map_err(|e| e.to_string())?
        .header_str("content-type", "application/octet-stream")
        .map_err(|e| e.to_string())?
        .body(bytes::Bytes::from(body.to_vec()))
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?;

    let bytes = resp.bytes().await.map_err(|e| e.to_string())?;

    Ok(bytes.to_vec())
}

pub async fn forward_client_write(
    addr: &str,
    tls: Option<Arc<RaftTlsConfig>>,
    req: &Request,
) -> Result<RaftResponse, String> {
    if addr.is_empty() {
        return Err("leader address is unknown".to_string());
    }

    let client = build_rpc_client(tls.as_deref())?;
    let scheme = if tls.is_some() { "https" } else { "http" };
    let url = format!("{}://{}/raft/write", scheme, addr);
    let body = bc_encode(req).map_err(|e| e.to_string())?;
    let resp = send_rpc(&client, &url, &body).await?;

    bc_decode(&resp).map_err(|e| e.to_string())
}

pub(crate) fn build_rpc_client(
    tls: Option<&RaftTlsConfig>,
) -> Result<aioduct::TokioClient, String> {
    build_rpc_client_with_timeout(tls, std::time::Duration::from_secs(60))
}

pub(crate) fn build_rpc_client_with_timeout(
    tls: Option<&RaftTlsConfig>,
    timeout: std::time::Duration,
) -> Result<aioduct::TokioClient, String> {
    install_rustls_provider();

    let mut builder = aioduct::TokioClient::builder().timeout(timeout);
    if let Some(tls) = tls {
        let ca_pem = std::fs::read(&tls.server_ca_path)
            .map_err(|e| format!("failed to read raft server CA: {e}"))?;
        let roots = aioduct::Certificate::from_pem(&ca_pem)
            .map_err(|e| format!("failed to parse raft server CA: {e}"))?;
        let mut identity_pem =
            std::fs::read(&tls.cert_path).map_err(|e| format!("failed to read raft cert: {e}"))?;
        identity_pem.push(b'\n');
        identity_pem.extend(
            std::fs::read(&tls.key_path).map_err(|e| format!("failed to read raft key: {e}"))?,
        );
        let identity = aioduct::Identity::from_pem(&identity_pem)
            .map_err(|e| format!("failed to parse raft client identity: {e}"))?;
        builder = builder.add_root_certificates(&roots).identity(identity);
    }
    builder.build().map_err(|e| e.to_string())
}

pub async fn raft_rustls_config(tls: &RaftTlsConfig) -> io::Result<RustlsConfig> {
    install_rustls_provider();

    let certs = load_certs(&tls.cert_path)?;
    let key = load_key(&tls.key_path)?;
    let mut client_roots = rustls::RootCertStore::empty();
    for cert in load_certs(&tls.client_ca_path)? {
        client_roots.add(cert).map_err(io::Error::other)?;
    }
    let client_verifier = rustls::server::WebPkiClientVerifier::builder(client_roots.into())
        .build()
        .map_err(io::Error::other)?;
    let mut config = rustls::ServerConfig::builder()
        .with_client_cert_verifier(client_verifier)
        .with_single_cert(certs, key)
        .map_err(io::Error::other)?;
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    Ok(RustlsConfig::from_config(Arc::new(config)))
}

fn load_certs(path: &str) -> io::Result<Vec<rustls::pki_types::CertificateDer<'static>>> {
    let bytes = std::fs::read(path)?;
    let mut reader = BufReader::new(bytes.as_slice());
    rustls_pemfile::certs(&mut reader).collect::<Result<Vec<_>, _>>()
}

fn load_key(path: &str) -> io::Result<rustls::pki_types::PrivateKeyDer<'static>> {
    let bytes = std::fs::read(path)?;
    let mut reader = BufReader::new(bytes.as_slice());
    rustls_pemfile::private_key(&mut reader)?
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "no private key found"))
}

// ── Raft RPC server (axum handlers) ──

use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};

pub fn raft_routes(raft: Arc<RaftInstance>) -> Router {
    Router::new()
        .route("/raft/append", post(handle_append))
        .route("/raft/vote", post(handle_vote))
        .route("/raft/snapshot", post(handle_snapshot))
        .route("/raft/write", post(handle_write))
        .route("/raft/join", post(super::membership::handle_join))
        .route("/raft/leave", post(super::membership::handle_leave))
        .route("/raft/status", get(super::membership::handle_status))
        .with_state(raft)
}

async fn handle_append(State(raft): State<Arc<RaftInstance>>, body: axum::body::Bytes) -> Response {
    let rpc: AppendEntriesRequest<TypeConfig> = match bc_decode(&body) {
        Ok(r) => r,
        Err(e) => return (StatusCode::BAD_REQUEST, format!("deserialize: {}", e)).into_response(),
    };

    match raft.append_entries(rpc).await {
        Ok(resp) => match bc_encode(&resp) {
            Ok(bytes) => (StatusCode::OK, bytes).into_response(),
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("serialize: {}", e),
            )
                .into_response(),
        },
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{}", e)).into_response(),
    }
}

async fn handle_vote(State(raft): State<Arc<RaftInstance>>, body: axum::body::Bytes) -> Response {
    let rpc: VoteRequest<u64> = match bc_decode(&body) {
        Ok(r) => r,
        Err(e) => return (StatusCode::BAD_REQUEST, format!("deserialize: {}", e)).into_response(),
    };

    match raft.vote(rpc).await {
        Ok(resp) => match bc_encode(&resp) {
            Ok(bytes) => (StatusCode::OK, bytes).into_response(),
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("serialize: {}", e),
            )
                .into_response(),
        },
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{}", e)).into_response(),
    }
}

async fn handle_snapshot(
    State(raft): State<Arc<RaftInstance>>,
    body: axum::body::Bytes,
) -> Response {
    let payload: SnapshotPayload = match bc_decode(&body) {
        Ok(r) => r,
        Err(e) => return (StatusCode::BAD_REQUEST, format!("deserialize: {}", e)).into_response(),
    };

    let meta = match bc_decode(&payload.meta) {
        Ok(m) => m,
        Err(e) => {
            return (StatusCode::BAD_REQUEST, format!("deserialize meta: {}", e)).into_response();
        }
    };

    let snapshot = openraft::storage::Snapshot {
        meta,
        snapshot: Box::new(std::io::Cursor::new(payload.data)),
    };

    match raft.install_full_snapshot(payload.vote, snapshot).await {
        Ok(resp) => match bc_encode(&resp) {
            Ok(bytes) => (StatusCode::OK, bytes).into_response(),
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("serialize: {}", e),
            )
                .into_response(),
        },
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{}", e)).into_response(),
    }
}

async fn handle_write(State(raft): State<Arc<RaftInstance>>, body: axum::body::Bytes) -> Response {
    let req: Request = match bc_decode(&body) {
        Ok(r) => r,
        Err(e) => return (StatusCode::BAD_REQUEST, format!("deserialize: {}", e)).into_response(),
    };

    match raft.client_write(req).await {
        Ok(resp) => match bc_encode(&resp.data) {
            Ok(bytes) => (StatusCode::OK, bytes).into_response(),
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("serialize: {}", e),
            )
                .into_response(),
        },
        Err(e) => (StatusCode::SERVICE_UNAVAILABLE, format!("{}", e)).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Json;
    use axum::routing::{get, post};
    use rcgen::{
        BasicConstraints, CertificateParams, CertifiedIssuer, DistinguishedName, DnType,
        ExtendedKeyUsagePurpose, IsCa, KeyPair, KeyUsagePurpose,
    };
    use time::{Duration, OffsetDateTime};

    #[tokio::test]
    async fn raft_mtls_requires_client_certificate() {
        let temp = tempfile::tempdir().unwrap();
        let tls = write_test_tls(temp.path(), "localhost");
        let rustls_config = raft_rustls_config(&tls).await.unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = axum_server::Handle::new();
        let shutdown = handle.clone();
        tokio::spawn(async move {
            let app = Router::new().route("/ok", get(|| async { "ok" }));
            let _ = axum_server::from_tcp_rustls(listener, rustls_config)
                .unwrap()
                .handle(handle)
                .serve(app.into_make_service())
                .await;
        });

        let good = build_rpc_client(Some(&tls)).unwrap();
        let ok = good
            .get(&format!("https://localhost:{}/ok", addr.port()))
            .unwrap()
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert_eq!(ok, "ok");

        let ca = std::fs::read(&tls.server_ca_path).unwrap();
        let roots = aioduct::Certificate::from_pem(&ca).unwrap();
        let bad = aioduct::TokioClient::builder()
            .add_root_certificates(&roots)
            .build()
            .unwrap();
        let missing_client_cert = bad
            .get(&format!("https://localhost:{}/ok", addr.port()))
            .unwrap()
            .send()
            .await;
        assert!(missing_client_cert.is_err());

        let wrong_dir = temp.path().join("wrong-ca");
        std::fs::create_dir(&wrong_dir).unwrap();
        let wrong_ca = write_test_tls(&wrong_dir, "localhost");
        let wrong_server_ca = RaftTlsConfig {
            server_ca_path: wrong_ca.server_ca_path,
            ..tls.clone()
        };
        let wrong_ca_client = build_rpc_client(Some(&wrong_server_ca)).unwrap();
        let wrong_ca_result = wrong_ca_client
            .get(&format!("https://localhost:{}/ok", addr.port()))
            .unwrap()
            .send()
            .await;
        assert!(wrong_ca_result.is_err());

        shutdown.graceful_shutdown(Some(std::time::Duration::from_secs(1)));
    }

    #[tokio::test]
    async fn membership_helpers_use_raft_mtls_identity() {
        let temp = tempfile::tempdir().unwrap();
        let tls = write_test_tls(temp.path(), "localhost");
        let rustls_config = raft_rustls_config(&tls).await.unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = axum_server::Handle::new();
        let shutdown = handle.clone();
        tokio::spawn(async move {
            let status = std::sync::Arc::new(super::super::membership::ClusterStatus {
                node_id: 1,
                state: super::super::membership::NodeState::Leader,
                leader_id: Some(1),
                leader_addr: Some(format!("localhost:{}", addr.port())),
                voters: vec![super::super::membership::NodeInfo {
                    id: 1,
                    addr: format!("localhost:{}", addr.port()),
                }],
                learners: Vec::new(),
                term: 1,
                last_log_index: Some(1),
                last_applied_log: Some(1),
                last_membership_log_id: Some(1),
                millis_since_quorum_ack: Some(0),
                replication: std::collections::BTreeMap::new(),
            });
            let status_for_route = status.clone();
            let app = Router::new()
                .route(
                    "/raft/status",
                    get(move || {
                        let status = status_for_route.clone();
                        async move { Json((*status).clone()) }
                    }),
                )
                .route(
                    "/raft/join",
                    post(
                        |Json(_): Json<super::super::membership::JoinRequest>| async {
                            Json(super::super::membership::JoinResponse {
                                result: super::super::membership::JoinResult::Ok,
                                leader_addr: None,
                            })
                        },
                    ),
                )
                .route(
                    "/raft/leave",
                    post(
                        |Json(_): Json<super::super::membership::LeaveRequest>| async {
                            Json(super::super::membership::LeaveResponse {
                                result: super::super::membership::LeaveResult::Ok,
                                leader_addr: None,
                            })
                        },
                    ),
                );
            let _ = axum_server::from_tcp_rustls(listener, rustls_config)
                .unwrap()
                .handle(handle)
                .serve(app.into_make_service())
                .await;
        });

        let addr = format!("localhost:{}", addr.port());
        let status = super::super::membership::get_status(&addr, Some(&tls))
            .await
            .unwrap();
        assert_eq!(status.node_id, 1);

        let join = super::super::membership::request_join(
            &addr,
            Some(&tls),
            &super::super::membership::JoinRequest {
                node_id: 2,
                addr: "orb-chrysa-1:5051".to_string(),
            },
        )
        .await
        .unwrap();
        assert_eq!(join.result, super::super::membership::JoinResult::Ok);

        let leave = super::super::membership::request_leave(
            &addr,
            Some(&tls),
            &super::super::membership::LeaveRequest { node_id: 2 },
        )
        .await
        .unwrap();
        assert_eq!(leave.result, super::super::membership::LeaveResult::Ok);

        let missing_client_cert = super::super::membership::get_status(&addr, None).await;
        assert!(missing_client_cert.is_err());

        let wrong_dir = temp.path().join("wrong-ca-status");
        std::fs::create_dir(&wrong_dir).unwrap();
        let wrong_ca = write_test_tls(&wrong_dir, "localhost");
        let wrong_server_ca = RaftTlsConfig {
            server_ca_path: wrong_ca.server_ca_path,
            ..tls.clone()
        };
        let wrong_ca_result =
            super::super::membership::get_status(&addr, Some(&wrong_server_ca)).await;
        assert!(wrong_ca_result.is_err());

        shutdown.graceful_shutdown(Some(std::time::Duration::from_secs(1)));
    }

    fn write_test_tls(dir: &std::path::Path, host: &str) -> RaftTlsConfig {
        let now = OffsetDateTime::now_utc();
        let ca_key = KeyPair::generate().unwrap();
        let ca_key_pem = ca_key.serialize_pem();
        let mut ca_params =
            CertificateParams::new(vec!["orb-chrysa-test-ca.local".to_string()]).unwrap();
        ca_params.distinguished_name = DistinguishedName::new();
        ca_params
            .distinguished_name
            .push(DnType::CommonName, "orb-chrysa-test-ca");
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        ca_params.key_usages = vec![
            KeyUsagePurpose::KeyCertSign,
            KeyUsagePurpose::DigitalSignature,
            KeyUsagePurpose::CrlSign,
        ];
        ca_params.not_before = now - Duration::days(1);
        ca_params.not_after = now + Duration::days(30);
        let ca = CertifiedIssuer::self_signed(ca_params, ca_key).unwrap();

        let leaf_key = KeyPair::generate().unwrap();
        let mut leaf_params = CertificateParams::new(vec![host.to_string()]).unwrap();
        leaf_params.distinguished_name = DistinguishedName::new();
        leaf_params
            .distinguished_name
            .push(DnType::CommonName, host.to_string());
        leaf_params.key_usages = vec![
            KeyUsagePurpose::DigitalSignature,
            KeyUsagePurpose::KeyEncipherment,
        ];
        leaf_params.extended_key_usages = vec![
            ExtendedKeyUsagePurpose::ServerAuth,
            ExtendedKeyUsagePurpose::ClientAuth,
        ];
        leaf_params.not_before = now - Duration::days(1);
        leaf_params.not_after = now + Duration::days(30);
        let leaf = leaf_params.signed_by(&leaf_key, &ca).unwrap();

        let ca_path = dir.join("ca.crt");
        let ca_key_path = dir.join("ca.key");
        let cert_path = dir.join("tls.crt");
        let key_path = dir.join("tls.key");
        std::fs::write(&ca_path, ca.pem()).unwrap();
        std::fs::write(&ca_key_path, ca_key_pem).unwrap();
        std::fs::write(&cert_path, leaf.pem()).unwrap();
        std::fs::write(&key_path, leaf_key.serialize_pem()).unwrap();

        RaftTlsConfig {
            cert_path: cert_path.display().to_string(),
            key_path: key_path.display().to_string(),
            server_ca_path: ca_path.display().to_string(),
            client_ca_path: ca_path.display().to_string(),
        }
    }
}

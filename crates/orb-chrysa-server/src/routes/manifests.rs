use std::sync::Arc;

use axum::body::Body;
use axum::extract::Request;
use axum::http::{HeaderName, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};

use crate::error::OrbChrysaError;
use crate::oci::digest::Digest;
use crate::oci::manifest;
use crate::store::blob::BlobStore;
#[allow(unused_imports)]
use crate::store::metadata::{ManifestEntry, ManifestStore, RegistryStore, now_epoch};

use super::AppState;

fn docker_content_digest_header() -> HeaderName {
    HeaderName::from_static("docker-content-digest")
}

fn oci_subject_header() -> HeaderName {
    HeaderName::from_static("oci-subject")
}

pub async fn dispatch<M: RegistryStore, B: BlobStore>(
    state: Arc<AppState<M, B>>,
    method: &Method,
    name: &str,
    reference: &str,
    req: Request<Body>,
) -> Result<Response, OrbChrysaError> {
    match *method {
        Method::GET => respond_manifest(&state, name, reference, true).await,
        Method::HEAD => respond_manifest(&state, name, reference, false).await,
        Method::PUT => put_manifest(&state, name, reference, req).await,
        Method::DELETE => delete_manifest(&state, name, reference).await,
        _ => Err(OrbChrysaError::Unsupported("method not allowed".into())),
    }
}

async fn resolve_manifest<M: RegistryStore, B: BlobStore>(
    state: &AppState<M, B>,
    name: &str,
    reference: &str,
) -> Result<ManifestEntry, OrbChrysaError> {
    match state.core.metadata.get_manifest(name, reference).await? {
        Some(entry) => Ok(entry),
        None => state
            .mirror
            .pull_manifest(name, reference, &state.core.metadata, &state.core.blobs)
            .await?
            .ok_or_else(|| OrbChrysaError::ManifestUnknown(reference.to_string())),
    }
}

async fn respond_manifest<M: RegistryStore, B: BlobStore>(
    state: &AppState<M, B>,
    name: &str,
    reference: &str,
    include_body: bool,
) -> Result<Response, OrbChrysaError> {
    let entry = resolve_manifest(state, name, reference).await?;
    let headers = [
        ("Content-Type", entry.content_type.as_str()),
        ("Docker-Content-Digest", &entry.digest.to_string()),
        ("Content-Length", &entry.body.len().to_string()),
    ];
    if include_body {
        Ok((StatusCode::OK, headers, entry.body).into_response())
    } else {
        Ok((StatusCode::OK, headers).into_response())
    }
}

async fn put_manifest<M: RegistryStore, B: BlobStore>(
    state: &AppState<M, B>,
    name: &str,
    reference: &str,
    req: Request<Body>,
) -> Result<Response, OrbChrysaError> {
    let headers = req.headers().clone();
    let body = axum::body::to_bytes(req.into_body(), 10 * 1024 * 1024)
        .await
        .map_err(|e| OrbChrysaError::ManifestInvalid(e.to_string()))?;

    let content_type = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/vnd.oci.image.manifest.v1+json")
        .to_string();

    if !manifest::is_manifest_media_type(&content_type) {
        return Err(OrbChrysaError::ManifestInvalid(format!(
            "unsupported media type: {}",
            content_type
        )));
    }

    let parsed: serde_json::Value = serde_json::from_slice(&body)
        .map_err(|e| OrbChrysaError::ManifestInvalid(format!("invalid JSON: {}", e)))?;

    let digests = manifest::extract_referenced_digests(&parsed);
    let blob_checks: Vec<_> = digests.iter().map(|d| state.core.blobs.stat(d)).collect();
    let results = futures::future::join_all(blob_checks).await;
    for (i, result) in results.into_iter().enumerate() {
        if result.is_err() {
            return Err(OrbChrysaError::ManifestBlobUnknown(digests[i].to_string()));
        }
    }

    let digest = Digest::sha256(&body);
    let mut seen_refs = std::collections::BTreeSet::new();
    let referenced_blobs: Vec<Digest> = digests
        .into_iter()
        .filter(|digest| seen_refs.insert(digest.to_string()))
        .collect();
    let entry =
        ManifestEntry::from_parsed_json(&parsed, content_type, body.to_vec(), referenced_blobs);
    let subject = entry.subject.clone();

    state
        .core
        .metadata
        .put_manifest(name, reference, entry)
        .await?;

    let mut resp = StatusCode::CREATED.into_response();
    let location = format!("/v2/{}/manifests/{}", name, digest);
    resp.headers_mut().insert(
        header::LOCATION,
        HeaderValue::from_str(&location)
            .map_err(|e| OrbChrysaError::Serialization(e.to_string()))?,
    );
    resp.headers_mut().insert(
        docker_content_digest_header(),
        HeaderValue::from_str(&digest.to_string())
            .map_err(|e| OrbChrysaError::Serialization(e.to_string()))?,
    );
    if let Some(ref subj) = subject {
        resp.headers_mut().insert(
            oci_subject_header(),
            HeaderValue::from_str(&subj.to_string())
                .map_err(|e| OrbChrysaError::Serialization(e.to_string()))?,
        );
    }
    Ok(resp)
}

async fn delete_manifest<M: RegistryStore, B: BlobStore>(
    state: &AppState<M, B>,
    name: &str,
    reference: &str,
) -> Result<Response, OrbChrysaError> {
    let digest = Digest::from_str_checked(reference)
        .ok_or_else(|| OrbChrysaError::Unsupported("tag deletion not supported".into()))?;

    state.core.metadata.delete_manifest(name, &digest).await?;

    Ok(StatusCode::ACCEPTED.into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::test_state;
    use axum::body::Body;
    use axum::http::{Method, Request};
    use axum::response::IntoResponse;

    fn request(method: Method, reference: &str) -> Request<Body> {
        Request::builder()
            .uri(format!("/v2/test-repo/manifests/{}", reference))
            .method(method)
            .header("Content-Type", "application/vnd.oci.image.manifest.v1+json")
            .body(Body::empty())
            .unwrap()
    }

    #[tokio::test]
    async fn get_nonexistent_manifest_returns_404() {
        let state = test_state();
        let response = dispatch(
            state,
            &Method::GET,
            "test-repo",
            "latest",
            request(Method::GET, "latest"),
        )
        .await
        .unwrap_or_else(|e| e.into_response());
        assert_eq!(response.status(), axum::http::StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn head_nonexistent_manifest_returns_404() {
        let state = test_state();
        let response = dispatch(
            state,
            &Method::HEAD,
            "test-repo",
            "latest",
            request(Method::HEAD, "latest"),
        )
        .await
        .unwrap_or_else(|e| e.into_response());
        assert_eq!(response.status(), axum::http::StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn put_invalid_json_rejected() {
        let state = test_state();
        let response = dispatch(
            state,
            &Method::PUT,
            "test-repo",
            "latest",
            Request::builder()
                .uri("/v2/test-repo/manifests/latest")
                .method(Method::PUT)
                .header("Content-Type", "application/vnd.oci.image.manifest.v1+json")
                .body(Body::from(b"not json".to_vec()))
                .unwrap(),
        )
        .await
        .unwrap_or_else(|e| e.into_response());
        assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn delete_tag_rejected() {
        let state = test_state();
        let response = dispatch(
            state,
            &Method::DELETE,
            "test-repo",
            "latest",
            request(Method::DELETE, "latest"),
        )
        .await
        .unwrap_or_else(|e| e.into_response());
        // Tag deletion (non-digest reference) is not allowed through this endpoint
        assert_eq!(
            response.status(),
            axum::http::StatusCode::METHOD_NOT_ALLOWED
        );
    }

    #[tokio::test]
    async fn delete_nonexistent_digest_returns_accepted() {
        let state = test_state();
        let response = dispatch(
            state,
            &Method::DELETE,
            "test-repo",
            "sha256:00000000000000000000000000000000000000000000000000000000000000ff",
            request(
                Method::DELETE,
                "sha256:00000000000000000000000000000000000000000000000000000000000000ff",
            ),
        )
        .await
        .unwrap_or_else(|e| e.into_response());
        // DELETE is idempotent
        assert_eq!(response.status(), axum::http::StatusCode::ACCEPTED);
    }
}

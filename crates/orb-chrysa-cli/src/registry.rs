use std::collections::HashMap;
use std::time::Duration;

use aioduct::{StatusCode, TokioClient};
use bytes::Bytes;
use http::header::WWW_AUTHENTICATE;
use thiserror::Error;
use tokio::sync::RwLock;

#[derive(Debug, Error)]
pub enum RegistryError {
    #[error("{0}")]
    Http(String),
    #[error("{0}")]
    Protocol(String),
    #[error("{0}")]
    Json(#[from] serde_json::Error),
    #[error("{context}: {source}")]
    WithContext {
        context: String,
        source: Box<RegistryError>,
    },
}

impl RegistryError {
    pub fn context(self, ctx: impl Into<String>) -> Self {
        RegistryError::WithContext {
            context: ctx.into(),
            source: Box::new(self),
        }
    }
}

impl From<aioduct::SendError> for RegistryError {
    fn from(e: aioduct::SendError) -> Self {
        RegistryError::Http(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, RegistryError>;

const MANIFEST_ACCEPT: &str = "\
    application/vnd.oci.image.manifest.v1+json, \
    application/vnd.oci.image.index.v1+json, \
    application/vnd.docker.distribution.manifest.v2+json, \
    application/vnd.docker.distribution.manifest.list.v2+json, \
    */*";

const MAX_RETRIES: u32 = 5;
const INITIAL_BACKOFF_MS: u64 = 200;
pub const CHUNK_SIZE: usize = 5 * 1024 * 1024; // 5 MB

#[derive(Debug, Clone)]
pub struct ImageRef {
    pub registry: String,
    pub repository: String,
    pub reference: Option<String>,
    pub scheme: String,
}

impl ImageRef {
    pub fn parse(s: &str, plain_http: bool) -> Result<Self> {
        let (host, remainder) = if let Some(pos) = s.find('/') {
            let first = &s[..pos];
            if first.contains('.') || first.contains(':') || first == "localhost" {
                (first.to_string(), &s[pos + 1..])
            } else {
                ("docker.io".to_string(), s)
            }
        } else {
            ("docker.io".to_string(), s)
        };

        let (repo, reference) = if let Some(at_pos) = remainder.find('@') {
            (
                remainder[..at_pos].to_string(),
                Some(remainder[at_pos + 1..].to_string()),
            )
        } else {
            let last_slash = remainder.rfind('/').unwrap_or(0);
            if let Some(colon_pos) = remainder[last_slash..].rfind(':') {
                let abs_colon = last_slash + colon_pos;
                (
                    remainder[..abs_colon].to_string(),
                    Some(remainder[abs_colon + 1..].to_string()),
                )
            } else {
                (remainder.to_string(), None)
            }
        };

        let registry = if host == "docker.io" {
            "registry-1.docker.io".to_string()
        } else {
            host.clone()
        };

        let repository =
            if (host == "docker.io" || host == "registry-1.docker.io") && !repo.contains('/') {
                format!("library/{}", repo)
            } else {
                repo
            };

        let scheme = if plain_http {
            "http".to_string()
        } else {
            let hostname = registry.split(':').next().unwrap_or(&registry);
            if hostname == "localhost" || hostname == "127.0.0.1" || hostname == "[::1]" {
                "http".to_string()
            } else {
                "https".to_string()
            }
        };

        Ok(Self {
            registry,
            repository,
            reference,
            scheme,
        })
    }

    pub fn base_url(&self) -> String {
        format!("{}://{}", self.scheme, self.registry)
    }

    pub fn display(&self) -> String {
        match &self.reference {
            Some(r) if r.contains(':') => {
                format!("{}/{}@{}", self.registry, self.repository, r)
            }
            Some(r) => format!("{}/{}:{}", self.registry, self.repository, r),
            None => format!("{}/{}", self.registry, self.repository),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ManifestData {
    pub body: Vec<u8>,
    pub content_type: String,
    pub digest: String,
}

#[derive(Debug, Clone)]
pub struct BlobDescriptor {
    pub digest: String,
    pub size: u64,
}

pub struct RegistryClient {
    http: TokioClient,
    tokens: RwLock<HashMap<String, String>>,
}

struct BearerChallenge {
    realm: String,
    service: Option<String>,
}

fn parse_bearer_challenge(header: &str) -> Result<BearerChallenge> {
    let header = header.strip_prefix("Bearer ").unwrap_or(header);
    let mut realm = None;
    let mut service = None;

    let mut remaining = header;
    while !remaining.is_empty() {
        remaining = remaining.trim_start_matches([',', ' ']);
        let Some((key, rest)) = remaining.split_once('=') else {
            break;
        };
        let key = key.trim();

        let (value, rest) = if let Some(rest) = rest.strip_prefix('"') {
            let end = rest.find('"').unwrap_or(rest.len());
            (&rest[..end], rest.get(end + 1..).unwrap_or(""))
        } else {
            let end = rest.find(',').unwrap_or(rest.len());
            (&rest[..end], &rest[end..])
        };

        match key {
            "realm" => realm = Some(value.to_string()),
            "service" => service = Some(value.to_string()),
            _ => {}
        }
        remaining = rest;
    }

    Ok(BearerChallenge {
        realm: realm
            .ok_or_else(|| RegistryError::Protocol("no realm in Bearer challenge".into()))?,
        service,
    })
}

fn is_retryable(status: StatusCode) -> bool {
    status == StatusCode::TOO_MANY_REQUESTS
        || status == StatusCode::INTERNAL_SERVER_ERROR
        || status == StatusCode::BAD_GATEWAY
        || status == StatusCode::SERVICE_UNAVAILABLE
        || status == StatusCode::GATEWAY_TIMEOUT
        || status == StatusCode::REQUEST_TIMEOUT
}

fn is_retryable_err(e: &aioduct::SendError) -> bool {
    e.is_connect() || e.is_timeout()
}

async fn backoff(attempt: u32) {
    let ms = INITIAL_BACKOFF_MS * 2u64.pow(attempt);
    tokio::time::sleep(Duration::from_millis(ms)).await;
}

impl RegistryClient {
    pub fn new() -> Self {
        let http = TokioClient::builder()
            .user_agent("orb-chrysa-cli/0.1")
            .timeout(Duration::from_secs(300))
            .connect_timeout(Duration::from_secs(30))
            .build()
            .expect("failed to create HTTP client");

        Self {
            http,
            tokens: RwLock::new(HashMap::new()),
        }
    }

    fn cache_key(image: &ImageRef, actions: &str) -> String {
        format!(
            "{}|repository:{}:{}",
            image.registry, image.repository, actions
        )
    }

    pub async fn ensure_auth(&self, image: &ImageRef, actions: &str) -> Result<()> {
        let url = format!("{}/v2/", image.base_url());
        let resp = self
            .http
            .get(&url)
            .map_err(|e| RegistryError::Http(e.to_string()))?
            .send()
            .await?;

        if resp.status() != StatusCode::UNAUTHORIZED {
            return Ok(());
        }

        let www_auth = resp
            .headers()
            .get(WWW_AUTHENTICATE)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| {
                RegistryError::Protocol(format!(
                    "401 without WWW-Authenticate from {}",
                    image.registry
                ))
            })?
            .to_string();

        let scope = format!("repository:{}:{}", image.repository, actions);
        let token = self.fetch_token(&www_auth, &scope).await?;
        let key = Self::cache_key(image, actions);
        self.tokens.write().await.insert(key, token);
        Ok(())
    }

    async fn refresh_auth(&self, image: &ImageRef, actions: &str) -> Result<()> {
        let key = Self::cache_key(image, actions);
        self.tokens.write().await.remove(&key);
        self.ensure_auth(image, actions).await
    }

    async fn fetch_token(&self, www_auth: &str, scope: &str) -> Result<String> {
        let challenge = parse_bearer_challenge(www_auth)?;

        let url = format!("{}?scope={}", challenge.realm, scope);
        let mut full_url = url;
        if let Some(service) = &challenge.service {
            full_url.push_str(&format!("&service={}", service));
        }

        let resp = self
            .http
            .get(&full_url)
            .map_err(|e| RegistryError::Http(e.to_string()))?
            .send()
            .await?
            .error_for_status()
            .map_err(|e| RegistryError::Http(format!("token fetch: {}", e)))?;
        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| RegistryError::Http(format!("token json: {}", e)))?;

        body.get("token")
            .or_else(|| body.get("access_token"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| RegistryError::Protocol("no token in auth response".into()))
    }

    async fn apply_auth<'a>(
        &self,
        builder: aioduct::RequestBuilderSend<
            'a,
            aioduct::runtime::tokio_rt::TokioRuntime,
            aioduct::runtime::tokio_rt::TcpConnector,
        >,
        key: &str,
    ) -> aioduct::RequestBuilderSend<
        'a,
        aioduct::runtime::tokio_rt::TokioRuntime,
        aioduct::runtime::tokio_rt::TcpConnector,
    > {
        if let Some(token) = self.tokens.read().await.get(key) {
            builder.bearer_auth(token)
        } else {
            builder
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn retry(
        &self,
        image: &ImageRef,
        actions: &str,
        method: http::Method,
        url: &str,
        accept: Option<&str>,
        body: Option<Bytes>,
        content_type: Option<&str>,
    ) -> Result<aioduct::Response> {
        let key = Self::cache_key(image, actions);
        let mut last_err: Option<RegistryError> = None;

        for attempt in 0..=MAX_RETRIES {
            if attempt > 0 {
                backoff(attempt - 1).await;
            }

            let mut builder = self
                .http
                .request(method.clone(), url)
                .map_err(|e| RegistryError::Http(e.to_string()))?;
            if let Some(accept_val) = accept {
                builder = builder
                    .header_str("accept", accept_val)
                    .map_err(|e| RegistryError::Http(e.to_string()))?;
            }
            if let Some(ct) = content_type {
                builder = builder
                    .header_str("content-type", ct)
                    .map_err(|e| RegistryError::Http(e.to_string()))?;
            }
            if let Some(ref b) = body {
                builder = builder.body(b.clone());
            }
            let builder = self.apply_auth(builder, &key).await;

            let resp = match builder.send().await {
                Ok(r) => r,
                Err(e) => {
                    if is_retryable_err(&e) && attempt < MAX_RETRIES {
                        eprintln!("    retry {}/{}: {}", attempt + 1, MAX_RETRIES, e);
                        last_err = Some(RegistryError::Http(e.to_string()));
                        continue;
                    }
                    return Err(RegistryError::Http(e.to_string()));
                }
            };

            if resp.status() == StatusCode::UNAUTHORIZED && attempt < MAX_RETRIES {
                let _ = self.refresh_auth(image, actions).await;
                last_err = Some(RegistryError::Http("401 Unauthorized".into()));
                continue;
            }

            if is_retryable(resp.status()) && attempt < MAX_RETRIES {
                eprintln!(
                    "    retry {}/{}: HTTP {}",
                    attempt + 1,
                    MAX_RETRIES,
                    resp.status()
                );
                last_err = Some(RegistryError::Http(format!("HTTP {}", resp.status())));
                continue;
            }

            return Ok(resp);
        }

        Err(last_err.unwrap_or_else(|| RegistryError::Http("max retries exceeded".into())))
    }

    fn resolve_location(&self, base_url: &str, location: &str) -> String {
        if location.starts_with("http://") || location.starts_with("https://") {
            location.to_string()
        } else {
            let parsed: http::Uri = base_url.parse().expect("valid base URL");
            let authority = parsed.authority().map(|a| a.as_str()).unwrap_or("");
            let scheme = parsed.scheme_str().unwrap_or("http");
            format!("{}://{}{}", scheme, authority, location)
        }
    }

    // ── Manifest operations ──

    pub async fn list_tags(&self, image: &ImageRef) -> Result<Vec<String>> {
        let mut tags = Vec::new();
        let mut last: Option<String> = None;

        loop {
            let mut url = format!(
                "{}/v2/{}/tags/list?n=1000",
                image.base_url(),
                image.repository
            );
            if let Some(ref l) = last {
                url.push_str(&format!("&last={}", l));
            }

            let resp = self
                .retry(
                    image,
                    "pull",
                    http::Method::GET,
                    &url,
                    Some("application/json"),
                    None,
                    None,
                )
                .await?
                .error_for_status()
                .map_err(|e| RegistryError::Http(format!("list tags: {}", e)))?;

            let body: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| RegistryError::Http(format!("list tags json: {}", e)))?;
            let page: Vec<String> = body
                .get("tags")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();

            if page.is_empty() {
                break;
            }
            last = page.last().cloned();
            tags.extend(page);
        }

        Ok(tags)
    }

    pub async fn head_manifest(
        &self,
        image: &ImageRef,
        reference: &str,
    ) -> Result<Option<(String, String)>> {
        let url = format!(
            "{}/v2/{}/manifests/{}",
            image.base_url(),
            image.repository,
            reference
        );
        let resp = self
            .retry(
                image,
                "pull",
                http::Method::HEAD,
                &url,
                Some(MANIFEST_ACCEPT),
                None,
                None,
            )
            .await?;

        match resp.status() {
            StatusCode::OK => {
                let digest = resp
                    .headers()
                    .get("docker-content-digest")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("")
                    .to_string();
                let ct = resp
                    .headers()
                    .get("content-type")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("application/octet-stream")
                    .to_string();
                Ok(Some((digest, ct)))
            }
            StatusCode::NOT_FOUND => Ok(None),
            s => Err(RegistryError::Http(format!(
                "HEAD manifest {} failed: {}",
                reference, s
            ))),
        }
    }

    pub async fn get_manifest(&self, image: &ImageRef, reference: &str) -> Result<ManifestData> {
        let url = format!(
            "{}/v2/{}/manifests/{}",
            image.base_url(),
            image.repository,
            reference
        );
        let resp = self
            .retry(
                image,
                "pull",
                http::Method::GET,
                &url,
                Some(MANIFEST_ACCEPT),
                None,
                None,
            )
            .await?
            .error_for_status()
            .map_err(|e| RegistryError::Http(format!("GET manifest {}: {}", reference, e)))?;

        let digest = resp
            .headers()
            .get("docker-content-digest")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let content_type = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("application/vnd.oci.image.manifest.v1+json")
            .to_string();
        let body = resp
            .bytes()
            .await
            .map_err(|e| RegistryError::Http(format!("GET manifest body: {}", e)))?
            .to_vec();

        Ok(ManifestData {
            body,
            content_type,
            digest,
        })
    }

    pub async fn put_manifest(
        &self,
        image: &ImageRef,
        reference: &str,
        body: &[u8],
        content_type: &str,
    ) -> Result<()> {
        let url = format!(
            "{}/v2/{}/manifests/{}",
            image.base_url(),
            image.repository,
            reference
        );
        let resp = self
            .retry(
                image,
                "push,pull",
                http::Method::PUT,
                &url,
                None,
                Some(Bytes::from(body.to_vec())),
                Some(content_type),
            )
            .await?;

        if resp.status() != StatusCode::CREATED {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(RegistryError::Http(format!(
                "PUT manifest {} failed: {} {}",
                reference, status, text
            )));
        }
        Ok(())
    }

    // ── Blob operations ──

    pub async fn head_blob(&self, image: &ImageRef, digest: &str) -> Result<bool> {
        let url = format!(
            "{}/v2/{}/blobs/{}",
            image.base_url(),
            image.repository,
            digest
        );
        let resp = self
            .retry(image, "pull", http::Method::HEAD, &url, None, None, None)
            .await?;
        Ok(resp.status() == StatusCode::OK)
    }

    pub async fn get_blob_stream(
        &self,
        image: &ImageRef,
        digest: &str,
        offset: u64,
    ) -> Result<aioduct::Response> {
        let url = format!(
            "{}/v2/{}/blobs/{}",
            image.base_url(),
            image.repository,
            digest
        );
        if offset > 0 {
            let key = Self::cache_key(image, "pull");
            let builder = self
                .http
                .get(&url)
                .map_err(|e| RegistryError::Http(e.to_string()))?
                .header_str("range", &format!("bytes={}-", offset))
                .map_err(|e| RegistryError::Http(e.to_string()))?;
            let builder = self.apply_auth(builder, &key).await;
            builder
                .send()
                .await
                .map_err(|e| RegistryError::Http(e.to_string()))?
                .error_for_status()
                .map_err(|e| {
                    RegistryError::Http(format!(
                        "GET blob {} from offset {}: {}",
                        digest, offset, e
                    ))
                })
        } else {
            self.retry(image, "pull", http::Method::GET, &url, None, None, None)
                .await?
                .error_for_status()
                .map_err(|e| RegistryError::Http(format!("GET blob {}: {}", digest, e)))
        }
    }

    pub async fn start_upload(&self, image: &ImageRef) -> Result<String> {
        let url = format!(
            "{}/v2/{}/blobs/uploads/",
            image.base_url(),
            image.repository
        );
        let resp = self
            .retry(
                image,
                "push,pull",
                http::Method::POST,
                &url,
                None,
                None,
                None,
            )
            .await?;

        if resp.status() != StatusCode::ACCEPTED {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(RegistryError::Http(format!(
                "start upload failed: {} {}",
                status, text
            )));
        }

        let location = resp
            .headers()
            .get("location")
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| {
                RegistryError::Protocol("no Location header in upload start response".into())
            })?
            .to_string();
        Ok(self.resolve_location(&image.base_url(), &location))
    }

    pub async fn patch_chunk(
        &self,
        image: &ImageRef,
        upload_url: &str,
        chunk: Bytes,
        offset: u64,
    ) -> Result<(String, u64)> {
        let chunk_len = chunk.len() as u64;
        let range_header = format!("{}-{}", offset, offset + chunk_len - 1);
        let key = Self::cache_key(image, "push,pull");

        let builder = self
            .http
            .request(http::Method::PATCH, upload_url)
            .map_err(|e| RegistryError::Http(e.to_string()))?
            .header_str("content-type", "application/octet-stream")
            .map_err(|e| RegistryError::Http(e.to_string()))?
            .header_str("content-range", &range_header)
            .map_err(|e| RegistryError::Http(e.to_string()))?
            .header_str("content-length", &chunk_len.to_string())
            .map_err(|e| RegistryError::Http(e.to_string()))?
            .body(chunk);
        let builder = self.apply_auth(builder, &key).await;

        let resp = builder
            .send()
            .await
            .map_err(|e| RegistryError::Http(e.to_string()))?;

        if resp.status() != StatusCode::ACCEPTED {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(RegistryError::Http(format!(
                "PATCH chunk failed at offset {}: {} {}",
                offset, status, text
            )));
        }

        let new_location = resp
            .headers()
            .get("location")
            .and_then(|v| v.to_str().ok())
            .map(|l| self.resolve_location(&image.base_url(), l))
            .unwrap_or_else(|| upload_url.to_string());

        let new_offset = resp
            .headers()
            .get("range")
            .and_then(|v| v.to_str().ok())
            .and_then(|r| r.strip_prefix("0-"))
            .and_then(|end| end.parse::<u64>().ok())
            .map(|end| end + 1)
            .unwrap_or(offset + chunk_len);

        Ok((new_location, new_offset))
    }

    pub async fn complete_upload(
        &self,
        image: &ImageRef,
        upload_url: &str,
        digest: &str,
    ) -> Result<()> {
        let url = if upload_url.contains('?') {
            format!("{}&digest={}", upload_url, digest)
        } else {
            format!("{}?digest={}", upload_url, digest)
        };
        let key = Self::cache_key(image, "push,pull");

        let builder = self
            .http
            .request(http::Method::PUT, &url)
            .map_err(|e| RegistryError::Http(e.to_string()))?
            .header_str("content-length", "0")
            .map_err(|e| RegistryError::Http(e.to_string()))?;
        let builder = self.apply_auth(builder, &key).await;

        let resp = builder
            .send()
            .await
            .map_err(|e| RegistryError::Http(e.to_string()))?;

        if resp.status() != StatusCode::CREATED {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(RegistryError::Http(format!(
                "complete upload failed: {} {}",
                status, text
            )));
        }
        Ok(())
    }
}

pub fn is_index_manifest(content_type: &str) -> bool {
    matches!(
        content_type,
        "application/vnd.oci.image.index.v1+json"
            | "application/vnd.docker.distribution.manifest.list.v2+json"
    )
}

pub fn extract_blob_descriptors(manifest: &serde_json::Value) -> Vec<BlobDescriptor> {
    let mut blobs = Vec::new();

    if let Some(config) = manifest.get("config")
        && let (Some(digest), Some(size)) = (
            config.get("digest").and_then(|d| d.as_str()),
            config.get("size").and_then(|s| s.as_u64()),
        )
    {
        blobs.push(BlobDescriptor {
            digest: digest.to_string(),
            size,
        });
    }

    if let Some(layers) = manifest.get("layers").and_then(|l| l.as_array()) {
        for layer in layers {
            if let (Some(digest), Some(size)) = (
                layer.get("digest").and_then(|d| d.as_str()),
                layer.get("size").and_then(|s| s.as_u64()),
            ) {
                blobs.push(BlobDescriptor {
                    digest: digest.to_string(),
                    size,
                });
            }
        }
    }

    blobs
}

pub fn extract_child_manifests(index: &serde_json::Value) -> Vec<BlobDescriptor> {
    let mut children = Vec::new();
    if let Some(manifests) = index.get("manifests").and_then(|m| m.as_array()) {
        for m in manifests {
            if let (Some(digest), Some(size)) = (
                m.get("digest").and_then(|d| d.as_str()),
                m.get("size").and_then(|s| s.as_u64()),
            ) {
                children.push(BlobDescriptor {
                    digest: digest.to_string(),
                    size,
                });
            }
        }
    }
    children
}

pub fn format_size(bytes: u64) -> String {
    if bytes >= 1024 * 1024 * 1024 {
        format!("{:.1} GiB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    } else if bytes >= 1024 * 1024 {
        format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    } else {
        format!("{} B", bytes)
    }
}

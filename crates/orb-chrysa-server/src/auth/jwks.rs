use std::time::{SystemTime, UNIX_EPOCH};

use aws_sdk_s3::Client;
use aws_sdk_s3::primitives::ByteStream;
use serde::{Deserialize, Serialize};

use crate::config::S3Config;
use crate::error::OrbChrysaError;

#[derive(Debug, Clone)]
pub struct JwksMetrics {
    pub key_count: usize,
    pub cache_age_seconds: u64,
    pub stale_mode: bool,
    pub refresh_failures: u64,
    pub endpoint: Option<String>,
}

pub struct JwksCache {
    keys: Vec<(String, jsonwebtoken::DecodingKey)>,
    fetched_at_unix: Option<u64>,
    stale_mode: bool,
    endpoint: Option<String>,
    refresh_failures: u64,
}

impl JwksCache {
    pub fn empty() -> Self {
        Self {
            keys: Vec::new(),
            fetched_at_unix: None,
            stale_mode: false,
            endpoint: None,
            refresh_failures: 0,
        }
    }

    pub fn find_key(&self, kid: &str) -> Option<&jsonwebtoken::DecodingKey> {
        self.keys.iter().find(|(k, _)| k == kid).map(|(_, key)| key)
    }

    pub fn refresh_from_value(
        &mut self,
        jwks_value: &serde_json::Value,
        fetched_at_unix: u64,
        stale_mode: bool,
        endpoint: impl Into<String>,
    ) -> Result<usize, OrbChrysaError> {
        let jwks: jsonwebtoken::jwk::JwkSet = serde_json::from_value(jwks_value.clone())
            .map_err(|e| OrbChrysaError::Internal(format!("JWKS parse failed: {}", e)))?;

        let mut keys = Vec::new();
        for jwk in &jwks.keys {
            let kid = jwk.common.key_id.clone().unwrap_or_default();
            let decoding_key = jsonwebtoken::DecodingKey::from_jwk(jwk)
                .map_err(|e| OrbChrysaError::Internal(format!("JWK conversion failed: {}", e)))?;
            keys.push((kid, decoding_key));
        }

        self.keys = keys;
        self.fetched_at_unix = Some(fetched_at_unix);
        self.stale_mode = stale_mode;
        self.endpoint = Some(endpoint.into());
        Ok(self.keys.len())
    }

    pub fn record_refresh_failure(&mut self) {
        self.refresh_failures = self.refresh_failures.saturating_add(1);
    }

    pub fn metrics(&self) -> JwksMetrics {
        let now = now_unix();
        let cache_age_seconds = self
            .fetched_at_unix
            .map(|fetched| now.saturating_sub(fetched))
            .unwrap_or(0);
        JwksMetrics {
            key_count: self.keys.len(),
            cache_age_seconds,
            stale_mode: self.stale_mode,
            refresh_failures: self.refresh_failures,
            endpoint: self.endpoint.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedJwksDocument {
    pub version: u8,
    pub issuer_url: String,
    pub issuer_internal_url: String,
    pub discovery: serde_json::Value,
    pub jwks: serde_json::Value,
    pub fetched_at_unix: u64,
}

impl CachedJwksDocument {
    pub fn new(
        issuer_url: String,
        issuer_internal_url: String,
        discovery: serde_json::Value,
        jwks: serde_json::Value,
        fetched_at_unix: u64,
    ) -> Self {
        Self {
            version: 1,
            issuer_url,
            issuer_internal_url,
            discovery,
            jwks,
            fetched_at_unix,
        }
    }

    pub fn age_seconds(&self) -> u64 {
        now_unix().saturating_sub(self.fetched_at_unix)
    }

    pub fn within_stale_window(&self, max_stale_seconds: u64) -> bool {
        self.age_seconds() <= max_stale_seconds
    }
}

pub struct JwksS3Cache {
    client: Client,
    bucket: String,
    key: String,
}

impl JwksS3Cache {
    pub async fn new(config: &S3Config, key: impl Into<String>) -> Self {
        let creds = aws_credential_types::Credentials::new(
            &config.access_key,
            &config.secret_key,
            None,
            None,
            "orb-chrysa",
        );
        let sdk_config = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .endpoint_url(&config.endpoint)
            .credentials_provider(creds)
            .region(aws_types::region::Region::new(config.region.clone()))
            .load()
            .await;

        let s3_config = aws_sdk_s3::config::Builder::from(&sdk_config)
            .force_path_style(config.path_style)
            .build();

        Self {
            client: Client::from_conf(s3_config),
            bucket: config.bucket.clone(),
            key: key.into(),
        }
    }

    pub async fn store(&self, document: &CachedJwksDocument) -> Result<(), OrbChrysaError> {
        let bytes = serde_json::to_vec(document)
            .map_err(|e| OrbChrysaError::Serialization(format!("JWKS cache encode: {e}")))?;
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(&self.key)
            .content_type("application/json")
            .body(ByteStream::from(bytes))
            .send()
            .await
            .map_err(|e| OrbChrysaError::S3(format!("JWKS cache upload failed: {e}")))?;
        tracing::info!(key = %self.key, "stored last-good JWKS cache in S3");
        Ok(())
    }

    pub async fn load(&self) -> Result<Option<CachedJwksDocument>, OrbChrysaError> {
        let result = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(&self.key)
            .send()
            .await;

        let output = match result {
            Ok(output) => output,
            Err(sdk_err) => {
                let service_err = sdk_err.into_service_error();
                if service_err.is_no_such_key() {
                    return Ok(None);
                }
                let meta = service_err.meta();
                if meta.code() == Some("NoSuchBucket")
                    || meta.code() == Some("NotFound")
                    || meta.code() == Some("404")
                {
                    return Ok(None);
                }
                return Err(OrbChrysaError::S3(format!(
                    "JWKS cache download failed: {service_err}"
                )));
            }
        };

        let bytes = output
            .body
            .collect()
            .await
            .map_err(|e| OrbChrysaError::S3(format!("JWKS cache body read failed: {e}")))?
            .to_vec();
        let document = serde_json::from_slice(&bytes)
            .map_err(|e| OrbChrysaError::Serialization(format!("JWKS cache decode: {e}")))?;
        Ok(Some(document))
    }
}

pub async fn fetch_jwks(
    jwks_uri: &str,
    tls_insecure: bool,
) -> Result<serde_json::Value, OrbChrysaError> {
    let mut client_builder = aioduct::TokioClient::builder()
        .timeout(std::time::Duration::from_secs(10))
        .connect_timeout(std::time::Duration::from_secs(5));
    if tls_insecure {
        client_builder = client_builder.danger_accept_invalid_certs();
    }
    let client = client_builder
        .build()
        .map_err(|e| OrbChrysaError::Internal(format!("HTTP client build failed: {}", e)))?;

    let response = client
        .request(http::Method::GET, jwks_uri)
        .map_err(|e| OrbChrysaError::Internal(format!("JWKS request build failed: {}", e)))?
        .send()
        .await
        .map_err(|e| OrbChrysaError::Internal(format!("JWKS fetch failed: {}", e)))?;

    let body = response
        .text()
        .await
        .map_err(|e| OrbChrysaError::Internal(format!("JWKS read failed: {}", e)))?;

    serde_json::from_str(&body)
        .map_err(|e| OrbChrysaError::Internal(format!("JWKS parse failed: {}", e)))
}

pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::{CachedJwksDocument, JwksCache, now_unix};

    fn hs256_jwks(kid: &str) -> serde_json::Value {
        serde_json::json!({
            "keys": [
                {
                    "kty": "oct",
                    "k": "c2VjcmV0",
                    "kid": kid,
                    "alg": "HS256"
                }
            ]
        })
    }

    #[test]
    fn cached_jwks_document_enforces_stale_window() {
        let fresh = CachedJwksDocument::new(
            "https://idp.example.test".to_string(),
            "https://idp-internal.example.test".to_string(),
            serde_json::json!({}),
            serde_json::json!({"keys":[]}),
            now_unix().saturating_sub(60),
        );
        assert!(fresh.within_stale_window(24 * 60 * 60));

        let old = CachedJwksDocument {
            fetched_at_unix: now_unix().saturating_sub(25 * 60 * 60),
            ..fresh
        };
        assert!(!old.within_stale_window(24 * 60 * 60));
    }

    #[test]
    fn jwks_cache_preserves_keys_when_refresh_parse_fails() {
        let mut cache = JwksCache::empty();
        let fetched_at = now_unix();
        let key_count = cache
            .refresh_from_value(&hs256_jwks("kid-a"), fetched_at, false, "https://jwks")
            .expect("initial jwks should parse");
        assert_eq!(key_count, 1);
        assert!(cache.find_key("kid-a").is_some());

        assert!(
            cache
                .refresh_from_value(
                    &serde_json::json!({"not_keys": []}),
                    fetched_at + 1,
                    false,
                    "https://jwks"
                )
                .is_err()
        );
        assert!(cache.find_key("kid-a").is_some());
    }

    #[test]
    fn jwks_metrics_report_stale_mode_and_failures() {
        let mut cache = JwksCache::empty();
        let fetched_at = now_unix().saturating_sub(30);
        cache
            .refresh_from_value(&hs256_jwks("kid-a"), fetched_at, true, "s3:auth/jwks")
            .expect("jwks should parse");
        cache.record_refresh_failure();

        let metrics = cache.metrics();
        assert_eq!(metrics.key_count, 1);
        assert!(metrics.cache_age_seconds >= 30);
        assert!(metrics.stale_mode);
        assert_eq!(metrics.refresh_failures, 1);
        assert_eq!(metrics.endpoint.as_deref(), Some("s3:auth/jwks"));
    }
}

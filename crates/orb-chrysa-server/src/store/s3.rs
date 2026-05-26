use async_trait::async_trait;
use aws_sdk_s3::Client;
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::types::{CompletedMultipartUpload, CompletedPart};
use bytes::{Bytes, BytesMut};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest as ShaDigest, Sha256};

use crate::config::S3Config;
use crate::error::OrbChrysaError;
use crate::oci::digest::Digest;
use crate::store::blob::{BlobInfo, BlobStore, BlobStream};

const MIN_PART_SIZE: usize = 5 * 1024 * 1024; // 5MB minimum for S3 multipart parts

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StagedUploadState {
    parts: Vec<StagedUploadPart>,
    next_part_number: i32,
    total_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StagedUploadPart {
    number: i32,
    key: String,
    size: u64,
}

pub struct S3BlobStore {
    client: Client,
    bucket: String,
    redirect_enabled: bool,
    presign_client: Option<Client>,
    presign_expires_secs: u64,
}

impl S3BlobStore {
    pub async fn new(config: &S3Config) -> Self {
        let client = Self::build_client(config, &config.endpoint).await;
        let presign_client = if config.redirect.enabled {
            Some(Self::build_client(config, &config.redirect.public_endpoint).await)
        } else {
            None
        };

        Self {
            client,
            bucket: config.bucket.clone(),
            redirect_enabled: config.redirect.enabled,
            presign_client,
            presign_expires_secs: config.redirect.expires_secs,
        }
    }

    async fn build_client(config: &S3Config, endpoint: &str) -> Client {
        let creds = aws_credential_types::Credentials::new(
            &config.access_key,
            &config.secret_key,
            None,
            None,
            "orb-chrysa",
        );
        let sdk_config = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .endpoint_url(endpoint)
            .credentials_provider(creds)
            .region(aws_types::region::Region::new(config.region.clone()))
            .load()
            .await;

        let s3_config = aws_sdk_s3::config::Builder::from(&sdk_config)
            .force_path_style(config.path_style)
            .build();

        Client::from_conf(s3_config)
    }

    pub fn client(&self) -> &Client {
        &self.client
    }

    pub fn bucket(&self) -> &str {
        &self.bucket
    }

    fn upload_state_key(session_id: &str) -> String {
        format!("uploads/{}/blob-state.json", session_id)
    }

    fn upload_part_key(session_id: &str, part_number: i32) -> String {
        format!("uploads/{}/parts/{part_number:020}", session_id)
    }

    async fn write_upload_state(
        &self,
        session_id: &str,
        state: &StagedUploadState,
    ) -> Result<(), OrbChrysaError> {
        let bytes = serde_json::to_vec(state)
            .map_err(|e| OrbChrysaError::S3(format!("encode upload state: {e}")))?;
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(Self::upload_state_key(session_id))
            .body(ByteStream::from(bytes))
            .send()
            .await
            .map_err(|e| OrbChrysaError::S3(e.to_string()))?;
        Ok(())
    }

    async fn read_upload_state(
        &self,
        session_id: &str,
    ) -> Result<Option<StagedUploadState>, OrbChrysaError> {
        let result = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(Self::upload_state_key(session_id))
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
        let state = serde_json::from_slice(&bytes)
            .map_err(|e| OrbChrysaError::S3(format!("decode upload state: {e}")))?;
        Ok(Some(state))
    }

    async fn delete_staged_upload(&self, state: &StagedUploadState) -> Result<(), OrbChrysaError> {
        for part in &state.parts {
            self.client
                .delete_object()
                .bucket(&self.bucket)
                .key(&part.key)
                .send()
                .await
                .map_err(|e| OrbChrysaError::S3(e.to_string()))?;
        }
        Ok(())
    }

    async fn upload_final_part(
        &self,
        key: &str,
        upload_id: &str,
        part_number: i32,
        part_data: Bytes,
    ) -> Result<CompletedPart, OrbChrysaError> {
        let resp = self
            .client
            .upload_part()
            .bucket(&self.bucket)
            .key(key)
            .upload_id(upload_id)
            .part_number(part_number)
            .body(ByteStream::from(part_data))
            .send()
            .await
            .map_err(|e| OrbChrysaError::S3(e.to_string()))?;

        Ok(CompletedPart::builder()
            .part_number(part_number)
            .set_e_tag(resp.e_tag().map(|s| s.to_string()))
            .build())
    }
}

#[async_trait]
impl BlobStore for S3BlobStore {
    async fn health_check(&self) -> Result<(), OrbChrysaError> {
        self.client
            .head_bucket()
            .bucket(&self.bucket)
            .send()
            .await
            .map_err(|e| OrbChrysaError::S3(e.to_string()))?;
        Ok(())
    }

    async fn stat(&self, digest: &Digest) -> Result<BlobInfo, OrbChrysaError> {
        let resp = self
            .client
            .head_object()
            .bucket(&self.bucket)
            .key(digest.s3_key())
            .send()
            .await
            .map_err(|e| {
                use aws_sdk_s3::operation::head_object::HeadObjectError;
                if e.as_service_error()
                    .is_some_and(|se| matches!(se, HeadObjectError::NotFound(_)))
                {
                    OrbChrysaError::BlobUnknown(digest.to_string())
                } else {
                    OrbChrysaError::S3(e.to_string())
                }
            })?;

        Ok(BlobInfo {
            size: resp.content_length().unwrap_or(0) as u64,
        })
    }

    async fn get(&self, digest: &Digest) -> Result<BlobStream, OrbChrysaError> {
        let resp = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(digest.s3_key())
            .send()
            .await
            .map_err(|e| OrbChrysaError::S3(e.to_string()))?;

        Ok(BlobStream::S3(Box::new(resp)))
    }

    async fn get_range(
        &self,
        digest: &Digest,
        start: u64,
        end: u64,
    ) -> Result<BlobStream, OrbChrysaError> {
        let range = format!("bytes={}-{}", start, end);
        let resp = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(digest.s3_key())
            .range(range)
            .send()
            .await
            .map_err(|e| OrbChrysaError::S3(e.to_string()))?;

        Ok(BlobStream::S3(Box::new(resp)))
    }

    async fn start_upload(&self, session_id: &str) -> Result<(), OrbChrysaError> {
        let state = StagedUploadState {
            parts: Vec::new(),
            next_part_number: 1,
            total_bytes: 0,
        };
        self.write_upload_state(session_id, &state).await
    }

    async fn push_chunk(&self, session_id: &str, data: Bytes) -> Result<u64, OrbChrysaError> {
        let mut state = self
            .read_upload_state(session_id)
            .await?
            .ok_or_else(|| OrbChrysaError::S3(format!("no upload session: {}", session_id)))?;

        if data.is_empty() {
            return Ok(state.total_bytes);
        }

        let part_number = state.next_part_number;
        let key = Self::upload_part_key(session_id, part_number);
        let size = data.len() as u64;
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(&key)
            .body(ByteStream::from(data))
            .send()
            .await
            .map_err(|e| OrbChrysaError::S3(e.to_string()))?;

        state.total_bytes += size;
        state.next_part_number += 1;
        state.parts.push(StagedUploadPart {
            number: part_number,
            key,
            size,
        });
        self.write_upload_state(session_id, &state).await?;
        Ok(state.total_bytes)
    }

    async fn complete_upload(
        &self,
        session_id: &str,
        expected_digest: &Digest,
    ) -> Result<(), OrbChrysaError> {
        let mut state = self
            .read_upload_state(session_id)
            .await?
            .ok_or_else(|| OrbChrysaError::S3(format!("no upload session: {}", session_id)))?;
        state.parts.sort_by_key(|part| part.number);

        let final_key = expected_digest.s3_key();
        let mut hasher = Sha256::new();
        let mut buffer = BytesMut::new();
        let mut upload_id: Option<String> = None;
        let mut uploaded_parts = Vec::new();
        let mut final_part_number = 0;

        for part in &state.parts {
            let output = self
                .client
                .get_object()
                .bucket(&self.bucket)
                .key(&part.key)
                .send()
                .await
                .map_err(|e| OrbChrysaError::S3(e.to_string()))?;
            let bytes = output
                .body
                .collect()
                .await
                .map_err(|e| OrbChrysaError::S3(e.to_string()))?
                .into_bytes();
            if bytes.len() as u64 != part.size {
                return Err(OrbChrysaError::S3(format!(
                    "staged upload part size changed: {}",
                    part.key
                )));
            }
            hasher.update(&bytes);
            buffer.extend_from_slice(&bytes);

            while buffer.len() >= MIN_PART_SIZE {
                if upload_id.is_none() {
                    let resp = self
                        .client
                        .create_multipart_upload()
                        .bucket(&self.bucket)
                        .key(&final_key)
                        .send()
                        .await
                        .map_err(|e| OrbChrysaError::S3(e.to_string()))?;
                    upload_id = Some(
                        resp.upload_id()
                            .ok_or_else(|| OrbChrysaError::S3("no upload_id returned".into()))?
                            .to_string(),
                    );
                }
                final_part_number += 1;
                let part_data = buffer.split_to(MIN_PART_SIZE).freeze();
                uploaded_parts.push(
                    self.upload_final_part(
                        &final_key,
                        upload_id.as_deref().expect("multipart upload id is set"),
                        final_part_number,
                        part_data,
                    )
                    .await?,
                );
            }
        }

        let hash = hasher.finalize();
        let actual_digest = Digest::from_sha256_bytes(&hash);
        if actual_digest.to_string() != expected_digest.to_string() {
            if let Some(upload_id) = upload_id {
                let _ = self
                    .client
                    .abort_multipart_upload()
                    .bucket(&self.bucket)
                    .key(&final_key)
                    .upload_id(upload_id)
                    .send()
                    .await;
            }
            return Err(OrbChrysaError::S3(format!(
                "digest mismatch: expected {}, got {}",
                expected_digest, actual_digest
            )));
        }

        if let Some(upload_id) = upload_id {
            if !buffer.is_empty() {
                final_part_number += 1;
                let part_data = buffer.split().freeze();
                uploaded_parts.push(
                    self.upload_final_part(&final_key, &upload_id, final_part_number, part_data)
                        .await?,
                );
            }

            let completed = CompletedMultipartUpload::builder()
                .set_parts(Some(uploaded_parts))
                .build();
            self.client
                .complete_multipart_upload()
                .bucket(&self.bucket)
                .key(&final_key)
                .upload_id(upload_id)
                .multipart_upload(completed)
                .send()
                .await
                .map_err(|e| OrbChrysaError::S3(e.to_string()))?;
        } else {
            self.client
                .put_object()
                .bucket(&self.bucket)
                .key(&final_key)
                .body(ByteStream::from(buffer.freeze()))
                .send()
                .await
                .map_err(|e| OrbChrysaError::S3(e.to_string()))?;
        }

        self.client
            .delete_object()
            .bucket(&self.bucket)
            .key(Self::upload_state_key(session_id))
            .send()
            .await
            .map_err(|e| OrbChrysaError::S3(e.to_string()))?;

        self.delete_staged_upload(&state).await?;

        Ok(())
    }

    async fn delete_upload(&self, session_id: &str) -> Result<(), OrbChrysaError> {
        if let Some(state) = self.read_upload_state(session_id).await? {
            self.delete_staged_upload(&state).await?;
        }
        self.client
            .delete_object()
            .bucket(&self.bucket)
            .key(Self::upload_state_key(session_id))
            .send()
            .await
            .map_err(|e| OrbChrysaError::S3(e.to_string()))?;

        Ok(())
    }

    fn redirect_enabled(&self) -> bool {
        self.redirect_enabled
    }

    async fn presigned_url(&self, digest: &Digest) -> Result<String, OrbChrysaError> {
        let presigning_config = aws_sdk_s3::presigning::PresigningConfig::expires_in(
            std::time::Duration::from_secs(self.presign_expires_secs),
        )
        .map_err(|e| OrbChrysaError::S3(e.to_string()))?;
        let client = self.presign_client.as_ref().unwrap_or(&self.client);

        let req = client
            .get_object()
            .bucket(&self.bucket)
            .key(digest.s3_key())
            .presigned(presigning_config)
            .await
            .map_err(|e| OrbChrysaError::S3(e.to_string()))?;

        Ok(req.uri().to_string())
    }

    async fn put_streaming(
        &self,
        digest: &Digest,
        mut stream: crate::store::blob::ByteStream,
    ) -> Result<(), OrbChrysaError> {
        let key = digest.s3_key();

        let resp = self
            .client
            .create_multipart_upload()
            .bucket(&self.bucket)
            .key(&key)
            .send()
            .await
            .map_err(|e| OrbChrysaError::S3(e.to_string()))?;

        let upload_id = resp
            .upload_id()
            .ok_or_else(|| OrbChrysaError::S3("no upload_id returned".into()))?
            .to_string();

        let mut parts: Vec<CompletedPart> = Vec::new();
        let mut part_number: i32 = 0;
        let mut buffer = BytesMut::new();
        let mut hasher = Sha256::new();

        let abort = |uid: &str, k: &str| {
            let client = self.client.clone();
            let bucket = self.bucket.clone();
            let uid = uid.to_string();
            let k = k.to_string();
            async move {
                let _ = client
                    .abort_multipart_upload()
                    .bucket(&bucket)
                    .key(&k)
                    .upload_id(&uid)
                    .send()
                    .await;
            }
        };

        while let Some(chunk_result) = stream.next().await {
            let chunk = match chunk_result {
                Ok(c) => c,
                Err(e) => {
                    abort(&upload_id, &key).await;
                    return Err(OrbChrysaError::S3(e.to_string()));
                }
            };
            hasher.update(&chunk);
            buffer.extend_from_slice(&chunk);

            while buffer.len() >= MIN_PART_SIZE {
                part_number += 1;
                let part_data = buffer.split_to(MIN_PART_SIZE).freeze();

                let resp = match self
                    .client
                    .upload_part()
                    .bucket(&self.bucket)
                    .key(&key)
                    .upload_id(&upload_id)
                    .part_number(part_number)
                    .body(aws_sdk_s3::primitives::ByteStream::from(part_data))
                    .send()
                    .await
                {
                    Ok(r) => r,
                    Err(e) => {
                        abort(&upload_id, &key).await;
                        return Err(OrbChrysaError::S3(e.to_string()));
                    }
                };

                parts.push(
                    CompletedPart::builder()
                        .part_number(part_number)
                        .set_e_tag(resp.e_tag().map(|s| s.to_string()))
                        .build(),
                );
            }
        }

        // Verify digest before completing
        let hash = hasher.finalize();
        let actual_digest = Digest::from_sha256_bytes(&hash);
        if actual_digest.to_string() != digest.to_string() {
            abort(&upload_id, &key).await;
            return Err(OrbChrysaError::S3(format!(
                "digest mismatch: expected {}, got {}",
                digest, actual_digest
            )));
        }

        // Flush remaining buffer
        if !buffer.is_empty() || parts.is_empty() {
            part_number += 1;
            let part_data = buffer.split().freeze();

            let resp = match self
                .client
                .upload_part()
                .bucket(&self.bucket)
                .key(&key)
                .upload_id(&upload_id)
                .part_number(part_number)
                .body(aws_sdk_s3::primitives::ByteStream::from(part_data))
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    abort(&upload_id, &key).await;
                    return Err(OrbChrysaError::S3(e.to_string()));
                }
            };

            parts.push(
                CompletedPart::builder()
                    .part_number(part_number)
                    .set_e_tag(resp.e_tag().map(|s| s.to_string()))
                    .build(),
            );
        }

        let completed = CompletedMultipartUpload::builder()
            .set_parts(Some(parts))
            .build();

        if let Err(e) = self
            .client
            .complete_multipart_upload()
            .bucket(&self.bucket)
            .key(&key)
            .upload_id(&upload_id)
            .multipart_upload(completed)
            .send()
            .await
        {
            abort(&upload_id, &key).await;
            return Err(OrbChrysaError::S3(e.to_string()));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_sdk_s3::Client;
    use aws_sdk_s3::config::{Credentials, Region};
    use httpmock::prelude::*;

    /// Build an S3BlobStore that talks to a `MockServer`.
    fn mock_store(server: &MockServer) -> S3BlobStore {
        let creds = Credentials::new("test", "test", None, None, "mock");
        let config = aws_sdk_s3::config::Builder::new()
            .endpoint_url(server.url(""))
            .force_path_style(true)
            .credentials_provider(creds)
            .behavior_version(aws_config::BehaviorVersion::latest())
            .region(Region::new("us-east-1"))
            .build();

        let client = Client::from_conf(config);
        S3BlobStore {
            client,
            bucket: "test-bucket".into(),
            redirect_enabled: false,
            presign_client: None,
            presign_expires_secs: 0,
        }
    }

    #[tokio::test]
    async fn stat_returns_blob_info() {
        let server = MockServer::start();
        let digest = Digest::from_str_checked(
            "sha256:abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789",
        )
        .unwrap();

        server.mock(|when, then| {
            when.method("HEAD")
                .path(format!("/test-bucket/{}", digest.s3_key()));
            then.status(200)
                .header("Content-Length", "1024")
                .header("ETag", "\"abc123\"");
        });

        let store = mock_store(&server);
        let info = store.stat(&digest).await.unwrap();
        assert_eq!(info.size, 1024);
    }

    #[tokio::test]
    async fn stat_unknown_blob_returns_blob_unknown_error() {
        let server = MockServer::start();
        let digest = Digest::from_str_checked(
            "sha256:0000000000000000000000000000000000000000000000000000000000000000",
        )
        .unwrap();

        server.mock(|when, then| {
            when.method("HEAD")
                .path(format!("/test-bucket/{}", digest.s3_key()));
            then.status(404);
        });

        let store = mock_store(&server);
        let result = store.stat(&digest).await;
        assert!(matches!(result, Err(OrbChrysaError::BlobUnknown(_))));
    }

    #[tokio::test]
    async fn health_check_hits_head_bucket() {
        let server = MockServer::start();

        server.mock(|when, then| {
            when.method("HEAD").path_contains("test-bucket");
            then.status(200);
        });

        let store = mock_store(&server);
        assert!(store.health_check().await.is_ok());
    }

    #[tokio::test]
    async fn health_check_bucket_not_found() {
        let server = MockServer::start();

        server.mock(|when, then| {
            when.method("HEAD").path_contains("test-bucket");
            then.status(404);
        });

        let store = mock_store(&server);
        assert!(store.health_check().await.is_err());
    }

    #[tokio::test]
    async fn get_returns_blob_stream() {
        let server = MockServer::start();
        let digest = Digest::from_str_checked(
            "sha256:abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789",
        )
        .unwrap();

        server.mock(|when, then| {
            when.method("GET")
                .path(format!("/test-bucket/{}", digest.s3_key()));
            then.status(200).header("Content-Length", "5").body("hello");
        });

        let store = mock_store(&server);
        let result = store.get(&digest).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn start_upload_creates_session() {
        let server = MockServer::start();

        // Mock the initial empty upload state PUT (write_upload_state is called
        // when start_upload begins, but InMemoryBlobStore start_upload is trivial)
        // S3BlobStore.start_upload creates a staged upload with an empty state.
        server.mock(|when, then| {
            when.method("PUT").path_contains("blob-state.json");
            then.status(200);
        });

        let store = mock_store(&server);
        let session_id = "test-session-1";
        store.start_upload(session_id).await.unwrap();
    }

    #[tokio::test]
    async fn start_upload_writes_empty_state() {
        let server = MockServer::start();

        server.mock(|when, then| {
            when.method("PUT").path_contains("blob-state.json");
            then.status(200);
        });

        let store = mock_store(&server);
        let session_id = "test-session-2";
        store.start_upload(session_id).await.unwrap();
    }

    #[tokio::test]
    async fn put_streaming_small_blob_succeeds() {
        let server = MockServer::start();
        let body = b"hello world";
        let digest = Digest::sha256(body);

        // 1. CreateMultipartUpload
        server.mock(|when, then| {
            when.method("POST")
                .path(format!("/test-bucket/{}", digest.s3_key()))
                .query_param("uploads", "");
            then.status(200)
                .body(r#"<InitiateMultipartUploadResult><UploadId>upload-123</UploadId></InitiateMultipartUploadResult>"#);
        });

        // 2. UploadPart
        server.mock(|when, then| {
            when.method("PUT")
                .path(format!("/test-bucket/{}", digest.s3_key()))
                .query_param("uploadId", "upload-123")
                .query_param("partNumber", "1");
            then.status(200).header("ETag", "\"etag-1\"");
        });

        // 3. CompleteMultipartUpload
        server.mock(|when, then| {
            when.method("POST")
                .path(format!("/test-bucket/{}", digest.s3_key()))
                .query_param("uploadId", "upload-123");
            then.status(200)
                .body(r#"<CompleteMultipartUploadResult><Location>http://test/obj</Location></CompleteMultipartUploadResult>"#);
        });

        let store = mock_store(&server);
        let body_bytes = Bytes::from(body.to_vec());
        let stream: crate::store::blob::ByteStream =
            Box::pin(futures::stream::once(async move { Ok(body_bytes) }));
        store.put_streaming(&digest, stream).await.unwrap();
    }
}

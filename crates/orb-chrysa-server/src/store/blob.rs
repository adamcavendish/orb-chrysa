use async_trait::async_trait;
use bytes::Bytes;
#[cfg(test)]
use bytes::BytesMut;
use futures::Stream;
#[cfg(test)]
use futures::StreamExt;
#[cfg(test)]
use sha2::{Digest as ShaDigest, Sha256};
#[cfg(test)]
use std::collections::BTreeMap;
use std::pin::Pin;
#[cfg(test)]
use std::sync::Arc;
#[cfg(test)]
use tokio::sync::RwLock;

use crate::error::OrbChrysaError;
use crate::oci::digest::Digest;

pub struct BlobInfo {
    pub size: u64,
}

pub type ByteStream =
    Pin<Box<dyn Stream<Item = Result<Bytes, Box<dyn std::error::Error + Send + Sync>>> + Send>>;

#[async_trait]
pub trait BlobStore: Send + Sync + 'static {
    async fn health_check(&self) -> Result<(), OrbChrysaError> {
        Ok(())
    }
    async fn stat(&self, digest: &Digest) -> Result<BlobInfo, OrbChrysaError>;
    async fn get(&self, digest: &Digest) -> Result<BlobStream, OrbChrysaError>;
    async fn get_range(
        &self,
        digest: &Digest,
        start: u64,
        end: u64,
    ) -> Result<BlobStream, OrbChrysaError>;
    async fn start_upload(&self, session_id: &str) -> Result<(), OrbChrysaError>;
    async fn push_chunk(&self, session_id: &str, data: Bytes) -> Result<u64, OrbChrysaError>;
    async fn complete_upload(
        &self,
        session_id: &str,
        expected_digest: &Digest,
    ) -> Result<(), OrbChrysaError>;
    async fn delete_upload(&self, session_id: &str) -> Result<(), OrbChrysaError>;
    fn redirect_enabled(&self) -> bool {
        false
    }
    async fn presigned_url(&self, digest: &Digest) -> Result<String, OrbChrysaError>;
    async fn put_streaming(
        &self,
        digest: &Digest,
        stream: ByteStream,
    ) -> Result<(), OrbChrysaError>;
}

pub enum BlobStream {
    S3(Box<aws_sdk_s3::operation::get_object::GetObjectOutput>),
    #[cfg(test)]
    Memory(ByteStream),
}

#[cfg(test)]
#[derive(Clone, Default)]
pub struct InMemoryBlobStore {
    blobs: Arc<RwLock<BTreeMap<String, Bytes>>>,
    uploads: Arc<RwLock<BTreeMap<String, BytesMut>>>,
    redirect_enabled: bool,
}

#[cfg(test)]
impl InMemoryBlobStore {
    pub fn with_redirect_enabled() -> Self {
        Self {
            redirect_enabled: true,
            ..Self::default()
        }
    }
}

#[cfg(test)]
#[async_trait]
impl BlobStore for InMemoryBlobStore {
    async fn stat(&self, digest: &Digest) -> Result<BlobInfo, OrbChrysaError> {
        let blobs = self.blobs.read().await;
        let Some(bytes) = blobs.get(&digest.to_string()) else {
            return Err(OrbChrysaError::BlobUnknown(digest.to_string()));
        };
        Ok(BlobInfo {
            size: bytes.len() as u64,
        })
    }
    async fn get(&self, digest: &Digest) -> Result<BlobStream, OrbChrysaError> {
        let blobs = self.blobs.read().await;
        let Some(bytes) = blobs.get(&digest.to_string()) else {
            return Err(OrbChrysaError::BlobUnknown(digest.to_string()));
        };
        let bytes = bytes.clone();
        Ok(BlobStream::Memory(Box::pin(futures::stream::once(
            async move { Ok(bytes) },
        ))))
    }
    async fn get_range(
        &self,
        digest: &Digest,
        start: u64,
        end: u64,
    ) -> Result<BlobStream, OrbChrysaError> {
        let blobs = self.blobs.read().await;
        let Some(bytes) = blobs.get(&digest.to_string()) else {
            return Err(OrbChrysaError::BlobUnknown(digest.to_string()));
        };
        let start = start as usize;
        let end = (end as usize).saturating_add(1).min(bytes.len());
        let slice = if start < end && start < bytes.len() {
            bytes.slice(start..end)
        } else {
            Bytes::new()
        };
        Ok(BlobStream::Memory(Box::pin(futures::stream::once(
            async move { Ok(slice) },
        ))))
    }
    async fn start_upload(&self, session_id: &str) -> Result<(), OrbChrysaError> {
        self.uploads
            .write()
            .await
            .insert(session_id.to_string(), BytesMut::new());
        Ok(())
    }
    async fn push_chunk(&self, session_id: &str, data: Bytes) -> Result<u64, OrbChrysaError> {
        let mut uploads = self.uploads.write().await;
        let Some(upload) = uploads.get_mut(session_id) else {
            return Err(OrbChrysaError::BlobUploadUnknown(session_id.to_string()));
        };
        upload.extend_from_slice(&data);
        Ok(upload.len() as u64)
    }
    async fn complete_upload(
        &self,
        session_id: &str,
        expected_digest: &Digest,
    ) -> Result<(), OrbChrysaError> {
        let bytes = self
            .uploads
            .write()
            .await
            .remove(session_id)
            .ok_or_else(|| OrbChrysaError::BlobUploadUnknown(session_id.to_string()))?
            .freeze();
        let actual = Digest::sha256(&bytes);
        if actual != *expected_digest {
            return Err(OrbChrysaError::DigestInvalid(format!(
                "digest mismatch: expected {}, got {}",
                expected_digest, actual
            )));
        }
        self.blobs
            .write()
            .await
            .insert(expected_digest.to_string(), bytes);
        Ok(())
    }
    async fn delete_upload(&self, session_id: &str) -> Result<(), OrbChrysaError> {
        self.uploads.write().await.remove(session_id);
        Ok(())
    }
    fn redirect_enabled(&self) -> bool {
        self.redirect_enabled
    }
    async fn presigned_url(&self, digest: &Digest) -> Result<String, OrbChrysaError> {
        Ok(format!("memory://{}", digest))
    }
    async fn put_streaming(
        &self,
        digest: &Digest,
        mut stream: ByteStream,
    ) -> Result<(), OrbChrysaError> {
        let mut data = BytesMut::new();
        let mut hasher = Sha256::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| OrbChrysaError::S3(e.to_string()))?;
            hasher.update(&chunk);
            data.extend_from_slice(&chunk);
        }
        let actual = Digest::from_sha256_bytes(&hasher.finalize());
        if actual != *digest {
            return Err(OrbChrysaError::DigestInvalid(format!(
                "digest mismatch: expected {}, got {}",
                digest, actual
            )));
        }
        self.blobs
            .write()
            .await
            .insert(digest.to_string(), data.freeze());
        Ok(())
    }
}

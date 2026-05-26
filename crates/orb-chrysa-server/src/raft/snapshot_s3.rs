use aws_sdk_s3::Client;
use aws_sdk_s3::primitives::ByteStream;
use thiserror::Error;

use crate::config::S3Config;

#[derive(Debug, Error)]
pub enum SnapshotError {
    #[error("S3 put_object failed: {0}")]
    Upload(String),
    #[error("S3 get_object failed: {0}")]
    Download(String),
    #[error("S3 body collect failed: {0}")]
    BodyCollect(String),
}

pub struct S3SnapshotStore {
    client: Client,
    bucket: String,
    node_id: u64,
}

impl S3SnapshotStore {
    pub async fn new(config: &S3Config, node_id: u64) -> Self {
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
            node_id,
        }
    }

    fn key(&self) -> String {
        format!("raft-snapshots/{}/latest.bin", self.node_id)
    }

    pub async fn upload(&self, data: &[u8]) -> Result<(), SnapshotError> {
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(self.key())
            .body(ByteStream::from(data.to_vec()))
            .send()
            .await
            .map_err(|e| SnapshotError::Upload(e.to_string()))?;
        tracing::info!(
            node_id = self.node_id,
            bytes = data.len(),
            "uploaded raft snapshot to S3"
        );
        Ok(())
    }

    pub async fn download(&self) -> Result<Option<Vec<u8>>, SnapshotError> {
        let result = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(self.key())
            .send()
            .await;

        match result {
            Ok(output) => {
                let bytes = output
                    .body
                    .collect()
                    .await
                    .map_err(|e| SnapshotError::BodyCollect(e.to_string()))?
                    .to_vec();
                tracing::info!(
                    node_id = self.node_id,
                    bytes = bytes.len(),
                    "downloaded raft snapshot from S3"
                );
                Ok(Some(bytes))
            }
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
                    Ok(None)
                } else {
                    Err(SnapshotError::Download(service_err.to_string()))
                }
            }
        }
    }
}

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Error)]
#[expect(
    dead_code,
    reason = "OCI Distribution error codes are represented before every variant is emitted"
)]
pub enum OrbChrysaError {
    // OCI Distribution Spec error codes
    #[error("BLOB_UNKNOWN: {0}")]
    BlobUnknown(String),

    #[error("BLOB_UPLOAD_INVALID: {0}")]
    BlobUploadInvalid(String),

    #[error("BLOB_UPLOAD_UNKNOWN: {0}")]
    BlobUploadUnknown(String),

    #[error("DIGEST_INVALID: {0}")]
    DigestInvalid(String),

    #[error("MANIFEST_BLOB_UNKNOWN: {0}")]
    ManifestBlobUnknown(String),

    #[error("MANIFEST_INVALID: {0}")]
    ManifestInvalid(String),

    #[error("MANIFEST_UNKNOWN: {0}")]
    ManifestUnknown(String),

    #[error("NAME_INVALID: {0}")]
    NameInvalid(String),

    #[error("NAME_UNKNOWN: {0}")]
    NameUnknown(String),

    #[error("SIZE_INVALID: {0}")]
    SizeInvalid(String),

    #[error("UNAUTHORIZED: {message}")]
    Unauthorized {
        message: String,
        realm: Option<String>,
        service: Option<String>,
        scope: Option<String>,
    },

    #[error("DENIED: {0}")]
    Denied(String),

    #[error("UNSUPPORTED: {0}")]
    Unsupported(String),

    #[error("TOOMANYREQUESTS: {0}")]
    TooManyRequests(String),

    #[error("CONFLICT: {0}")]
    Conflict(String),

    // Storage errors (flattened from former StorageError)
    #[error("S3 error: {0}")]
    S3(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("serialization error: {0}")]
    Serialization(String),

    // Mirror / upstream registry errors
    #[error("upstream error: {0}")]
    Upstream(String),

    // Raft consensus errors
    #[error("not leader, redirect to: {0}")]
    NotLeader(String),

    #[error("consensus error: {0}")]
    Consensus(String),

    // Internal catch-all
    #[error("internal error: {0}")]
    Internal(String),
}

#[derive(Serialize)]
struct OciErrorResponse {
    errors: Vec<OciErrorEntry>,
}

#[derive(Serialize)]
struct OciErrorEntry {
    code: &'static str,
    message: String,
    detail: serde_json::Value,
}

impl OrbChrysaError {
    fn oci_code(&self) -> &'static str {
        match self {
            Self::BlobUnknown(_) => "BLOB_UNKNOWN",
            Self::BlobUploadInvalid(_) => "BLOB_UPLOAD_INVALID",
            Self::BlobUploadUnknown(_) => "BLOB_UPLOAD_UNKNOWN",
            Self::DigestInvalid(_) => "DIGEST_INVALID",
            Self::ManifestBlobUnknown(_) => "MANIFEST_BLOB_UNKNOWN",
            Self::ManifestInvalid(_) => "MANIFEST_INVALID",
            Self::ManifestUnknown(_) => "MANIFEST_UNKNOWN",
            Self::NameInvalid(_) => "NAME_INVALID",
            Self::NameUnknown(_) => "NAME_UNKNOWN",
            Self::SizeInvalid(_) => "SIZE_INVALID",
            Self::Unauthorized { .. } => "UNAUTHORIZED",
            Self::Denied(_) => "DENIED",
            Self::Unsupported(_) => "UNSUPPORTED",
            Self::TooManyRequests(_) => "TOOMANYREQUESTS",
            Self::Conflict(_) => "CONFLICT",
            Self::S3(_)
            | Self::Io(_)
            | Self::Serialization(_)
            | Self::Upstream(_)
            | Self::NotLeader(_)
            | Self::Consensus(_)
            | Self::Internal(_) => "UNKNOWN",
        }
    }

    fn status_code(&self) -> StatusCode {
        match self {
            Self::BlobUnknown(_)
            | Self::ManifestUnknown(_)
            | Self::NameUnknown(_)
            | Self::BlobUploadUnknown(_) => StatusCode::NOT_FOUND,

            Self::BlobUploadInvalid(_)
            | Self::DigestInvalid(_)
            | Self::ManifestBlobUnknown(_)
            | Self::ManifestInvalid(_)
            | Self::NameInvalid(_)
            | Self::SizeInvalid(_) => StatusCode::BAD_REQUEST,

            Self::Unauthorized { .. } => StatusCode::UNAUTHORIZED,
            Self::Denied(_) => StatusCode::FORBIDDEN,
            Self::Unsupported(_) => StatusCode::METHOD_NOT_ALLOWED,
            Self::TooManyRequests(_) => StatusCode::TOO_MANY_REQUESTS,
            Self::Conflict(_) => StatusCode::CONFLICT,

            Self::Upstream(_) => StatusCode::BAD_GATEWAY,

            Self::NotLeader(_) | Self::Consensus(_) => StatusCode::SERVICE_UNAVAILABLE,

            Self::S3(_) | Self::Io(_) | Self::Serialization(_) | Self::Internal(_) => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        }
    }
}

impl OrbChrysaError {
    pub fn auth_required(
        realm: impl Into<String>,
        service: impl Into<String>,
        scope: impl Into<String>,
    ) -> Self {
        Self::Unauthorized {
            message: "authentication required".to_string(),
            realm: Some(realm.into()),
            service: Some(service.into()),
            scope: Some(scope.into()),
        }
    }
}

impl IntoResponse for OrbChrysaError {
    fn into_response(self) -> Response {
        let status = self.status_code();
        let body = OciErrorResponse {
            errors: vec![OciErrorEntry {
                code: self.oci_code(),
                message: self.to_string(),
                detail: serde_json::Value::Null,
            }],
        };
        let mut response = (status, axum::Json(body)).into_response();

        // Emit WWW-Authenticate header on 401 per OCI Distribution Spec
        if let Self::Unauthorized {
            realm,
            service,
            scope,
            ..
        } = &self
        {
            let mut value = String::from("Bearer");
            if let Some(r) = realm {
                value.push_str(&format!(" realm=\"{}\"", r));
            }
            if let Some(s) = service {
                value.push_str(&format!(",service=\"{}\"", s));
            }
            if let Some(sc) = scope {
                value.push_str(&format!(",scope=\"{}\"", sc));
            }
            if let Ok(hv) = axum::http::HeaderValue::from_str(&value) {
                response
                    .headers_mut()
                    .insert(axum::http::header::WWW_AUTHENTICATE, hv);
            }
        }

        response
    }
}

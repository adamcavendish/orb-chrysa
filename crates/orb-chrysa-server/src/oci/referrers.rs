use serde::Serialize;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferrerIndex {
    pub schema_version: u32,
    pub media_type: String,
    pub manifests: Vec<ReferrerDescriptor>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferrerDescriptor {
    pub media_type: String,
    pub digest: String,
    pub size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub annotations: Option<serde_json::Value>,
}

impl ReferrerIndex {
    pub fn empty() -> Self {
        Self {
            schema_version: 2,
            media_type: "application/vnd.oci.image.index.v1+json".to_string(),
            manifests: Vec::new(),
        }
    }
}

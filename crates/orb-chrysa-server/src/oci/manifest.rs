use crate::oci::digest::Digest;

pub fn is_manifest_media_type(media_type: &str) -> bool {
    matches!(
        media_type,
        "application/vnd.oci.image.manifest.v1+json"
            | "application/vnd.oci.image.index.v1+json"
            | "application/vnd.docker.distribution.manifest.v2+json"
            | "application/vnd.docker.distribution.manifest.list.v2+json"
    )
}

pub fn extract_referenced_digests(value: &serde_json::Value) -> Vec<Digest> {
    let mut digests = Vec::new();

    if let Some(config) = value
        .get("config")
        .and_then(|c| c.get("digest"))
        .and_then(|d| d.as_str())
        && let Some(d) = Digest::from_str_checked(config)
    {
        digests.push(d);
    }

    if let Some(layers) = value.get("layers").and_then(|l| l.as_array()) {
        for layer in layers {
            if let Some(digest_str) = layer.get("digest").and_then(|d| d.as_str())
                && let Some(d) = Digest::from_str_checked(digest_str)
            {
                digests.push(d);
            }
        }
    }

    digests
}

pub fn extract_subject_digest(value: &serde_json::Value) -> Option<Digest> {
    let digest_str = value.get("subject")?.get("digest")?.as_str()?;
    Digest::from_str_checked(digest_str)
}

pub fn extract_artifact_type(value: &serde_json::Value) -> Option<String> {
    if let Some(at) = value.get("artifactType").and_then(|v| v.as_str()) {
        return Some(at.to_string());
    }
    let config_mt = value.get("config")?.get("mediaType")?.as_str()?;
    if config_mt != "application/vnd.oci.empty.v1+json" {
        return Some(config_mt.to_string());
    }
    None
}

pub fn extract_annotations(value: &serde_json::Value) -> Option<serde_json::Value> {
    value.get("annotations").cloned()
}

pub fn extract_config_summary(manifest: &serde_json::Value) -> Option<serde_json::Value> {
    let mut summary = serde_json::Map::new();

    if let Some(config) = manifest.get("config").and_then(|v| v.as_object()) {
        if let Some(media_type) = config.get("mediaType").and_then(|v| v.as_str()) {
            summary.insert("mediaType".to_string(), media_type.into());
        }
        if let Some(digest) = config.get("digest").and_then(|v| v.as_str()) {
            summary.insert("config_digest".to_string(), digest.into());
        }
        if let Some(size) = config.get("size").and_then(|v| v.as_u64()) {
            summary.insert("config_size".to_string(), size.into());
        }
    }

    if let Some(layers) = manifest.get("layers").and_then(|v| v.as_array()) {
        summary.insert("layer_count".to_string(), layers.len().into());
        let total: u64 = layers
            .iter()
            .filter_map(|layer| layer.get("size").and_then(|v| v.as_u64()))
            .sum();
        summary.insert("layer_size_bytes".to_string(), total.into());
    }

    if let Some(manifests) = manifest.get("manifests").and_then(|v| v.as_array()) {
        summary.insert("manifest_count".to_string(), manifests.len().into());
        let platforms: Vec<serde_json::Value> = manifests
            .iter()
            .filter_map(|m| m.get("platform"))
            .cloned()
            .collect();
        if !platforms.is_empty() {
            summary.insert("platforms".to_string(), platforms.into());
        }
    }

    if let Some(artifact_type) = manifest.get("artifactType").and_then(|v| v.as_str()) {
        summary.insert("artifactType".to_string(), artifact_type.into());
    }

    if summary.is_empty() {
        None
    } else {
        Some(serde_json::Value::Object(summary))
    }
}

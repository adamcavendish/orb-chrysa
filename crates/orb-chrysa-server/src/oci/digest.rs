use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest as Sha2Digest, Sha256};
use std::fmt;
use std::str::FromStr;

use crate::error::OrbChrysaError;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Digest {
    pub algorithm: String,
    pub hex: String,
}

impl Digest {
    pub fn from_str_checked(s: &str) -> Option<Self> {
        let (algo, hex) = s.split_once(':')?;
        if algo != "sha256" && algo != "sha512" {
            return None;
        }
        let expected_len = if algo == "sha256" { 64 } else { 128 };
        if hex.len() != expected_len {
            return None;
        }
        if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        Some(Self {
            algorithm: algo.to_string(),
            hex: hex.to_string(),
        })
    }

    pub fn sha256(data: &[u8]) -> Self {
        let hash = Sha256::digest(data);
        Self {
            algorithm: "sha256".to_string(),
            hex: hex::encode(hash),
        }
    }

    pub fn from_sha256_bytes(hash: &[u8]) -> Self {
        Self {
            algorithm: "sha256".to_string(),
            hex: hex::encode(hash),
        }
    }

    pub fn s3_key(&self) -> String {
        let prefix = &self.hex[..2];
        format!("blobs/{}/{}/{}", self.algorithm, prefix, self.hex)
    }
}

impl fmt::Display for Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.algorithm, self.hex)
    }
}

impl std::str::FromStr for Digest {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        Self::from_str_checked(s).ok_or_else(|| format!("invalid digest: {}", s))
    }
}

impl TryFrom<&str> for Digest {
    type Error = OrbChrysaError;

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        Self::from_str_checked(s).ok_or_else(|| OrbChrysaError::DigestInvalid(s.to_string()))
    }
}

// ── Serde helpers: serialize Digest as "sha256:hex..." string ────────

pub mod serde_string {
    use super::*;

    pub fn serialize<S: Serializer>(digest: &Digest, s: S) -> Result<S::Ok, S::Error> {
        digest.to_string().serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Digest, D::Error> {
        let string = String::deserialize(d)?;
        Digest::from_str(&string).map_err(serde::de::Error::custom)
    }
}

pub mod serde_string_opt {
    use super::*;

    pub fn serialize<S: Serializer>(digest: &Option<Digest>, s: S) -> Result<S::Ok, S::Error> {
        digest.as_ref().map(|d| d.to_string()).serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Digest>, D::Error> {
        let string: Option<String> = Option::deserialize(d)?;
        string
            .map(|s| Digest::from_str(&s).map_err(serde::de::Error::custom))
            .transpose()
    }
}

pub mod serde_string_vec {
    use super::*;

    pub fn serialize<S: Serializer>(digests: &[Digest], s: S) -> Result<S::Ok, S::Error> {
        let strings: Vec<String> = digests.iter().map(|d| d.to_string()).collect();
        strings.serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<Digest>, D::Error> {
        let strings: Vec<String> = Vec::deserialize(d)?;
        strings
            .into_iter()
            .map(|s| Digest::from_str(&s).map_err(serde::de::Error::custom))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_sha256() {
        let d = Digest::from_str_checked(
            "sha256:a3ed95caeb02ffe68cdd9fd84406680ae93d633cb16422d00e8a7c22955b46d4",
        );
        assert!(d.is_some());
        let d = d.unwrap();
        assert_eq!(d.algorithm, "sha256");
        assert_eq!(
            d.s3_key(),
            "blobs/sha256/a3/a3ed95caeb02ffe68cdd9fd84406680ae93d633cb16422d00e8a7c22955b46d4"
        );
    }

    #[test]
    fn reject_invalid_digest() {
        assert!(Digest::from_str_checked("md5:abc").is_none());
        assert!(Digest::from_str_checked("sha256:tooshort").is_none());
        assert!(Digest::from_str_checked("noalgorithm").is_none());
    }

    #[test]
    fn sha256_computation() {
        let d = Digest::sha256(b"hello");
        assert_eq!(d.algorithm, "sha256");
        assert_eq!(d.hex.len(), 64);
    }
}

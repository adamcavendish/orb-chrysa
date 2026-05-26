use aes_gcm::aead::rand_core::RngCore;
use aes_gcm::aead::{Aead, OsRng};
use aes_gcm::{Aes256Gcm, Key, KeyInit, Nonce};
use serde::{Deserialize, Serialize};

use crate::error::OrbChrysaError;

#[derive(Debug, Serialize, Deserialize)]
pub struct DashboardSession {
    pub subject: String,
    pub username: Option<String>,
    pub display_name: Option<String>,
    pub email: Option<String>,
    pub access_token: String,
    pub refresh_token: String,
    pub id_token: String,
    pub expires_at: u64,
}

impl DashboardSession {
    pub fn encrypt(&self, key: &[u8; 32]) -> Result<String, OrbChrysaError> {
        let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
        let mut nonce_bytes = [0u8; 12];
        OsRng.fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);

        let plaintext =
            serde_json::to_vec(self).map_err(|e| OrbChrysaError::Serialization(e.to_string()))?;

        let ciphertext = cipher
            .encrypt(nonce, plaintext.as_ref())
            .map_err(|e| OrbChrysaError::Internal(format!("encryption failed: {}", e)))?;

        let mut result = nonce_bytes.to_vec();
        result.extend_from_slice(&ciphertext);
        Ok(base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            &result,
        ))
    }

    pub fn decrypt(encrypted: &str, key: &[u8; 32]) -> Result<Self, OrbChrysaError> {
        let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));

        let data = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, encrypted)
            .map_err(|e| OrbChrysaError::Internal(format!("base64 decode failed: {}", e)))?;

        if data.len() < 12 {
            return Err(OrbChrysaError::Internal("invalid session data".to_string()));
        }

        let (nonce_bytes, ciphertext) = data.split_at(12);
        let nonce = Nonce::from_slice(nonce_bytes);

        let plaintext = cipher
            .decrypt(nonce, ciphertext)
            .map_err(|_| OrbChrysaError::Internal("session decryption failed".to_string()))?;

        serde_json::from_slice(&plaintext).map_err(|e| OrbChrysaError::Serialization(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encrypt_decrypt_round_trip() {
        let key = [42u8; 32];
        let session = DashboardSession {
            subject: "user-1".into(),
            username: Some("admin".into()),
            display_name: Some("Admin".into()),
            email: Some("admin@test.local".into()),
            access_token: "access-token-value".into(),
            refresh_token: "refresh-token-value".into(),
            id_token: "id-token-value".into(),
            expires_at: 1717200000,
        };

        let encrypted = session.encrypt(&key).unwrap();
        let decrypted = DashboardSession::decrypt(&encrypted, &key).unwrap();

        assert_eq!(decrypted.subject, session.subject);
        assert_eq!(decrypted.username, session.username);
        assert_eq!(decrypted.email, session.email);
        assert_eq!(decrypted.access_token, session.access_token);
        assert_eq!(decrypted.expires_at, session.expires_at);
    }

    #[test]
    fn decrypt_wrong_key_fails() {
        let session = DashboardSession {
            subject: "user-1".into(),
            username: None,
            display_name: None,
            email: None,
            access_token: "token".into(),
            refresh_token: "refresh".into(),
            id_token: "id".into(),
            expires_at: 1,
        };

        let encrypted = session.encrypt(&[1u8; 32]).unwrap();
        let result = DashboardSession::decrypt(&encrypted, &[2u8; 32]);
        assert!(result.is_err());
    }
}

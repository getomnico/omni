use anyhow::{anyhow, Result};
use base64::{engine::general_purpose, Engine as _};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::env;

/// Encryption service for sensitive data using AES-256-GCM
pub struct EncryptionService {
    key: [u8; 32], // 256-bit key
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncryptedData {
    pub data: String,  // Base64 encoded encrypted data
    pub nonce: String, // Base64 encoded nonce
    pub salt: String,  // Base64 encoded salt
}

impl EncryptionService {
    /// Create a new encryption service with a key derived from environment variables
    pub fn new() -> Result<Self> {
        let master_key = env::var("ENCRYPTION_KEY")
            .map_err(|_| anyhow!("ENCRYPTION_KEY environment variable not set"))?;

        let base_salt = env::var("ENCRYPTION_SALT")
            .map_err(|_| anyhow!("ENCRYPTION_SALT environment variable not set"))?;

        if master_key.len() < 32 {
            return Err(anyhow!(
                "ENCRYPTION_KEY must be at least 32 characters long"
            ));
        }

        if base_salt.len() < 16 {
            return Err(anyhow!(
                "ENCRYPTION_SALT must be at least 16 characters long"
            ));
        }

        // Use HKDF to derive a proper 256-bit key from the master key and salt
        let key = Self::derive_key(&master_key, &base_salt)?;

        Ok(Self { key })
    }

    /// Derive a 256-bit key using HKDF
    fn derive_key(master_key: &str, base_salt: &str) -> Result<[u8; 32]> {
        use ring::hkdf::{self, HKDF_SHA256};

        let salt = hkdf::Salt::new(HKDF_SHA256, base_salt.as_bytes());
        let prk = salt.extract(master_key.as_bytes());

        let okm = prk
            .expand(&[b"clio-encryption-key"], HKDF_SHA256)
            .map_err(|_| anyhow!("Failed to derive encryption key"))?;

        let mut key = [0u8; 32];
        okm.fill(&mut key)
            .map_err(|_| anyhow!("Failed to fill key buffer"))?;

        Ok(key)
    }

    /// Encrypt data using AES-256-GCM
    pub fn encrypt(&self, data: &str) -> Result<EncryptedData> {
        use ring::aead::{self, BoundKey, SealingKey, UnboundKey, AES_256_GCM};

        // Generate random nonce (96 bits for GCM)
        let mut nonce_bytes = [0u8; 12];
        rand::thread_rng().fill_bytes(&mut nonce_bytes);

        // Generate random salt for this operation
        let mut salt_bytes = [0u8; 16];
        rand::thread_rng().fill_bytes(&mut salt_bytes);

        // Derive operation-specific key using the salt
        let operation_key = Self::derive_operation_key(&self.key, &salt_bytes)?;

        // Create sealing key
        let unbound_key = UnboundKey::new(&AES_256_GCM, &operation_key)
            .map_err(|_| anyhow!("Failed to create unbound key"))?;
        let nonce = aead::Nonce::assume_unique_for_key(nonce_bytes);
        let mut sealing_key = SealingKey::new(unbound_key, OneNonceSequence(Some(nonce)));

        // Encrypt the data
        let mut in_out = data.as_bytes().to_vec();
        sealing_key
            .seal_in_place_append_tag(aead::Aad::empty(), &mut in_out)
            .map_err(|_| anyhow!("Failed to encrypt data"))?;

        // Encode everything as base64
        let encrypted_data = general_purpose::STANDARD.encode(in_out);
        let nonce_b64 = general_purpose::STANDARD.encode(nonce_bytes);
        let salt_b64 = general_purpose::STANDARD.encode(salt_bytes);

        Ok(EncryptedData {
            data: encrypted_data,
            nonce: nonce_b64,
            salt: salt_b64,
        })
    }

    /// Decrypt data using AES-256-GCM
    pub fn decrypt(&self, encrypted_data: &EncryptedData) -> Result<String> {
        use ring::aead::{self, BoundKey, OpeningKey, UnboundKey, AES_256_GCM};

        // Decode base64 data
        let encrypted_bytes = general_purpose::STANDARD
            .decode(&encrypted_data.data)
            .map_err(|_| anyhow!("Failed to decode encrypted data"))?;
        let nonce_bytes = general_purpose::STANDARD
            .decode(&encrypted_data.nonce)
            .map_err(|_| anyhow!("Failed to decode nonce"))?;
        let salt_bytes = general_purpose::STANDARD
            .decode(&encrypted_data.salt)
            .map_err(|_| anyhow!("Failed to decode salt"))?;

        if nonce_bytes.len() != 12 {
            return Err(anyhow!("Invalid nonce length"));
        }
        if salt_bytes.len() != 16 {
            return Err(anyhow!("Invalid salt length"));
        }

        // Derive operation-specific key using the salt
        let operation_key = Self::derive_operation_key(&self.key, &salt_bytes)?;

        // Create opening key
        let unbound_key = UnboundKey::new(&AES_256_GCM, &operation_key)
            .map_err(|_| anyhow!("Failed to create unbound key"))?;
        let nonce = aead::Nonce::try_assume_unique_for_key(&nonce_bytes)
            .map_err(|_| anyhow!("Failed to create nonce"))?;
        let mut opening_key = OpeningKey::new(unbound_key, OneNonceSequence(Some(nonce)));

        // Decrypt the data
        let mut in_out = encrypted_bytes;
        let plaintext = opening_key
            .open_in_place(aead::Aad::empty(), &mut in_out)
            .map_err(|_| anyhow!("Failed to decrypt data"))?;

        String::from_utf8(plaintext.to_vec())
            .map_err(|_| anyhow!("Decrypted data is not valid UTF-8"))
    }

    /// Derive operation-specific key from master key and salt
    fn derive_operation_key(master_key: &[u8; 32], salt: &[u8]) -> Result<[u8; 32]> {
        use ring::hkdf::{self, HKDF_SHA256};

        let salt = hkdf::Salt::new(HKDF_SHA256, salt);
        let prk = salt.extract(master_key);

        let okm = prk
            .expand(&[b"clio-operation-key"], HKDF_SHA256)
            .map_err(|_| anyhow!("Failed to derive operation key"))?;

        let mut key = [0u8; 32];
        okm.fill(&mut key)
            .map_err(|_| anyhow!("Failed to fill operation key buffer"))?;

        Ok(key)
    }

    /// Encrypt JSON data (convenience method)
    pub fn encrypt_json<T: Serialize>(&self, data: &T) -> Result<EncryptedData> {
        let json_str =
            serde_json::to_string(data).map_err(|e| anyhow!("Failed to serialize data: {}", e))?;
        self.encrypt(&json_str)
    }

    /// Decrypt JSON data (convenience method)
    pub fn decrypt_json<T: for<'de> Deserialize<'de>>(
        &self,
        encrypted_data: &EncryptedData,
    ) -> Result<T> {
        let json_str = self.decrypt(encrypted_data)?;
        serde_json::from_str(&json_str).map_err(|e| anyhow!("Failed to deserialize data: {}", e))
    }
}

/// Helper struct for nonce sequence (ring requirement)
struct OneNonceSequence(Option<ring::aead::Nonce>);

impl ring::aead::NonceSequence for OneNonceSequence {
    fn advance(&mut self) -> Result<ring::aead::Nonce, ring::error::Unspecified> {
        self.0.take().ok_or(ring::error::Unspecified)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    // Mutex to prevent tests from interfering with each other's environment variables
    static TEST_ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn test_encrypt_decrypt_string() {
        // Set up test environment variables
        let _lock = TEST_ENV_LOCK.lock().unwrap();
        std::env::set_var(
            "ENCRYPTION_KEY",
            "test_master_key_that_is_long_enough_32_chars",
        );
        std::env::set_var("ENCRYPTION_SALT", "test_salt_16_chars");

        let service = EncryptionService::new().unwrap();
        let original_data = "Hello, World! This is sensitive data.";

        // Encrypt
        let encrypted = service.encrypt(original_data).unwrap();

        // Verify structure
        assert!(!encrypted.data.is_empty());
        assert!(!encrypted.nonce.is_empty());
        assert!(!encrypted.salt.is_empty());

        // Decrypt
        let decrypted = service.decrypt(&encrypted).unwrap();
        assert_eq!(decrypted, original_data);
    }

    #[test]
    fn test_encrypt_decrypt_json() {
        // Set up test environment variables
        let _lock = TEST_ENV_LOCK.lock().unwrap();
        std::env::set_var(
            "ENCRYPTION_KEY",
            "test_master_key_that_is_long_enough_32_chars",
        );
        std::env::set_var("ENCRYPTION_SALT", "test_salt_16_chars");

        let service = EncryptionService::new().unwrap();

        // Test with a HashMap (similar to JSONB data)
        let mut original_data = HashMap::new();
        original_data.insert("client_id".to_string(), "test_client_id".to_string());
        original_data.insert(
            "client_secret".to_string(),
            "test_client_secret".to_string(),
        );
        original_data.insert(
            "refresh_token".to_string(),
            "test_refresh_token".to_string(),
        );

        // Encrypt
        let encrypted = service.encrypt_json(&original_data).unwrap();

        // Decrypt
        let decrypted: HashMap<String, String> = service.decrypt_json(&encrypted).unwrap();
        assert_eq!(decrypted, original_data);
    }

    #[test]
    fn test_different_salts_produce_different_ciphertexts() {
        // Set up test environment variables for this test
        let _lock = TEST_ENV_LOCK.lock().unwrap();
        std::env::set_var(
            "ENCRYPTION_KEY",
            "test_master_key_that_is_long_enough_32_chars",
        );
        std::env::set_var("ENCRYPTION_SALT", "test_salt_16_chars");

        let service = EncryptionService::new().unwrap();
        let data = "Same data for both encryptions";

        let encrypted1 = service.encrypt(data).unwrap();
        let encrypted2 = service.encrypt(data).unwrap();

        // Different salts should produce different ciphertexts
        assert_ne!(encrypted1.data, encrypted2.data);
        assert_ne!(encrypted1.salt, encrypted2.salt);

        // Both should decrypt to the same original data
        assert_eq!(service.decrypt(&encrypted1).unwrap(), data);
        assert_eq!(service.decrypt(&encrypted2).unwrap(), data);
    }

    #[test]
    fn test_invalid_environment_variables() {
        let _lock = TEST_ENV_LOCK.lock().unwrap();

        // Test missing ENCRYPTION_KEY
        std::env::remove_var("ENCRYPTION_KEY");
        std::env::set_var("ENCRYPTION_SALT", "test_salt_16_chars");
        assert!(EncryptionService::new().is_err());

        // Test missing ENCRYPTION_SALT
        std::env::set_var(
            "ENCRYPTION_KEY",
            "test_master_key_that_is_long_enough_32_chars",
        );
        std::env::remove_var("ENCRYPTION_SALT");
        assert!(EncryptionService::new().is_err());

        // Test short ENCRYPTION_KEY
        std::env::set_var("ENCRYPTION_KEY", "short");
        std::env::set_var("ENCRYPTION_SALT", "test_salt_16_chars");
        assert!(EncryptionService::new().is_err());

        // Test short ENCRYPTION_SALT
        std::env::set_var(
            "ENCRYPTION_KEY",
            "test_master_key_that_is_long_enough_32_chars",
        );
        std::env::set_var("ENCRYPTION_SALT", "short");
        assert!(EncryptionService::new().is_err());
    }
}

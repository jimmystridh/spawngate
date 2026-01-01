//! Secrets management with encryption at rest
//!
//! Provides encrypted storage for sensitive environment variables.
//! Uses AES-256-GCM for authenticated encryption.

use aes_gcm::{
    aead::{Aead, KeyInit, OsRng},
    Aes256Gcm, Nonce,
};
use anyhow::{Context, Result};
use rand::RngCore;
use std::collections::HashMap;
use std::path::Path;
use tracing::info;

/// Length of encryption key in bytes (256 bits)
const KEY_LENGTH: usize = 32;
/// Length of nonce in bytes (96 bits)
const NONCE_LENGTH: usize = 12;
/// Prefix for encrypted values in database
const ENCRYPTED_PREFIX: &str = "enc:v1:";

/// Encryption key for secrets
#[derive(Clone)]
pub struct EncryptionKey {
    key: [u8; KEY_LENGTH],
    id: String,
    created_at: String,
}

impl EncryptionKey {
    /// Generate a new random encryption key
    pub fn generate() -> Self {
        let mut key = [0u8; KEY_LENGTH];
        OsRng.fill_bytes(&mut key);

        let id = uuid::Uuid::new_v4().to_string();
        let created_at = chrono_lite_now();

        Self {
            key,
            id,
            created_at,
        }
    }

    /// Create from raw bytes
    pub fn from_bytes(bytes: &[u8], id: String, created_at: String) -> Result<Self> {
        if bytes.len() != KEY_LENGTH {
            anyhow::bail!("Invalid key length: expected {}, got {}", KEY_LENGTH, bytes.len());
        }

        let mut key = [0u8; KEY_LENGTH];
        key.copy_from_slice(bytes);

        Ok(Self {
            key,
            id,
            created_at,
        })
    }

    /// Get the key ID
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Get creation timestamp
    pub fn created_at(&self) -> &str {
        &self.created_at
    }

    /// Export key as base64
    pub fn to_base64(&self) -> String {
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &self.key)
    }

    /// Import key from base64
    pub fn from_base64(encoded: &str, id: String, created_at: String) -> Result<Self> {
        let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, encoded)
            .context("Failed to decode base64 key")?;
        Self::from_bytes(&bytes, id, created_at)
    }
}

/// Secrets manager for encrypting/decrypting sensitive config vars
pub struct SecretsManager {
    /// Current encryption key
    current_key: EncryptionKey,
    /// Previous keys for decryption during rotation
    previous_keys: Vec<EncryptionKey>,
}

impl SecretsManager {
    /// Create a new secrets manager with a fresh key
    pub fn new() -> Self {
        Self {
            current_key: EncryptionKey::generate(),
            previous_keys: Vec::new(),
        }
    }

    /// Create from an existing key
    pub fn with_key(key: EncryptionKey) -> Self {
        Self {
            current_key: key,
            previous_keys: Vec::new(),
        }
    }

    /// Load secrets manager from a key file
    pub fn load_from_file(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .context("Failed to read key file")?;

        let key_data: KeyFileData = serde_json::from_str(&content)
            .context("Failed to parse key file")?;

        let current_key = EncryptionKey::from_base64(
            &key_data.current_key,
            key_data.current_key_id,
            key_data.current_key_created_at,
        )?;

        let mut previous_keys = Vec::new();
        for prev in key_data.previous_keys {
            if let Ok(key) = EncryptionKey::from_base64(&prev.key, prev.id, prev.created_at) {
                previous_keys.push(key);
            }
        }

        Ok(Self {
            current_key,
            previous_keys,
        })
    }

    /// Save secrets manager to a key file
    pub fn save_to_file(&self, path: &Path) -> Result<()> {
        let key_data = KeyFileData {
            current_key: self.current_key.to_base64(),
            current_key_id: self.current_key.id.clone(),
            current_key_created_at: self.current_key.created_at.clone(),
            previous_keys: self
                .previous_keys
                .iter()
                .map(|k| PreviousKeyData {
                    key: k.to_base64(),
                    id: k.id.clone(),
                    created_at: k.created_at.clone(),
                })
                .collect(),
        };

        let content = serde_json::to_string_pretty(&key_data)?;

        // Create parent directory if needed
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Write atomically
        let tmp_path = path.with_extension("tmp");
        std::fs::write(&tmp_path, &content)?;
        std::fs::rename(&tmp_path, path)?;

        // Set restrictive permissions on Unix
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(path)?.permissions();
            perms.set_mode(0o600);
            std::fs::set_permissions(path, perms)?;
        }

        info!(path = %path.display(), "Saved encryption key");
        Ok(())
    }

    /// Get current key ID
    pub fn current_key_id(&self) -> &str {
        self.current_key.id()
    }

    /// Rotate to a new key
    pub fn rotate_key(&mut self) -> &EncryptionKey {
        let old_key = std::mem::replace(&mut self.current_key, EncryptionKey::generate());
        self.previous_keys.insert(0, old_key);

        // Keep only last 5 previous keys
        if self.previous_keys.len() > 5 {
            self.previous_keys.truncate(5);
        }

        info!(
            new_key_id = self.current_key.id(),
            previous_keys = self.previous_keys.len(),
            "Rotated encryption key"
        );

        &self.current_key
    }

    /// Encrypt a secret value
    pub fn encrypt(&self, plaintext: &str) -> Result<String> {
        let cipher = Aes256Gcm::new_from_slice(&self.current_key.key)
            .map_err(|e| anyhow::anyhow!("Failed to create cipher: {}", e))?;

        // Generate random nonce
        let mut nonce_bytes = [0u8; NONCE_LENGTH];
        OsRng.fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);

        // Encrypt
        let ciphertext = cipher
            .encrypt(nonce, plaintext.as_bytes())
            .map_err(|e| anyhow::anyhow!("Encryption failed: {}", e))?;

        // Format: enc:v1:<key_id>:<nonce_base64>:<ciphertext_base64>
        let nonce_b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &nonce_bytes);
        let cipher_b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &ciphertext);

        Ok(format!(
            "{}{}:{}:{}",
            ENCRYPTED_PREFIX,
            self.current_key.id(),
            nonce_b64,
            cipher_b64
        ))
    }

    /// Decrypt a secret value
    pub fn decrypt(&self, encrypted: &str) -> Result<String> {
        if !encrypted.starts_with(ENCRYPTED_PREFIX) {
            // Not encrypted, return as-is
            return Ok(encrypted.to_string());
        }

        let data = encrypted.strip_prefix(ENCRYPTED_PREFIX).unwrap();
        let parts: Vec<&str> = data.splitn(3, ':').collect();

        if parts.len() != 3 {
            anyhow::bail!("Invalid encrypted format");
        }

        let key_id = parts[0];
        let nonce_b64 = parts[1];
        let cipher_b64 = parts[2];

        // Find the key
        let key = if self.current_key.id() == key_id {
            &self.current_key
        } else {
            self.previous_keys
                .iter()
                .find(|k| k.id() == key_id)
                .ok_or_else(|| anyhow::anyhow!("Unknown key ID: {}", key_id))?
        };

        // Decode
        let nonce_bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, nonce_b64)
            .context("Failed to decode nonce")?;
        let ciphertext = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, cipher_b64)
            .context("Failed to decode ciphertext")?;

        if nonce_bytes.len() != NONCE_LENGTH {
            anyhow::bail!("Invalid nonce length");
        }

        let nonce = Nonce::from_slice(&nonce_bytes);

        // Decrypt
        let cipher = Aes256Gcm::new_from_slice(&key.key)
            .map_err(|e| anyhow::anyhow!("Failed to create cipher: {}", e))?;

        let plaintext = cipher
            .decrypt(nonce, ciphertext.as_ref())
            .map_err(|e| anyhow::anyhow!("Decryption failed: {}", e))?;

        String::from_utf8(plaintext).context("Invalid UTF-8 in decrypted value")
    }

    /// Check if a value is encrypted
    pub fn is_encrypted(value: &str) -> bool {
        value.starts_with(ENCRYPTED_PREFIX)
    }

    /// Re-encrypt a value with the current key
    pub fn re_encrypt(&self, encrypted: &str) -> Result<String> {
        let plaintext = self.decrypt(encrypted)?;
        self.encrypt(&plaintext)
    }

    /// Encrypt all secrets in a config map
    pub fn encrypt_secrets(&self, config: &HashMap<String, String>, secret_keys: &[&str]) -> HashMap<String, String> {
        let mut result = config.clone();

        for key in secret_keys {
            if let Some(value) = result.get(*key) {
                if !Self::is_encrypted(value) {
                    if let Ok(encrypted) = self.encrypt(value) {
                        result.insert(key.to_string(), encrypted);
                    }
                }
            }
        }

        result
    }

    /// Decrypt all secrets in a config map
    pub fn decrypt_secrets(&self, config: &HashMap<String, String>) -> Result<HashMap<String, String>> {
        let mut result = HashMap::new();

        for (key, value) in config {
            let decrypted = if Self::is_encrypted(value) {
                self.decrypt(value)?
            } else {
                value.clone()
            };
            result.insert(key.clone(), decrypted);
        }

        Ok(result)
    }
}

impl Default for SecretsManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Key file format for persistence
#[derive(serde::Serialize, serde::Deserialize)]
struct KeyFileData {
    current_key: String,
    current_key_id: String,
    current_key_created_at: String,
    previous_keys: Vec<PreviousKeyData>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct PreviousKeyData {
    key: String,
    id: String,
    created_at: String,
}

/// Simple timestamp without chrono dependency
fn chrono_lite_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    // Format as ISO 8601
    let secs_per_day = 86400u64;
    let days_since_epoch = now / secs_per_day;
    let secs_today = now % secs_per_day;

    let hours = secs_today / 3600;
    let minutes = (secs_today % 3600) / 60;
    let seconds = secs_today % 60;

    // Approximate date calculation (not accounting for leap years precisely)
    let mut year = 1970i32;
    let mut remaining_days = days_since_epoch as i32;

    loop {
        let days_in_year = if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) {
            366
        } else {
            365
        };

        if remaining_days < days_in_year {
            break;
        }
        remaining_days -= days_in_year;
        year += 1;
    }

    let is_leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days_in_months: [i32; 12] = if is_leap {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };

    let mut month = 1;
    for &days in &days_in_months {
        if remaining_days < days {
            break;
        }
        remaining_days -= days;
        month += 1;
    }

    let day = remaining_days + 1;

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year, month, day, hours, minutes, seconds
    )
}

/// Secret config variable that tracks whether it's a secret
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ConfigVar {
    pub value: String,
    pub is_secret: bool,
}

/// Common secret key patterns
pub const SECRET_KEY_PATTERNS: &[&str] = &[
    "password",
    "secret",
    "api_key",
    "apikey",
    "token",
    "credential",
    "private",
    "auth",
    "key",
];

/// Check if a config key name looks like it should be a secret
pub fn is_likely_secret(key: &str) -> bool {
    let lower = key.to_lowercase();
    SECRET_KEY_PATTERNS.iter().any(|pattern| lower.contains(pattern))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encrypt_decrypt() {
        let manager = SecretsManager::new();

        let plaintext = "my-secret-password";
        let encrypted = manager.encrypt(plaintext).unwrap();

        assert!(encrypted.starts_with(ENCRYPTED_PREFIX));
        assert_ne!(encrypted, plaintext);

        let decrypted = manager.decrypt(&encrypted).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_decrypt_non_encrypted() {
        let manager = SecretsManager::new();

        let value = "plain-value";
        let result = manager.decrypt(value).unwrap();
        assert_eq!(result, value);
    }

    #[test]
    fn test_is_encrypted() {
        assert!(SecretsManager::is_encrypted("enc:v1:abc:def:ghi"));
        assert!(!SecretsManager::is_encrypted("plain-value"));
        assert!(!SecretsManager::is_encrypted(""));
    }

    #[test]
    fn test_key_rotation() {
        let mut manager = SecretsManager::new();
        let original_id = manager.current_key_id().to_string();

        // Encrypt with original key
        let encrypted = manager.encrypt("secret").unwrap();

        // Rotate key
        manager.rotate_key();

        // New key should be different
        assert_ne!(manager.current_key_id(), original_id);

        // Should still decrypt old value
        let decrypted = manager.decrypt(&encrypted).unwrap();
        assert_eq!(decrypted, "secret");

        // Re-encrypt with new key
        let re_encrypted = manager.re_encrypt(&encrypted).unwrap();
        assert!(re_encrypted.contains(manager.current_key_id()));

        // Should still decrypt
        let decrypted2 = manager.decrypt(&re_encrypted).unwrap();
        assert_eq!(decrypted2, "secret");
    }

    #[test]
    fn test_encrypt_secrets_map() {
        let manager = SecretsManager::new();

        let mut config = HashMap::new();
        config.insert("DATABASE_URL".to_string(), "postgres://localhost".to_string());
        config.insert("API_KEY".to_string(), "secret123".to_string());
        config.insert("DEBUG".to_string(), "true".to_string());

        let encrypted = manager.encrypt_secrets(&config, &["API_KEY"]);

        // API_KEY should be encrypted
        assert!(SecretsManager::is_encrypted(encrypted.get("API_KEY").unwrap()));

        // Others should not be encrypted
        assert!(!SecretsManager::is_encrypted(encrypted.get("DATABASE_URL").unwrap()));
        assert!(!SecretsManager::is_encrypted(encrypted.get("DEBUG").unwrap()));
    }

    #[test]
    fn test_is_likely_secret() {
        assert!(is_likely_secret("DATABASE_PASSWORD"));
        assert!(is_likely_secret("API_KEY"));
        assert!(is_likely_secret("SECRET_TOKEN"));
        assert!(is_likely_secret("PRIVATE_KEY"));
        assert!(is_likely_secret("AUTH_TOKEN"));

        assert!(!is_likely_secret("DATABASE_URL"));
        assert!(!is_likely_secret("PORT"));
        assert!(!is_likely_secret("DEBUG"));
        assert!(!is_likely_secret("NODE_ENV"));
    }

    #[test]
    fn test_key_file_roundtrip() {
        let manager = SecretsManager::new();
        let encrypted = manager.encrypt("test-secret").unwrap();

        let tmp = tempfile::TempDir::new().unwrap();
        let key_path = tmp.path().join("keys.json");

        manager.save_to_file(&key_path).unwrap();

        let loaded = SecretsManager::load_from_file(&key_path).unwrap();
        let decrypted = loaded.decrypt(&encrypted).unwrap();

        assert_eq!(decrypted, "test-secret");
    }
}

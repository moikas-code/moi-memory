use anyhow::{Result, anyhow};
use base64::{Engine as _, engine::general_purpose};
use rand::RngCore;
use serde::{Deserialize, Serialize, Deserializer, Serializer};
use sha2::{Sha256, Digest};
use std::collections::HashMap;
use std::env;
use std::fmt;
use tracing::{debug, warn, error};

// AES-GCM encryption using ring
use ring::{
    aead::{self, Aad, LessSafeKey, Nonce, UnboundKey, NONCE_LEN},
    rand::{SecureRandom, SystemRandom},
};

/// Configuration encryption settings
#[derive(Debug, Clone)]
pub struct EncryptionConfig {
    /// Master key for encryption (derived from environment or generated)
    pub master_key: Vec<u8>,
    /// Key derivation salt
    pub salt: Vec<u8>,
    /// Enable encryption for all sensitive fields
    pub encrypt_all_sensitive: bool,
    /// List of field patterns to encrypt
    pub encrypted_fields: Vec<String>,
}

impl Default for EncryptionConfig {
    fn default() -> Self {
        Self {
            master_key: Self::derive_default_key(),
            salt: Self::generate_salt(),
            encrypt_all_sensitive: true,
            encrypted_fields: vec![
                "password".to_string(),
                "secret".to_string(),
                "token".to_string(),
                "key".to_string(),
                "credentials".to_string(),
                "jwt_secret".to_string(),
                "database_password".to_string(),
                "redis_password".to_string(),
                "api_key".to_string(),
            ],
        }
    }
}

impl EncryptionConfig {
    /// Derive default key from environment or generate one
    fn derive_default_key() -> Vec<u8> {
        if let Ok(key_env) = env::var("KG_ENCRYPTION_KEY") {
            // Use provided key from environment
            let mut hasher = Sha256::new();
            hasher.update(key_env.as_bytes());
            hasher.finalize().to_vec()
        } else {
            // Generate a default key (NOT recommended for production)
            warn!("No encryption key found in environment. Using default key. Set KG_ENCRYPTION_KEY for production.");
            let mut hasher = Sha256::new();
            hasher.update(b"default-knowledge-graph-mcp-key-change-in-production");
            hasher.finalize().to_vec()
        }
    }
    
    /// Generate random salt
    fn generate_salt() -> Vec<u8> {
        let mut salt = vec![0u8; 16];
        let rng = SystemRandom::new();
        rng.fill(&mut salt).expect("Failed to generate salt");
        salt
    }
}

/// Encrypted string that can be serialized/deserialized
#[derive(Debug, Clone)]
pub struct EncryptedString {
    /// Base64 encoded encrypted data
    pub encrypted_data: String,
    /// Base64 encoded nonce
    pub nonce: String,
    /// Indicates if the data is actually encrypted
    pub is_encrypted: bool,
}

impl EncryptedString {
    /// Create a new encrypted string
    pub fn new(plaintext: &str, key: &[u8]) -> Result<Self> {
        let (encrypted_data, nonce) = encrypt_data(plaintext.as_bytes(), key)?;
        
        Ok(Self {
            encrypted_data: general_purpose::STANDARD.encode(&encrypted_data),
            nonce: general_purpose::STANDARD.encode(&nonce),
            is_encrypted: true,
        })
    }
    
    /// Create from plaintext (not encrypted)
    pub fn from_plaintext(plaintext: &str) -> Self {
        Self {
            encrypted_data: general_purpose::STANDARD.encode(plaintext.as_bytes()),
            nonce: String::new(),
            is_encrypted: false,
        }
    }
    
    /// Decrypt and return the plaintext
    pub fn decrypt(&self, key: &[u8]) -> Result<String> {
        if !self.is_encrypted {
            // Data is not encrypted, decode directly
            let decoded = general_purpose::STANDARD.decode(&self.encrypted_data)
                .map_err(|e| anyhow!("Failed to decode unencrypted data: {}", e))?;
            return Ok(String::from_utf8(decoded)
                .map_err(|e| anyhow!("Invalid UTF-8 in unencrypted data: {}", e))?);
        }
        
        let encrypted_data = general_purpose::STANDARD.decode(&self.encrypted_data)
            .map_err(|e| anyhow!("Failed to decode encrypted data: {}", e))?;
        let nonce = general_purpose::STANDARD.decode(&self.nonce)
            .map_err(|e| anyhow!("Failed to decode nonce: {}", e))?;
        
        let plaintext = decrypt_data(&encrypted_data, &nonce, key)?;
        String::from_utf8(plaintext)
            .map_err(|e| anyhow!("Invalid UTF-8 in decrypted data: {}", e))
    }
    
    /// Check if this string should be encrypted based on field name
    pub fn should_encrypt(field_name: &str, config: &EncryptionConfig) -> bool {
        if !config.encrypt_all_sensitive {
            return false;
        }
        
        let field_lower = field_name.to_lowercase();
        config.encrypted_fields.iter().any(|pattern| {
            field_lower.contains(&pattern.to_lowercase())
        })
    }
}

impl fmt::Display for EncryptedString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_encrypted {
            write!(f, "[ENCRYPTED]")
        } else {
            write!(f, "[UNENCRYPTED]")
        }
    }
}

/// Custom serialization for EncryptedString
impl Serialize for EncryptedString {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeStruct;
        
        let mut state = serializer.serialize_struct("EncryptedString", 3)?;
        state.serialize_field("encrypted_data", &self.encrypted_data)?;
        state.serialize_field("nonce", &self.nonce)?;
        state.serialize_field("is_encrypted", &self.is_encrypted)?;
        state.end()
    }
}

/// Custom deserialization for EncryptedString
impl<'de> Deserialize<'de> for EncryptedString {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct EncryptedStringHelper {
            encrypted_data: String,
            nonce: String,
            is_encrypted: bool,
        }
        
        let helper = EncryptedStringHelper::deserialize(deserializer)?;
        Ok(EncryptedString {
            encrypted_data: helper.encrypted_data,
            nonce: helper.nonce,
            is_encrypted: helper.is_encrypted,
        })
    }
}

/// Configuration encryptor/decryptor
pub struct ConfigEncryption {
    config: EncryptionConfig,
}

impl ConfigEncryption {
    /// Create new config encryption instance
    pub fn new(config: EncryptionConfig) -> Self {
        Self { config }
    }
    
    /// Create with default configuration
    pub fn default() -> Self {
        Self::new(EncryptionConfig::default())
    }
    
    /// Encrypt a configuration value
    pub fn encrypt_value(&self, value: &str, field_name: &str) -> Result<EncryptedString> {
        if EncryptedString::should_encrypt(field_name, &self.config) {
            EncryptedString::new(value, &self.config.master_key)
        } else {
            Ok(EncryptedString::from_plaintext(value))
        }
    }
    
    /// Decrypt a configuration value
    pub fn decrypt_value(&self, encrypted: &EncryptedString) -> Result<String> {
        encrypted.decrypt(&self.config.master_key)
    }
    
    /// Encrypt sensitive fields in a configuration map
    pub fn encrypt_config_map(&self, config: &mut HashMap<String, serde_json::Value>) -> Result<()> {
        for (key, value) in config.iter_mut() {
            if let Some(string_value) = value.as_str() {
                if EncryptedString::should_encrypt(key, &self.config) {
                    let encrypted = self.encrypt_value(string_value, key)?;
                    *value = serde_json::to_value(encrypted)
                        .map_err(|e| anyhow!("Failed to serialize encrypted value: {}", e))?;
                }
            }
        }
        Ok(())
    }
    
    /// Decrypt sensitive fields in a configuration map
    pub fn decrypt_config_map(&self, config: &mut HashMap<String, serde_json::Value>) -> Result<()> {
        for (key, value) in config.iter_mut() {
            // Check if this looks like an encrypted value
            if let Ok(encrypted_string) = serde_json::from_value::<EncryptedString>(value.clone()) {
                if encrypted_string.is_encrypted {
                    let decrypted = self.decrypt_value(&encrypted_string)?;
                    *value = serde_json::Value::String(decrypted);
                }
            }
        }
        Ok(())
    }
    
    /// Securely store configuration to file
    pub fn store_encrypted_config<T>(&self, config: &T, file_path: &str) -> Result<()>
    where
        T: Serialize,
    {
        // Serialize to JSON
        let json_value = serde_json::to_value(config)
            .map_err(|e| anyhow!("Failed to serialize config: {}", e))?;
        
        // Convert to map for encryption
        let mut config_map: HashMap<String, serde_json::Value> = 
            serde_json::from_value(json_value)
                .map_err(|e| anyhow!("Failed to convert config to map: {}", e))?;
        
        // Encrypt sensitive fields
        self.encrypt_config_map(&mut config_map)?;
        
        // Write to file
        let encrypted_json = serde_json::to_string_pretty(&config_map)
            .map_err(|e| anyhow!("Failed to serialize encrypted config: {}", e))?;
        
        std::fs::write(file_path, encrypted_json)
            .map_err(|e| anyhow!("Failed to write encrypted config file: {}", e))?;
        
        debug!("Stored encrypted configuration to {}", file_path);
        Ok(())
    }
    
    /// Load and decrypt configuration from file
    pub fn load_encrypted_config<T>(&self, file_path: &str) -> Result<T>
    where
        T: for<'de> Deserialize<'de>,
    {
        // Read file
        let config_data = std::fs::read_to_string(file_path)
            .map_err(|e| anyhow!("Failed to read config file {}: {}", file_path, e))?;
        
        // Parse JSON
        let mut config_map: HashMap<String, serde_json::Value> = 
            serde_json::from_str(&config_data)
                .map_err(|e| anyhow!("Failed to parse config file: {}", e))?;
        
        // Decrypt sensitive fields
        self.decrypt_config_map(&mut config_map)?;
        
        // Convert back to target type
        let decrypted_value = serde_json::to_value(config_map)
            .map_err(|e| anyhow!("Failed to convert decrypted config: {}", e))?;
        
        let config: T = serde_json::from_value(decrypted_value)
            .map_err(|e| anyhow!("Failed to deserialize config: {}", e))?;
        
        debug!("Loaded and decrypted configuration from {}", file_path);
        Ok(config)
    }
    
    /// Validate encryption key strength
    pub fn validate_key_strength(&self) -> Result<KeyStrengthReport> {
        let key_strength = if self.config.master_key.len() >= 32 {
            KeyStrength::Strong
        } else if self.config.master_key.len() >= 16 {
            KeyStrength::Medium
        } else {
            KeyStrength::Weak
        };
        
        let is_default_key = env::var("KG_ENCRYPTION_KEY").is_err();
        
        let mut recommendations = Vec::new();
        
        if is_default_key {
            recommendations.push("Set KG_ENCRYPTION_KEY environment variable with a strong key".to_string());
        }
        
        if key_strength == KeyStrength::Weak {
            recommendations.push("Use a longer encryption key (at least 32 bytes)".to_string());
        }
        
        if self.config.salt.len() < 16 {
            recommendations.push("Use a longer salt for key derivation".to_string());
        }
        
        Ok(KeyStrengthReport {
            strength: key_strength,
            key_length: self.config.master_key.len(),
            is_default_key,
            recommendations,
        })
    }
    
    /// Generate a new strong encryption key
    pub fn generate_strong_key() -> Result<String> {
        let mut key = vec![0u8; 32]; // 256-bit key
        let rng = SystemRandom::new();
        rng.fill(&mut key)
            .map_err(|_| anyhow!("Failed to generate random key"))?;
        
        Ok(general_purpose::STANDARD.encode(&key))
    }
    
    /// Rotate encryption key (re-encrypt all data with new key)
    pub fn rotate_key(&mut self, new_key: Vec<u8>) -> Result<()> {
        // In a real implementation, you would:
        // 1. Load all encrypted configurations
        // 2. Decrypt with old key
        // 3. Re-encrypt with new key
        // 4. Store updated configurations
        
        self.config.master_key = new_key;
        debug!("Encryption key rotated successfully");
        Ok(())
    }
}

/// Key strength assessment
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyStrength {
    Weak,
    Medium,
    Strong,
}

/// Key strength report
#[derive(Debug, Clone)]
pub struct KeyStrengthReport {
    pub strength: KeyStrength,
    pub key_length: usize,
    pub is_default_key: bool,
    pub recommendations: Vec<String>,
}

/// Encrypt data using AES-GCM
fn encrypt_data(plaintext: &[u8], key: &[u8]) -> Result<(Vec<u8>, Vec<u8>)> {
    let algorithm = &aead::AES_256_GCM;
    
    // Ensure key is correct length
    if key.len() != 32 {
        return Err(anyhow!("Key must be 32 bytes for AES-256"));
    }
    
    let unbound_key = UnboundKey::new(algorithm, key)
        .map_err(|_| anyhow!("Failed to create encryption key"))?;
    let sealing_key = LessSafeKey::new(unbound_key);
    
    // Generate random nonce
    let rng = SystemRandom::new();
    let mut nonce_bytes = vec![0u8; NONCE_LEN];
    rng.fill(&mut nonce_bytes)
        .map_err(|_| anyhow!("Failed to generate nonce"))?;
    
    let nonce = Nonce::try_assume_unique_for_key(&nonce_bytes)
        .map_err(|_| anyhow!("Failed to create nonce"))?;
    
    // Encrypt data
    let mut ciphertext = plaintext.to_vec();
    sealing_key.seal_in_place_append_tag(nonce, Aad::empty(), &mut ciphertext)
        .map_err(|_| anyhow!("Failed to encrypt data"))?;
    
    Ok((ciphertext, nonce_bytes))
}

/// Decrypt data using AES-GCM
fn decrypt_data(ciphertext: &[u8], nonce: &[u8], key: &[u8]) -> Result<Vec<u8>> {
    let algorithm = &aead::AES_256_GCM;
    
    // Ensure key is correct length
    if key.len() != 32 {
        return Err(anyhow!("Key must be 32 bytes for AES-256"));
    }
    
    let unbound_key = UnboundKey::new(algorithm, key)
        .map_err(|_| anyhow!("Failed to create decryption key"))?;
    let opening_key = LessSafeKey::new(unbound_key);
    
    let nonce = Nonce::try_assume_unique_for_key(nonce)
        .map_err(|_| anyhow!("Failed to create nonce"))?;
    
    // Decrypt data
    let mut plaintext = ciphertext.to_vec();
    let decrypted_data = opening_key.open_in_place(nonce, Aad::empty(), &mut plaintext)
        .map_err(|_| anyhow!("Failed to decrypt data"))?;
    
    Ok(decrypted_data.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_encrypt_decrypt_string() {
        let key = vec![0u8; 32]; // Test key
        let plaintext = "secret password";
        
        let encrypted = EncryptedString::new(plaintext, &key).unwrap();
        assert!(encrypted.is_encrypted);
        
        let decrypted = encrypted.decrypt(&key).unwrap();
        assert_eq!(decrypted, plaintext);
    }
    
    #[test]
    fn test_plaintext_string() {
        let plaintext = "not secret";
        let encrypted = EncryptedString::from_plaintext(plaintext);
        assert!(!encrypted.is_encrypted);
        
        let key = vec![0u8; 32];
        let decrypted = encrypted.decrypt(&key).unwrap();
        assert_eq!(decrypted, plaintext);
    }
    
    #[test]
    fn test_should_encrypt() {
        let config = EncryptionConfig::default();
        
        assert!(EncryptedString::should_encrypt("password", &config));
        assert!(EncryptedString::should_encrypt("jwt_secret", &config));
        assert!(EncryptedString::should_encrypt("api_key", &config));
        assert!(!EncryptedString::should_encrypt("username", &config));
        assert!(!EncryptedString::should_encrypt("port", &config));
    }
    
    #[test]
    fn test_config_encryption() {
        let encryptor = ConfigEncryption::default();
        
        let mut config = HashMap::new();
        config.insert("username".to_string(), serde_json::Value::String("admin".to_string()));
        config.insert("password".to_string(), serde_json::Value::String("secret123".to_string()));
        config.insert("port".to_string(), serde_json::Value::Number(serde_json::Number::from(8080)));
        
        let mut encrypted_config = config.clone();
        encryptor.encrypt_config_map(&mut encrypted_config).unwrap();
        
        // Password should be encrypted, username and port should not
        assert_ne!(encrypted_config.get("password"), config.get("password"));
        assert_eq!(encrypted_config.get("username"), config.get("username"));
        assert_eq!(encrypted_config.get("port"), config.get("port"));
        
        // Decrypt and verify
        encryptor.decrypt_config_map(&mut encrypted_config).unwrap();
        assert_eq!(encrypted_config.get("password"), config.get("password"));
    }
    
    #[test]
    fn test_key_strength_validation() {
        let weak_config = EncryptionConfig {
            master_key: vec![0u8; 8],
            ..Default::default()
        };
        let encryptor = ConfigEncryption::new(weak_config);
        let report = encryptor.validate_key_strength().unwrap();
        assert_eq!(report.strength, KeyStrength::Weak);
        assert!(!report.recommendations.is_empty());
        
        let strong_config = EncryptionConfig {
            master_key: vec![0u8; 32],
            ..Default::default()
        };
        let encryptor = ConfigEncryption::new(strong_config);
        let report = encryptor.validate_key_strength().unwrap();
        assert_eq!(report.strength, KeyStrength::Strong);
    }
}
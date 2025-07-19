use anyhow::{Result, anyhow};
use chrono::{Duration, Utc};
use hmac::{Hmac, Mac};
use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, warn, error};
use uuid::Uuid;

use super::rate_limiter::UserTier;

/// Authentication configuration
#[derive(Debug, Clone)]
pub struct AuthConfig {
    /// JWT secret key
    pub jwt_secret: String,
    /// JWT expiration time in hours
    pub jwt_expiration_hours: i64,
    /// API key expiration time in days
    pub api_key_expiration_days: i64,
    /// Enable session-based authentication
    pub enable_sessions: bool,
    /// Session timeout in minutes
    pub session_timeout_minutes: i64,
    /// Maximum concurrent sessions per user
    pub max_sessions_per_user: usize,
    /// Enable API key authentication
    pub enable_api_keys: bool,
    /// Require authentication for all requests
    pub require_authentication: bool,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            jwt_secret: "your-secret-key-change-in-production".to_string(),
            jwt_expiration_hours: 24,
            api_key_expiration_days: 30,
            enable_sessions: true,
            session_timeout_minutes: 60,
            max_sessions_per_user: 5,
            enable_api_keys: true,
            require_authentication: false, // Default to false for development
        }
    }
}

/// User information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: String,
    pub username: String,
    pub email: Option<String>,
    pub tier: UserTier,
    pub permissions: HashSet<Permission>,
    pub created_at: chrono::DateTime<Utc>,
    pub last_login: Option<chrono::DateTime<Utc>>,
    pub is_active: bool,
}

/// Permission types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Permission {
    // Read permissions
    ReadGraph,
    ReadMetrics,
    ReadHealth,
    
    // Write permissions
    WriteGraph,
    ModifyEntities,
    DeleteEntities,
    
    // Analysis permissions
    AnalyzeCode,
    ExportData,
    
    // Administrative permissions
    ManageUsers,
    ManageConfiguration,
    ViewLogs,
    
    // System permissions
    SystemAdministration,
    DatabaseAdministration,
}

/// JWT claims
#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String, // Subject (user ID)
    pub username: String,
    pub tier: UserTier,
    pub permissions: HashSet<Permission>,
    pub exp: i64, // Expiration time
    pub iat: i64, // Issued at
    pub jti: String, // JWT ID
}

/// API key information
#[derive(Debug, Clone)]
pub struct ApiKey {
    pub id: String,
    pub user_id: String,
    pub name: String,
    pub key_hash: String,
    pub permissions: HashSet<Permission>,
    pub created_at: chrono::DateTime<Utc>,
    pub expires_at: Option<chrono::DateTime<Utc>>,
    pub last_used: Option<chrono::DateTime<Utc>>,
    pub is_active: bool,
}

/// Session information
#[derive(Debug, Clone)]
pub struct Session {
    pub id: String,
    pub user_id: String,
    pub created_at: chrono::DateTime<Utc>,
    pub last_activity: chrono::DateTime<Utc>,
    pub ip_address: Option<String>,
    pub user_agent: Option<String>,
    pub is_active: bool,
}

/// Authentication result
#[derive(Debug, Clone)]
pub struct AuthResult {
    pub authenticated: bool,
    pub user: Option<User>,
    pub session_id: Option<String>,
    pub permissions: HashSet<Permission>,
    pub error: Option<String>,
}

/// Authentication token
#[derive(Debug, Clone)]
pub enum AuthToken {
    JWT(String),
    ApiKey(String),
    Session(String),
}

/// Authentication manager
pub struct AuthManager {
    config: AuthConfig,
    encoding_key: EncodingKey,
    decoding_key: DecodingKey,
    
    /// In-memory user store (in production, use a database)
    users: Arc<RwLock<HashMap<String, User>>>,
    
    /// In-memory API key store
    api_keys: Arc<RwLock<HashMap<String, ApiKey>>>,
    
    /// In-memory session store
    sessions: Arc<RwLock<HashMap<String, Session>>>,
    
    /// Revoked JWT IDs
    revoked_tokens: Arc<RwLock<HashSet<String>>>,
}

impl AuthManager {
    /// Create a new authentication manager
    pub async fn new(config: AuthConfig) -> Result<Self> {
        let encoding_key = EncodingKey::from_secret(config.jwt_secret.as_ref());
        let decoding_key = DecodingKey::from_secret(config.jwt_secret.as_ref());
        
        let auth_manager = Self {
            config,
            encoding_key,
            decoding_key,
            users: Arc::new(RwLock::new(HashMap::new())),
            api_keys: Arc::new(RwLock::new(HashMap::new())),
            sessions: Arc::new(RwLock::new(HashMap::new())),
            revoked_tokens: Arc::new(RwLock::new(HashSet::new())),
        };
        
        // Create default admin user for testing
        auth_manager.create_default_admin().await?;
        
        // Start cleanup tasks
        auth_manager.start_cleanup_tasks().await;
        
        Ok(auth_manager)
    }
    
    /// Authenticate a request
    pub async fn authenticate(&self, token: AuthToken) -> AuthResult {
        match token {
            AuthToken::JWT(jwt) => self.authenticate_jwt(&jwt).await,
            AuthToken::ApiKey(api_key) => self.authenticate_api_key(&api_key).await,
            AuthToken::Session(session_id) => self.authenticate_session(&session_id).await,
        }
    }
    
    /// Authenticate using JWT token
    async fn authenticate_jwt(&self, token: &str) -> AuthResult {
        let validation = Validation::new(Algorithm::HS256);
        
        match decode::<Claims>(token, &self.decoding_key, &validation) {
            Ok(token_data) => {
                let claims = token_data.claims;
                
                // Check if token is revoked
                {
                    let revoked = self.revoked_tokens.read().await;
                    if revoked.contains(&claims.jti) {
                        return AuthResult {
                            authenticated: false,
                            user: None,
                            session_id: None,
                            permissions: HashSet::new(),
                            error: Some("Token has been revoked".to_string()),
                        };
                    }
                }
                
                // Get user information
                let users = self.users.read().await;
                if let Some(user) = users.get(&claims.sub) {
                    if user.is_active {
                        AuthResult {
                            authenticated: true,
                            user: Some(user.clone()),
                            session_id: None,
                            permissions: claims.permissions,
                            error: None,
                        }
                    } else {
                        AuthResult {
                            authenticated: false,
                            user: None,
                            session_id: None,
                            permissions: HashSet::new(),
                            error: Some("User account is inactive".to_string()),
                        }
                    }
                } else {
                    AuthResult {
                        authenticated: false,
                        user: None,
                        session_id: None,
                        permissions: HashSet::new(),
                        error: Some("User not found".to_string()),
                    }
                }
            }
            Err(e) => {
                debug!("JWT authentication failed: {}", e);
                AuthResult {
                    authenticated: false,
                    user: None,
                    session_id: None,
                    permissions: HashSet::new(),
                    error: Some("Invalid token".to_string()),
                }
            }
        }
    }
    
    /// Authenticate using API key
    async fn authenticate_api_key(&self, api_key: &str) -> AuthResult {
        let api_key_hash = self.hash_api_key(api_key);
        
        let mut api_keys = self.api_keys.write().await;
        
        // Find API key by hash
        let found_key = api_keys.values_mut().find(|key| {
            key.key_hash == api_key_hash && key.is_active
        });
        
        if let Some(key) = found_key {
            // Check expiration
            if let Some(expires_at) = key.expires_at {
                if Utc::now() > expires_at {
                    return AuthResult {
                        authenticated: false,
                        user: None,
                        session_id: None,
                        permissions: HashSet::new(),
                        error: Some("API key has expired".to_string()),
                    };
                }
            }
            
            // Update last used timestamp
            key.last_used = Some(Utc::now());
            
            // Get user information
            let users = self.users.read().await;
            if let Some(user) = users.get(&key.user_id) {
                if user.is_active {
                    AuthResult {
                        authenticated: true,
                        user: Some(user.clone()),
                        session_id: None,
                        permissions: key.permissions.clone(),
                        error: None,
                    }
                } else {
                    AuthResult {
                        authenticated: false,
                        user: None,
                        session_id: None,
                        permissions: HashSet::new(),
                        error: Some("User account is inactive".to_string()),
                    }
                }
            } else {
                AuthResult {
                    authenticated: false,
                    user: None,
                    session_id: None,
                    permissions: HashSet::new(),
                    error: Some("User not found".to_string()),
                }
            }
        } else {
            AuthResult {
                authenticated: false,
                user: None,
                session_id: None,
                permissions: HashSet::new(),
                error: Some("Invalid API key".to_string()),
            }
        }
    }
    
    /// Authenticate using session
    async fn authenticate_session(&self, session_id: &str) -> AuthResult {
        let mut sessions = self.sessions.write().await;
        
        if let Some(session) = sessions.get_mut(session_id) {
            if !session.is_active {
                return AuthResult {
                    authenticated: false,
                    user: None,
                    session_id: None,
                    permissions: HashSet::new(),
                    error: Some("Session is inactive".to_string()),
                };
            }
            
            // Check session timeout
            let timeout = Duration::minutes(self.config.session_timeout_minutes);
            if Utc::now() - session.last_activity > timeout {
                session.is_active = false;
                return AuthResult {
                    authenticated: false,
                    user: None,
                    session_id: None,
                    permissions: HashSet::new(),
                    error: Some("Session has expired".to_string()),
                };
            }
            
            // Update last activity
            session.last_activity = Utc::now();
            
            // Get user information
            let users = self.users.read().await;
            if let Some(user) = users.get(&session.user_id) {
                if user.is_active {
                    AuthResult {
                        authenticated: true,
                        user: Some(user.clone()),
                        session_id: Some(session_id.to_string()),
                        permissions: user.permissions.clone(),
                        error: None,
                    }
                } else {
                    AuthResult {
                        authenticated: false,
                        user: None,
                        session_id: None,
                        permissions: HashSet::new(),
                        error: Some("User account is inactive".to_string()),
                    }
                }
            } else {
                AuthResult {
                    authenticated: false,
                    user: None,
                    session_id: None,
                    permissions: HashSet::new(),
                    error: Some("User not found".to_string()),
                }
            }
        } else {
            AuthResult {
                authenticated: false,
                user: None,
                session_id: None,
                permissions: HashSet::new(),
                error: Some("Session not found".to_string()),
            }
        }
    }
    
    /// Generate JWT token for user
    pub async fn generate_jwt(&self, user: &User) -> Result<String> {
        let now = Utc::now();
        let exp = now + Duration::hours(self.config.jwt_expiration_hours);
        
        let claims = Claims {
            sub: user.id.clone(),
            username: user.username.clone(),
            tier: user.tier,
            permissions: user.permissions.clone(),
            exp: exp.timestamp(),
            iat: now.timestamp(),
            jti: Uuid::new_v4().to_string(),
        };
        
        encode(&Header::default(), &claims, &self.encoding_key)
            .map_err(|e| anyhow!("Failed to generate JWT: {}", e))
    }
    
    /// Create a new session
    pub async fn create_session(&self, user_id: &str, ip_address: Option<String>, user_agent: Option<String>) -> Result<String> {
        let session_id = Uuid::new_v4().to_string();
        let now = Utc::now();
        
        let session = Session {
            id: session_id.clone(),
            user_id: user_id.to_string(),
            created_at: now,
            last_activity: now,
            ip_address,
            user_agent,
            is_active: true,
        };
        
        // Check session limits
        if self.config.max_sessions_per_user > 0 {
            let mut sessions = self.sessions.write().await;
            let user_sessions: Vec<_> = sessions.values()
                .filter(|s| s.user_id == user_id && s.is_active)
                .cloned()
                .collect();
            
            if user_sessions.len() >= self.config.max_sessions_per_user {
                // Deactivate oldest session
                if let Some(oldest_session) = user_sessions.iter().min_by_key(|s| s.created_at) {
                    if let Some(session) = sessions.get_mut(&oldest_session.id) {
                        session.is_active = false;
                    }
                }
            }
            
            sessions.insert(session_id.clone(), session);
        }
        
        Ok(session_id)
    }
    
    /// Create an API key
    pub async fn create_api_key(&self, user_id: &str, name: &str, permissions: HashSet<Permission>) -> Result<String> {
        let api_key = Uuid::new_v4().to_string();
        let api_key_hash = self.hash_api_key(&api_key);
        let now = Utc::now();
        
        let key_info = ApiKey {
            id: Uuid::new_v4().to_string(),
            user_id: user_id.to_string(),
            name: name.to_string(),
            key_hash: api_key_hash,
            permissions,
            created_at: now,
            expires_at: Some(now + Duration::days(self.config.api_key_expiration_days)),
            last_used: None,
            is_active: true,
        };
        
        let mut api_keys = self.api_keys.write().await;
        api_keys.insert(key_info.id.clone(), key_info);
        
        Ok(api_key)
    }
    
    /// Revoke a JWT token
    pub async fn revoke_jwt(&self, jti: &str) -> Result<()> {
        let mut revoked = self.revoked_tokens.write().await;
        revoked.insert(jti.to_string());
        Ok(())
    }
    
    /// Revoke a session
    pub async fn revoke_session(&self, session_id: &str) -> Result<()> {
        let mut sessions = self.sessions.write().await;
        if let Some(session) = sessions.get_mut(session_id) {
            session.is_active = false;
        }
        Ok(())
    }
    
    /// Check if user has permission
    pub fn has_permission(permissions: &HashSet<Permission>, required: Permission) -> bool {
        permissions.contains(&required) || permissions.contains(&Permission::SystemAdministration)
    }
    
    /// Hash API key
    fn hash_api_key(&self, api_key: &str) -> String {
        let mut mac = Hmac::<Sha256>::new_from_slice(self.config.jwt_secret.as_bytes())
            .expect("HMAC can take key of any size");
        mac.update(api_key.as_bytes());
        let result = mac.finalize();
        base64::encode(result.into_bytes())
    }
    
    /// Create default admin user
    async fn create_default_admin(&self) -> Result<()> {
        let admin_id = "admin-user-id".to_string();
        
        let mut permissions = HashSet::new();
        permissions.insert(Permission::SystemAdministration);
        permissions.insert(Permission::DatabaseAdministration);
        permissions.insert(Permission::ManageUsers);
        permissions.insert(Permission::ManageConfiguration);
        permissions.insert(Permission::ViewLogs);
        permissions.insert(Permission::ReadGraph);
        permissions.insert(Permission::WriteGraph);
        permissions.insert(Permission::ModifyEntities);
        permissions.insert(Permission::DeleteEntities);
        permissions.insert(Permission::AnalyzeCode);
        permissions.insert(Permission::ExportData);
        permissions.insert(Permission::ReadMetrics);
        permissions.insert(Permission::ReadHealth);
        
        let admin_user = User {
            id: admin_id.clone(),
            username: "admin".to_string(),
            email: Some("admin@localhost".to_string()),
            tier: UserTier::Admin,
            permissions,
            created_at: Utc::now(),
            last_login: None,
            is_active: true,
        };
        
        let mut users = self.users.write().await;
        users.insert(admin_id, admin_user);
        
        debug!("Created default admin user");
        Ok(())
    }
    
    /// Start cleanup tasks
    async fn start_cleanup_tasks(&self) {
        let sessions = self.sessions.clone();
        let api_keys = self.api_keys.clone();
        let revoked_tokens = self.revoked_tokens.clone();
        let session_timeout = self.config.session_timeout_minutes;
        
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(300)); // 5 minutes
            
            loop {
                interval.tick().await;
                
                let now = Utc::now();
                
                // Cleanup expired sessions
                {
                    let mut sessions = sessions.write().await;
                    let timeout = Duration::minutes(session_timeout);
                    
                    sessions.retain(|_, session| {
                        session.is_active && (now - session.last_activity) <= timeout
                    });
                }
                
                // Cleanup expired API keys
                {
                    let mut api_keys = api_keys.write().await;
                    api_keys.retain(|_, key| {
                        key.is_active && key.expires_at.map_or(true, |exp| now <= exp)
                    });
                }
                
                // Cleanup old revoked tokens (keep for 24 hours)
                {
                    let mut revoked = revoked_tokens.write().await;
                    // In a real implementation, you would track token expiration times
                    // For now, we'll just clear the set periodically
                    if revoked.len() > 10000 {
                        revoked.clear();
                    }
                }
                
                debug!("Authentication cleanup completed");
            }
        });
    }
    
    /// Get authentication statistics
    pub async fn get_stats(&self) -> AuthStats {
        let users = self.users.read().await;
        let sessions = self.sessions.read().await;
        let api_keys = self.api_keys.read().await;
        let revoked_tokens = self.revoked_tokens.read().await;
        
        let active_sessions = sessions.values().filter(|s| s.is_active).count();
        let active_api_keys = api_keys.values().filter(|k| k.is_active).count();
        
        AuthStats {
            total_users: users.len(),
            active_sessions,
            total_api_keys: api_keys.len(),
            active_api_keys,
            revoked_tokens: revoked_tokens.len(),
        }
    }
}

/// Authentication statistics
#[derive(Debug, Clone, Serialize)]
pub struct AuthStats {
    pub total_users: usize,
    pub active_sessions: usize,
    pub total_api_keys: usize,
    pub active_api_keys: usize,
    pub revoked_tokens: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_jwt_authentication() {
        let config = AuthConfig::default();
        let auth_manager = AuthManager::new(config).await.unwrap();
        
        // Get admin user
        let users = auth_manager.users.read().await;
        let admin_user = users.get("admin-user-id").unwrap().clone();
        drop(users);
        
        // Generate JWT
        let jwt = auth_manager.generate_jwt(&admin_user).await.unwrap();
        
        // Authenticate with JWT
        let result = auth_manager.authenticate(AuthToken::JWT(jwt)).await;
        assert!(result.authenticated);
        assert!(result.user.is_some());
        assert_eq!(result.user.unwrap().username, "admin");
    }
    
    #[tokio::test]
    async fn test_api_key_authentication() {
        let config = AuthConfig::default();
        let auth_manager = AuthManager::new(config).await.unwrap();
        
        let mut permissions = HashSet::new();
        permissions.insert(Permission::ReadGraph);
        
        // Create API key
        let api_key = auth_manager.create_api_key("admin-user-id", "test-key", permissions).await.unwrap();
        
        // Authenticate with API key
        let result = auth_manager.authenticate(AuthToken::ApiKey(api_key)).await;
        assert!(result.authenticated);
        assert!(result.user.is_some());
        assert!(result.permissions.contains(&Permission::ReadGraph));
    }
    
    #[tokio::test]
    async fn test_session_authentication() {
        let config = AuthConfig::default();
        let auth_manager = AuthManager::new(config).await.unwrap();
        
        // Create session
        let session_id = auth_manager.create_session("admin-user-id", None, None).await.unwrap();
        
        // Authenticate with session
        let result = auth_manager.authenticate(AuthToken::Session(session_id)).await;
        assert!(result.authenticated);
        assert!(result.user.is_some());
        assert!(result.session_id.is_some());
    }
    
    #[tokio::test]
    async fn test_permission_check() {
        let mut permissions = HashSet::new();
        permissions.insert(Permission::ReadGraph);
        
        assert!(AuthManager::has_permission(&permissions, Permission::ReadGraph));
        assert!(!AuthManager::has_permission(&permissions, Permission::WriteGraph));
        
        // System admin has all permissions
        let mut admin_permissions = HashSet::new();
        admin_permissions.insert(Permission::SystemAdministration);
        
        assert!(AuthManager::has_permission(&admin_permissions, Permission::WriteGraph));
        assert!(AuthManager::has_permission(&admin_permissions, Permission::DeleteEntities));
    }
}
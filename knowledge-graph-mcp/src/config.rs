use anyhow::{Result, Context};
use serde::{Deserialize, Serialize};
use std::env;
use std::path::Path;
use tracing::{info, debug};

/// Server configuration with environment variable support
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub server: ServerConfig,
    pub neo4j: Neo4jConfig,
    pub redis: RedisConfig,
    pub cache: CacheConfig,
    pub docker: DockerConfig,
    pub analysis: AnalysisConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub name: String,
    pub version: String,
    pub log_level: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Neo4jConfig {
    pub uri: String,
    pub username: String,
    pub password: String,
    pub database: String,
    pub max_connections: u32,
    pub connection_timeout: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RedisConfig {
    pub uri: String,
    pub max_connections: u32,
    pub connection_timeout: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheConfig {
    pub default_ttl: u64,
    pub query_cache_ttl: u64,
    pub session_ttl: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DockerConfig {
    pub neo4j_image: String,
    pub redis_image: String,
    pub neo4j_container_name: String,
    pub redis_container_name: String,
    pub auto_start: bool,
    pub health_check_retries: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisConfig {
    pub supported_languages: Vec<String>,
    pub ignored_directories: Vec<String>,
    pub max_file_size: usize,
    pub batch_size: usize,
}

impl Config {
    /// Load configuration from environment variables with fallback to config file
    pub fn load() -> Result<Self> {
        // First, try to load .env file if it exists
        if Path::new(".env").exists() {
            dotenv::dotenv().ok();
            debug!("Loaded .env file");
        }
        
        // Check if we should load from config file
        let config_path = env::var("CONFIG_PATH")
            .unwrap_or_else(|_| "config/server_config.toml".to_string());
        
        let mut config = if Path::new(&config_path).exists() {
            Self::from_file(&config_path)?
        } else {
            Self::default()
        };
        
        // Override with environment variables
        config.override_from_env();
        
        // Validate configuration
        config.validate()?;
        
        info!("Configuration loaded successfully");
        Ok(config)
    }
    
    /// Load configuration from TOML file
    fn from_file(path: &str) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read config file: {}", path))?;
        
        toml::from_str(&content)
            .with_context(|| format!("Failed to parse config file: {}", path))
    }
    
    /// Override configuration values from environment variables
    fn override_from_env(&mut self) {
        // Server configuration
        if let Ok(val) = env::var("LOG_LEVEL") {
            self.server.log_level = val;
        }
        
        // Neo4j configuration
        if let Ok(val) = env::var("NEO4J_URI") {
            self.neo4j.uri = val;
        }
        if let Ok(val) = env::var("NEO4J_USERNAME") {
            self.neo4j.username = val;
        }
        if let Ok(val) = env::var("NEO4J_PASSWORD") {
            self.neo4j.password = val;
        }
        if let Ok(val) = env::var("NEO4J_DATABASE") {
            self.neo4j.database = val;
        }
        if let Ok(val) = env::var("NEO4J_MAX_CONNECTIONS") {
            if let Ok(num) = val.parse() {
                self.neo4j.max_connections = num;
            }
        }
        
        // Redis configuration
        if let Ok(val) = env::var("REDIS_URI") {
            self.redis.uri = val;
        }
        if let Ok(val) = env::var("REDIS_MAX_CONNECTIONS") {
            if let Ok(num) = val.parse() {
                self.redis.max_connections = num;
            }
        }
        
        // Cache configuration
        if let Ok(val) = env::var("CACHE_DEFAULT_TTL") {
            if let Ok(num) = val.parse() {
                self.cache.default_ttl = num;
            }
        }
        if let Ok(val) = env::var("CACHE_QUERY_TTL") {
            if let Ok(num) = val.parse() {
                self.cache.query_cache_ttl = num;
            }
        }
        
        // Docker configuration
        if let Ok(val) = env::var("DOCKER_AUTO_START") {
            self.docker.auto_start = val.to_lowercase() == "true" || val == "1";
        }
        if let Ok(val) = env::var("NEO4J_IMAGE") {
            self.docker.neo4j_image = val;
        }
        if let Ok(val) = env::var("REDIS_IMAGE") {
            self.docker.redis_image = val;
        }
        
        // Analysis configuration
        if let Ok(val) = env::var("MAX_FILE_SIZE") {
            if let Ok(num) = val.parse() {
                self.analysis.max_file_size = num;
            }
        }
        if let Ok(val) = env::var("IGNORED_DIRECTORIES") {
            self.analysis.ignored_directories = val.split(',').map(String::from).collect();
        }
    }
    
    /// Validate configuration values
    fn validate(&self) -> Result<()> {
        // Validate required fields
        if self.neo4j.password.is_empty() {
            return Err(anyhow::anyhow!(
                "Neo4j password is required. Set NEO4J_PASSWORD environment variable."
            ));
        }
        
        // Validate numeric ranges
        if self.neo4j.max_connections == 0 {
            return Err(anyhow::anyhow!("Neo4j max_connections must be greater than 0"));
        }
        
        if self.redis.max_connections == 0 {
            return Err(anyhow::anyhow!("Redis max_connections must be greater than 0"));
        }
        
        if self.cache.default_ttl == 0 {
            return Err(anyhow::anyhow!("Cache TTL values must be greater than 0"));
        }
        
        Ok(())
    }
    
    /// Get a summary of the configuration for logging
    pub fn summary(&self) -> String {
        format!(
            "Config: Neo4j={}, Redis={}, AutoStart={}, LogLevel={}",
            self.neo4j.uri,
            self.redis.uri,
            self.docker.auto_start,
            self.server.log_level
        )
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            server: ServerConfig {
                name: "knowledge-graph-mcp".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
                log_level: "info".to_string(),
            },
            neo4j: Neo4jConfig {
                uri: "neo4j://localhost:7687".to_string(),
                username: "neo4j".to_string(),
                password: "knowledge-graph-2024".to_string(),
                database: "neo4j".to_string(),
                max_connections: 10,
                connection_timeout: 30,
            },
            redis: RedisConfig {
                uri: "redis://127.0.0.1:6379".to_string(),
                max_connections: 10,
                connection_timeout: 10,
            },
            cache: CacheConfig {
                default_ttl: 3600,
                query_cache_ttl: 1800,
                session_ttl: 86400,
            },
            docker: DockerConfig {
                neo4j_image: "neo4j:5.15-community".to_string(),
                redis_image: "redis:7-alpine".to_string(),
                neo4j_container_name: "knowledge-graph-neo4j".to_string(),
                redis_container_name: "knowledge-graph-redis".to_string(),
                auto_start: true,
                health_check_retries: 30,
            },
            analysis: AnalysisConfig {
                supported_languages: vec![
                    "rust".to_string(),
                    "javascript".to_string(),
                    "typescript".to_string(),
                    "python".to_string(),
                ],
                ignored_directories: vec![
                    "node_modules".to_string(),
                    "target".to_string(),
                    ".git".to_string(),
                    "dist".to_string(),
                    "build".to_string(),
                    "__pycache__".to_string(),
                ],
                max_file_size: 1048576, // 1MB
                batch_size: 100,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.server.name, "knowledge-graph-mcp");
        assert_eq!(config.neo4j.uri, "neo4j://localhost:7687");
        assert_eq!(config.redis.uri, "redis://127.0.0.1:6379");
        assert!(config.docker.auto_start);
    }
    
    #[test]
    fn test_env_override() {
        env::set_var("NEO4J_URI", "neo4j://test:7687");
        env::set_var("REDIS_URI", "redis://test:6379");
        env::set_var("LOG_LEVEL", "debug");
        
        let mut config = Config::default();
        config.override_from_env();
        
        assert_eq!(config.neo4j.uri, "neo4j://test:7687");
        assert_eq!(config.redis.uri, "redis://test:6379");
        assert_eq!(config.server.log_level, "debug");
        
        // Clean up
        env::remove_var("NEO4J_URI");
        env::remove_var("REDIS_URI");
        env::remove_var("LOG_LEVEL");
    }
    
    #[test]
    fn test_validation() {
        let mut config = Config::default();
        assert!(config.validate().is_ok());
        
        config.neo4j.password = String::new();
        assert!(config.validate().is_err());
        
        config.neo4j.password = "test".to_string();
        config.neo4j.max_connections = 0;
        assert!(config.validate().is_err());
    }
}
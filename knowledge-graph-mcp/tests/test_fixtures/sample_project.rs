// Sample Rust project for testing the knowledge graph

use std::collections::HashMap;

/// Configuration structure for the application
#[derive(Debug, Clone)]
pub struct Config {
    pub database_url: String,
    pub port: u16,
    pub workers: usize,
}

impl Config {
    /// Creates a new configuration with default values
    pub fn new() -> Self {
        Self {
            database_url: "postgres://localhost/myapp".to_string(),
            port: 8080,
            workers: 4,
        }
    }
    
    /// Loads configuration from environment variables
    pub fn from_env() -> Result<Self, String> {
        Ok(Self {
            database_url: std::env::var("DATABASE_URL")
                .map_err(|_| "DATABASE_URL not set")?,
            port: std::env::var("PORT")
                .unwrap_or_else(|_| "8080".to_string())
                .parse()
                .map_err(|_| "Invalid PORT")?,
            workers: num_cpus::get(),
        })
    }
}

/// Main application structure
pub struct App {
    config: Config,
    routes: HashMap<String, fn()>,
}

impl App {
    /// Creates a new application instance
    pub fn new(config: Config) -> Self {
        let mut routes = HashMap::new();
        routes.insert("/".to_string(), index_handler as fn());
        routes.insert("/api/users".to_string(), users_handler as fn());
        
        Self { config, routes }
    }
    
    /// Starts the application server
    pub async fn run(&self) -> Result<(), Box<dyn std::error::Error>> {
        println!("Starting server on port {}", self.config.port);
        // Server implementation would go here
        Ok(())
    }
    
    /// Registers a new route handler
    pub fn route(&mut self, path: &str, handler: fn()) {
        self.routes.insert(path.to_string(), handler);
    }
}

/// Index page handler
fn index_handler() {
    println!("Welcome to the application!");
}

/// Users API handler
fn users_handler() {
    println!("Users endpoint");
}

/// Entry point for the application
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::from_env().unwrap_or_else(|_| Config::new());
    let app = App::new(config);
    app.run().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_config_new() {
        let config = Config::new();
        assert_eq!(config.port, 8080);
        assert_eq!(config.workers, 4);
    }
    
    #[test]
    fn test_app_creation() {
        let config = Config::new();
        let app = App::new(config);
        assert_eq!(app.routes.len(), 2);
    }
}
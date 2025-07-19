use anyhow::Error;
use std::fmt;

/// User-friendly error types for better error messages
#[derive(Debug)]
pub enum UserError {
    DockerNotRunning,
    Neo4jConnectionFailed,
    RedisConnectionFailed,
    FileNotFound(String),
    InvalidConfiguration(String),
    QueryTimeout,
    ResourceNotFound(String),
    PermissionDenied(String),
    ServiceUnavailable(String),
}

impl fmt::Display for UserError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UserError::DockerNotRunning => write!(
                f,
                "Docker is not running. Please start Docker Desktop and try again.\n\
                 On macOS: Open Docker Desktop from Applications\n\
                 On Linux: Run 'sudo systemctl start docker'\n\
                 On Windows: Start Docker Desktop from the Start Menu"
            ),
            UserError::Neo4jConnectionFailed => write!(
                f,
                "Cannot connect to Neo4j database. The server will attempt to auto-start it.\n\
                 If this persists, check:\n\
                 - Port 7687 is not in use: lsof -i :7687\n\
                 - Neo4j container logs: docker logs knowledge-graph-neo4j"
            ),
            UserError::RedisConnectionFailed => write!(
                f,
                "Cannot connect to Redis cache. The server will attempt to auto-start it.\n\
                 If this persists, check:\n\
                 - Port 6379 is not in use: lsof -i :6379\n\
                 - Redis container logs: docker logs knowledge-graph-redis"
            ),
            UserError::FileNotFound(path) => write!(
                f,
                "File not found: {}\n\
                 Please check the file path and ensure it exists."
                , path
            ),
            UserError::InvalidConfiguration(msg) => write!(
                f,
                "Invalid configuration: {}\n\
                 Check your environment variables or config file."
                , msg
            ),
            UserError::QueryTimeout => write!(
                f,
                "Query timed out. This might be due to:\n\
                 - Complex query requiring optimization\n\
                 - Large dataset being processed\n\
                 - Database performance issues\n\
                 Try simplifying your query or increasing the timeout."
            ),
            UserError::ResourceNotFound(resource) => write!(
                f,
                "Resource not found: {}\n\
                 The requested resource does not exist or has been moved."
                , resource
            ),
            UserError::PermissionDenied(action) => write!(
                f,
                "Permission denied: {}\n\
                 You don't have the necessary permissions for this operation."
                , action
            ),
            UserError::ServiceUnavailable(service) => write!(
                f,
                "Service unavailable: {}\n\
                 The service is temporarily unavailable. Please try again later."
                , service
            ),
        }
    }
}

impl std::error::Error for UserError {}

/// Convert internal errors to user-friendly messages
pub fn format_user_error(error: &Error) -> String {
    // Check for specific error types and provide helpful messages
    let error_string = error.to_string();
    
    // Docker errors
    if error_string.contains("Docker") || error_string.contains("docker") {
        return UserError::DockerNotRunning.to_string();
    }
    
    // Neo4j connection errors
    if error_string.contains("Neo4j") || error_string.contains("neo4j") || error_string.contains("7687") {
        return UserError::Neo4jConnectionFailed.to_string();
    }
    
    // Redis connection errors
    if error_string.contains("Redis") || error_string.contains("redis") || error_string.contains("6379") {
        return UserError::RedisConnectionFailed.to_string();
    }
    
    // File system errors
    if error_string.contains("No such file") || error_string.contains("not found") {
        if let Some(path) = extract_path_from_error(&error_string) {
            return UserError::FileNotFound(path).to_string();
        }
    }
    
    // Timeout errors
    if error_string.contains("timeout") || error_string.contains("timed out") {
        return UserError::QueryTimeout.to_string();
    }
    
    // Permission errors
    if error_string.contains("permission") || error_string.contains("denied") {
        return UserError::PermissionDenied("the requested operation".to_string()).to_string();
    }
    
    // Configuration errors
    if error_string.contains("config") || error_string.contains("environment") {
        return UserError::InvalidConfiguration(error_string.clone()).to_string();
    }
    
    // Default: return the original error with some context
    format!(
        "An error occurred: {}\n\n\
         If this error persists, please check:\n\
         - All services are running (use 'health_check' tool)\n\
         - Your configuration is correct\n\
         - You have necessary permissions",
        error
    )
}

/// Extract file path from error message
fn extract_path_from_error(error_msg: &str) -> Option<String> {
    // Try to extract path from common error patterns
    if let Some(start) = error_msg.find("'") {
        if let Some(end) = error_msg[start + 1..].find("'") {
            return Some(error_msg[start + 1..start + 1 + end].to_string());
        }
    }
    
    if let Some(start) = error_msg.find(": ") {
        let path_candidate = &error_msg[start + 2..];
        if path_candidate.contains('/') || path_candidate.contains('\\') {
            return Some(path_candidate.split_whitespace().next()?.to_string());
        }
    }
    
    None
}

/// Create a user-friendly error response for MCP
pub fn create_error_response(error: &Error) -> serde_json::Value {
    let user_message = format_user_error(error);
    
    serde_json::json!({
        "error": {
            "message": user_message,
            "type": classify_error(error),
            "recoverable": is_recoverable_error(error)
        }
    })
}

/// Classify error type for better handling
fn classify_error(error: &Error) -> &'static str {
    let error_string = error.to_string();
    
    if error_string.contains("Docker") || error_string.contains("docker") {
        "infrastructure"
    } else if error_string.contains("Neo4j") || error_string.contains("Redis") {
        "service"
    } else if error_string.contains("file") || error_string.contains("path") {
        "filesystem"
    } else if error_string.contains("timeout") {
        "timeout"
    } else if error_string.contains("permission") {
        "permission"
    } else if error_string.contains("config") {
        "configuration"
    } else {
        "unknown"
    }
}

/// Determine if an error is recoverable
fn is_recoverable_error(error: &Error) -> bool {
    let error_string = error.to_string();
    
    // These errors are typically recoverable
    error_string.contains("timeout") ||
    error_string.contains("connection") ||
    error_string.contains("temporarily") ||
    error_string.contains("retry")
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_docker_error_formatting() {
        let error = anyhow::anyhow!("Failed to connect to Docker daemon");
        let formatted = format_user_error(&error);
        assert!(formatted.contains("Docker is not running"));
        assert!(formatted.contains("Please start Docker"));
    }
    
    #[test]
    fn test_neo4j_error_formatting() {
        let error = anyhow::anyhow!("Connection refused: neo4j://localhost:7687");
        let formatted = format_user_error(&error);
        assert!(formatted.contains("Cannot connect to Neo4j"));
        assert!(formatted.contains("auto-start"));
    }
    
    #[test]
    fn test_file_not_found_error() {
        let error = anyhow::anyhow!("No such file or directory: '/path/to/file.rs'");
        let formatted = format_user_error(&error);
        assert!(formatted.contains("File not found"));
    }
    
    #[test]
    fn test_error_classification() {
        let docker_error = anyhow::anyhow!("Docker connection failed");
        assert_eq!(classify_error(&docker_error), "infrastructure");
        
        let timeout_error = anyhow::anyhow!("Operation timed out");
        assert_eq!(classify_error(&timeout_error), "timeout");
    }
}
use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use tracing::{debug, warn};

/// Request validation configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestValidatorConfig {
    /// Maximum request body size in bytes
    pub max_request_size: usize,
    /// Maximum query string length
    pub max_query_length: usize,
    /// Maximum number of parameters
    pub max_parameters: usize,
    /// Maximum parameter name length
    pub max_parameter_name_length: usize,
    /// Maximum parameter value length
    pub max_parameter_value_length: usize,
    /// Maximum array parameter size
    pub max_array_size: usize,
    /// Maximum object nesting depth
    pub max_nesting_depth: usize,
    /// Maximum string length in any field
    pub max_string_length: usize,
    /// Maximum number of files in batch operations
    pub max_batch_size: usize,
    /// Enable strict validation
    pub strict_validation: bool,
}

impl Default for RequestValidatorConfig {
    fn default() -> Self {
        Self {
            max_request_size: 10 * 1024 * 1024, // 10MB
            max_query_length: 50000, // 50KB
            max_parameters: 100,
            max_parameter_name_length: 100,
            max_parameter_value_length: 10000,
            max_array_size: 1000,
            max_nesting_depth: 10,
            max_string_length: 100000,
            max_batch_size: 100,
            strict_validation: true,
        }
    }
}

/// Validation result
#[derive(Debug, Clone)]
pub struct ValidationResult {
    pub valid: bool,
    pub issues: Vec<ValidationIssue>,
    pub sanitized_data: Option<Value>,
}

/// Validation issue
#[derive(Debug, Clone)]
pub struct ValidationIssue {
    pub field: Option<String>,
    pub issue_type: ValidationIssueType,
    pub message: String,
    pub severity: ValidationSeverity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidationIssueType {
    SizeLimit,
    CountLimit,
    DepthLimit,
    FormatError,
    SecurityViolation,
    DataIntegrity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ValidationSeverity {
    Info,
    Warning,
    Error,
    Critical,
}

/// Request validator
pub struct RequestValidator {
    config: RequestValidatorConfig,
}

impl RequestValidator {
    /// Create a new request validator
    pub fn new(config: RequestValidatorConfig) -> Self {
        Self { config }
    }
    
    /// Create validator with default configuration
    pub fn default() -> Self {
        Self::new(RequestValidatorConfig::default())
    }
    
    /// Validate raw request data
    pub fn validate_request(&self, data: &[u8]) -> ValidationResult {
        let mut issues = Vec::new();
        
        // Check request size
        if data.len() > self.config.max_request_size {
            issues.push(ValidationIssue {
                field: None,
                issue_type: ValidationIssueType::SizeLimit,
                message: format!(
                    "Request size ({} bytes) exceeds maximum allowed ({} bytes)",
                    data.len(),
                    self.config.max_request_size
                ),
                severity: ValidationSeverity::Error,
            });
            
            return ValidationResult {
                valid: false,
                issues,
                sanitized_data: None,
            };
        }
        
        // Try to parse JSON
        match serde_json::from_slice::<Value>(data) {
            Ok(json_value) => {
                let mut validation_result = self.validate_json_value(&json_value, "", 0);
                validation_result.sanitized_data = Some(json_value);
                validation_result
            }
            Err(e) => {
                issues.push(ValidationIssue {
                    field: None,
                    issue_type: ValidationIssueType::FormatError,
                    message: format!("Invalid JSON format: {}", e),
                    severity: ValidationSeverity::Error,
                });
                
                ValidationResult {
                    valid: false,
                    issues,
                    sanitized_data: None,
                }
            }
        }
    }
    
    /// Validate JSON value recursively
    pub fn validate_json_value(&self, value: &Value, field_path: &str, depth: usize) -> ValidationResult {
        let mut issues = Vec::new();
        let mut valid = true;
        
        // Check nesting depth
        if depth > self.config.max_nesting_depth {
            issues.push(ValidationIssue {
                field: Some(field_path.to_string()),
                issue_type: ValidationIssueType::DepthLimit,
                message: format!(
                    "Nesting depth ({}) exceeds maximum allowed ({})",
                    depth,
                    self.config.max_nesting_depth
                ),
                severity: ValidationSeverity::Error,
            });
            valid = false;
        }
        
        match value {
            Value::String(s) => {
                if s.len() > self.config.max_string_length {
                    issues.push(ValidationIssue {
                        field: Some(field_path.to_string()),
                        issue_type: ValidationIssueType::SizeLimit,
                        message: format!(
                            "String length ({}) exceeds maximum allowed ({})",
                            s.len(),
                            self.config.max_string_length
                        ),
                        severity: ValidationSeverity::Error,
                    });
                    valid = false;
                }
                
                // Check for potential security issues
                if self.contains_suspicious_content(s) {
                    issues.push(ValidationIssue {
                        field: Some(field_path.to_string()),
                        issue_type: ValidationIssueType::SecurityViolation,
                        message: "String contains potentially malicious content".to_string(),
                        severity: ValidationSeverity::Warning,
                    });
                }
            }
            
            Value::Array(arr) => {
                if arr.len() > self.config.max_array_size {
                    issues.push(ValidationIssue {
                        field: Some(field_path.to_string()),
                        issue_type: ValidationIssueType::CountLimit,
                        message: format!(
                            "Array size ({}) exceeds maximum allowed ({})",
                            arr.len(),
                            self.config.max_array_size
                        ),
                        severity: ValidationSeverity::Error,
                    });
                    valid = false;
                }
                
                // Validate array elements
                for (i, item) in arr.iter().enumerate() {
                    let item_path = if field_path.is_empty() {
                        format!("[{}]", i)
                    } else {
                        format!("{}[{}]", field_path, i)
                    };
                    
                    let item_result = self.validate_json_value(item, &item_path, depth + 1);
                    if !item_result.valid {
                        valid = false;
                    }
                    issues.extend(item_result.issues);
                }
            }
            
            Value::Object(obj) => {
                if obj.len() > self.config.max_parameters {
                    issues.push(ValidationIssue {
                        field: Some(field_path.to_string()),
                        issue_type: ValidationIssueType::CountLimit,
                        message: format!(
                            "Object parameter count ({}) exceeds maximum allowed ({})",
                            obj.len(),
                            self.config.max_parameters
                        ),
                        severity: ValidationSeverity::Error,
                    });
                    valid = false;
                }
                
                // Validate object properties
                for (key, val) in obj {
                    // Validate key length
                    if key.len() > self.config.max_parameter_name_length {
                        issues.push(ValidationIssue {
                            field: Some(key.clone()),
                            issue_type: ValidationIssueType::SizeLimit,
                            message: format!(
                                "Parameter name length ({}) exceeds maximum allowed ({})",
                                key.len(),
                                self.config.max_parameter_name_length
                            ),
                            severity: ValidationSeverity::Error,
                        });
                        valid = false;
                    }
                    
                    // Check for suspicious parameter names
                    if self.contains_suspicious_parameter_name(key) {
                        issues.push(ValidationIssue {
                            field: Some(key.clone()),
                            issue_type: ValidationIssueType::SecurityViolation,
                            message: "Parameter name contains potentially malicious pattern".to_string(),
                            severity: ValidationSeverity::Warning,
                        });
                    }
                    
                    let prop_path = if field_path.is_empty() {
                        key.clone()
                    } else {
                        format!("{}.{}", field_path, key)
                    };
                    
                    let prop_result = self.validate_json_value(val, &prop_path, depth + 1);
                    if !prop_result.valid {
                        valid = false;
                    }
                    issues.extend(prop_result.issues);
                }
            }
            
            Value::Number(num) => {
                // Check for extremely large numbers that might cause issues
                if let Some(f) = num.as_f64() {
                    if f.is_infinite() || f.is_nan() {
                        issues.push(ValidationIssue {
                            field: Some(field_path.to_string()),
                            issue_type: ValidationIssueType::DataIntegrity,
                            message: "Number value is infinite or NaN".to_string(),
                            severity: ValidationSeverity::Error,
                        });
                        valid = false;
                    } else if f.abs() > 1e20 {
                        issues.push(ValidationIssue {
                            field: Some(field_path.to_string()),
                            issue_type: ValidationIssueType::DataIntegrity,
                            message: "Number value is extremely large".to_string(),
                            severity: ValidationSeverity::Warning,
                        });
                    }
                }
            }
            
            _ => {} // Bool and Null are always valid
        }
        
        ValidationResult {
            valid,
            issues,
            sanitized_data: None,
        }
    }
    
    /// Validate specific tool parameters
    pub fn validate_tool_parameters(&self, tool_name: &str, parameters: &Value) -> ValidationResult {
        let mut result = self.validate_json_value(parameters, "parameters", 0);
        
        // Tool-specific validation
        match tool_name {
            "batch_analyze" => {
                self.validate_batch_analyze_params(parameters, &mut result);
            }
            "export_graph" => {
                self.validate_export_graph_params(parameters, &mut result);
            }
            "query_knowledge" => {
                self.validate_query_knowledge_params(parameters, &mut result);
            }
            "analyze_code_structure" => {
                self.validate_analyze_code_params(parameters, &mut result);
            }
            _ => {
                // Generic validation for unknown tools
                debug!("No specific validation for tool: {}", tool_name);
            }
        }
        
        result
    }
    
    /// Validate batch_analyze parameters
    fn validate_batch_analyze_params(&self, params: &Value, result: &mut ValidationResult) {
        if let Some(paths) = params.get("paths").and_then(|p| p.as_array()) {
            if paths.len() > self.config.max_batch_size {
                result.valid = false;
                result.issues.push(ValidationIssue {
                    field: Some("paths".to_string()),
                    issue_type: ValidationIssueType::CountLimit,
                    message: format!(
                        "Batch size ({}) exceeds maximum allowed ({})",
                        paths.len(),
                        self.config.max_batch_size
                    ),
                    severity: ValidationSeverity::Error,
                });
            }
            
            // Validate individual paths
            for (i, path) in paths.iter().enumerate() {
                if let Some(path_str) = path.as_str() {
                    if self.contains_suspicious_path(path_str) {
                        result.issues.push(ValidationIssue {
                            field: Some(format!("paths[{}]", i)),
                            issue_type: ValidationIssueType::SecurityViolation,
                            message: "Path contains potentially dangerous patterns".to_string(),
                            severity: ValidationSeverity::Warning,
                        });
                    }
                }
            }
        }
    }
    
    /// Validate export_graph parameters
    fn validate_export_graph_params(&self, params: &Value, result: &mut ValidationResult) {
        if let Some(format) = params.get("format").and_then(|f| f.as_str()) {
            let valid_formats = ["json", "dot", "mermaid"];
            if !valid_formats.contains(&format) {
                result.valid = false;
                result.issues.push(ValidationIssue {
                    field: Some("format".to_string()),
                    issue_type: ValidationIssueType::FormatError,
                    message: format!("Invalid format '{}'. Allowed: {:?}", format, valid_formats),
                    severity: ValidationSeverity::Error,
                });
            }
        }
    }
    
    /// Validate query_knowledge parameters
    fn validate_query_knowledge_params(&self, params: &Value, result: &mut ValidationResult) {
        if let Some(query) = params.get("query").and_then(|q| q.as_str()) {
            if query.len() > self.config.max_query_length {
                result.valid = false;
                result.issues.push(ValidationIssue {
                    field: Some("query".to_string()),
                    issue_type: ValidationIssueType::SizeLimit,
                    message: format!(
                        "Query length ({}) exceeds maximum allowed ({})",
                        query.len(),
                        self.config.max_query_length
                    ),
                    severity: ValidationSeverity::Error,
                });
            }
        }
    }
    
    /// Validate analyze_code_structure parameters
    fn validate_analyze_code_params(&self, params: &Value, result: &mut ValidationResult) {
        if let Some(path) = params.get("path").and_then(|p| p.as_str()) {
            if self.contains_suspicious_path(path) {
                result.issues.push(ValidationIssue {
                    field: Some("path".to_string()),
                    issue_type: ValidationIssueType::SecurityViolation,
                    message: "Path contains potentially dangerous patterns".to_string(),
                    severity: ValidationSeverity::Warning,
                });
            }
        }
    }
    
    /// Check if string contains suspicious content
    fn contains_suspicious_content(&self, content: &str) -> bool {
        let suspicious_patterns = [
            "<script",
            "javascript:",
            "data:",
            "vbscript:",
            "onload=",
            "onerror=",
            "eval(",
            "function(",
            "${",
            "<%",
            "<?",
            "DROP TABLE",
            "DELETE FROM",
            "INSERT INTO",
            "UPDATE SET",
            "../",
            "..\\",
            "/etc/",
            "C:\\",
            "\x00", // Null byte
        ];
        
        let content_lower = content.to_lowercase();
        suspicious_patterns.iter().any(|pattern| content_lower.contains(pattern))
    }
    
    /// Check if parameter name contains suspicious patterns
    fn contains_suspicious_parameter_name(&self, name: &str) -> bool {
        let suspicious_patterns = [
            "__",
            "..",
            "/",
            "\\",
            "eval",
            "exec",
            "system",
            "cmd",
            "shell",
            "script",
        ];
        
        let name_lower = name.to_lowercase();
        suspicious_patterns.iter().any(|pattern| name_lower.contains(pattern))
    }
    
    /// Check if path contains suspicious patterns
    fn contains_suspicious_path(&self, path: &str) -> bool {
        let suspicious_patterns = [
            "../",
            "..\\",
            "/etc/",
            "/proc/",
            "/sys/",
            "C:\\Windows\\",
            "C:\\Program Files\\",
            "/root/",
            "/home/",
            "\x00", // Null byte
        ];
        
        suspicious_patterns.iter().any(|pattern| path.contains(pattern))
    }
    
    /// Sanitize string content
    pub fn sanitize_string(&self, content: &str) -> String {
        // Remove null bytes and control characters
        content
            .chars()
            .filter(|c| !c.is_control() || *c == '\n' || *c == '\r' || *c == '\t')
            .collect()
    }
    
    /// Get validation statistics
    pub fn get_validation_stats(&self) -> ValidationStats {
        ValidationStats {
            max_request_size: self.config.max_request_size,
            max_query_length: self.config.max_query_length,
            max_parameters: self.config.max_parameters,
            max_batch_size: self.config.max_batch_size,
            strict_validation: self.config.strict_validation,
        }
    }
}

/// Validation statistics
#[derive(Debug, Clone, Serialize)]
pub struct ValidationStats {
    pub max_request_size: usize,
    pub max_query_length: usize,
    pub max_parameters: usize,
    pub max_batch_size: usize,
    pub strict_validation: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    
    #[test]
    fn test_request_size_validation() {
        let config = RequestValidatorConfig {
            max_request_size: 100,
            ..Default::default()
        };
        let validator = RequestValidator::new(config);
        
        let small_data = b"{}";
        let result = validator.validate_request(small_data);
        assert!(result.valid);
        
        let large_data = vec![b'a'; 200];
        let result = validator.validate_request(&large_data);
        assert!(!result.valid);
        assert!(!result.issues.is_empty());
    }
    
    #[test]
    fn test_json_validation() {
        let validator = RequestValidator::default();
        
        let valid_json = json!({
            "name": "test",
            "value": 42,
            "active": true
        });
        
        let result = validator.validate_json_value(&valid_json, "", 0);
        assert!(result.valid);
    }
    
    #[test]
    fn test_array_size_limit() {
        let config = RequestValidatorConfig {
            max_array_size: 2,
            ..Default::default()
        };
        let validator = RequestValidator::new(config);
        
        let large_array = json!([1, 2, 3, 4, 5]);
        let result = validator.validate_json_value(&large_array, "test_array", 0);
        assert!(!result.valid);
        assert!(result.issues.iter().any(|i| i.issue_type == ValidationIssueType::CountLimit));
    }
    
    #[test]
    fn test_nesting_depth_limit() {
        let config = RequestValidatorConfig {
            max_nesting_depth: 2,
            ..Default::default()
        };
        let validator = RequestValidator::new(config);
        
        let deep_object = json!({
            "level1": {
                "level2": {
                    "level3": {
                        "level4": "too_deep"
                    }
                }
            }
        });
        
        let result = validator.validate_json_value(&deep_object, "", 0);
        assert!(!result.valid);
        assert!(result.issues.iter().any(|i| i.issue_type == ValidationIssueType::DepthLimit));
    }
    
    #[test]
    fn test_suspicious_content_detection() {
        let validator = RequestValidator::default();
        
        assert!(validator.contains_suspicious_content("<script>alert('xss')</script>"));
        assert!(validator.contains_suspicious_content("javascript:alert(1)"));
        assert!(validator.contains_suspicious_content("../../etc/passwd"));
        assert!(!validator.contains_suspicious_content("normal content"));
    }
    
    #[test]
    fn test_tool_parameter_validation() {
        let validator = RequestValidator::default();
        
        let params = json!({
            "query": "MATCH (n) RETURN n"
        });
        
        let result = validator.validate_tool_parameters("query_knowledge", &params);
        assert!(result.valid);
        
        // Test with too long query
        let long_query = "A".repeat(100000);
        let params = json!({
            "query": long_query
        });
        
        let result = validator.validate_tool_parameters("query_knowledge", &params);
        assert!(!result.valid);
    }
}
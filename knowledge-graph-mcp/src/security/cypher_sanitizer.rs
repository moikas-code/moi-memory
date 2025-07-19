use anyhow::{Result, anyhow};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use tracing::{debug, warn, error};

/// Configuration for Cypher query sanitization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CypherSanitizerConfig {
    /// Maximum query length in characters
    pub max_query_length: usize,
    /// Maximum depth of nested clauses
    pub max_clause_depth: usize,
    /// Maximum number of MATCH clauses
    pub max_match_clauses: usize,
    /// Maximum number of CREATE clauses
    pub max_create_clauses: usize,
    /// Maximum number of DELETE clauses
    pub max_delete_clauses: usize,
    /// Allow dangerous operations (DELETE, DROP, etc.)
    pub allow_dangerous_operations: bool,
    /// Allow schema modifications
    pub allow_schema_modifications: bool,
    /// Allow administrative commands
    pub allow_admin_commands: bool,
    /// Whitelist of allowed functions
    pub allowed_functions: HashSet<String>,
    /// Blacklist of forbidden patterns
    pub forbidden_patterns: Vec<String>,
    /// Enable parameter validation
    pub validate_parameters: bool,
}

impl Default for CypherSanitizerConfig {
    fn default() -> Self {
        let mut allowed_functions = HashSet::new();
        // Safe read-only functions
        allowed_functions.insert("count".to_string());
        allowed_functions.insert("size".to_string());
        allowed_functions.insert("length".to_string());
        allowed_functions.insert("id".to_string());
        allowed_functions.insert("type".to_string());
        allowed_functions.insert("labels".to_string());
        allowed_functions.insert("keys".to_string());
        allowed_functions.insert("properties".to_string());
        allowed_functions.insert("head".to_string());
        allowed_functions.insert("tail".to_string());
        allowed_functions.insert("last".to_string());
        allowed_functions.insert("collect".to_string());
        allowed_functions.insert("distinct".to_string());
        allowed_functions.insert("reverse".to_string());
        allowed_functions.insert("substring".to_string());
        allowed_functions.insert("toLower".to_string());
        allowed_functions.insert("toUpper".to_string());
        allowed_functions.insert("trim".to_string());
        allowed_functions.insert("split".to_string());
        allowed_functions.insert("replace".to_string());
        allowed_functions.insert("startsWith".to_string());
        allowed_functions.insert("endsWith".to_string());
        allowed_functions.insert("contains".to_string());
        allowed_functions.insert("toString".to_string());
        allowed_functions.insert("toInteger".to_string());
        allowed_functions.insert("toFloat".to_string());
        allowed_functions.insert("coalesce".to_string());
        allowed_functions.insert("exists".to_string());
        
        Self {
            max_query_length: 10000,
            max_clause_depth: 10,
            max_match_clauses: 20,
            max_create_clauses: 5,
            max_delete_clauses: 1,
            allow_dangerous_operations: false,
            allow_schema_modifications: false,
            allow_admin_commands: false,
            allowed_functions,
            forbidden_patterns: vec![
                // SQL injection patterns
                r"(?i)(\bUNION\b|\bINSERT\b|\bUPDATE\b|\bALTER\b)".to_string(),
                // System calls
                r"(?i)(\bSYSTEM\b|\bEXEC\b|\bEVAL\b)".to_string(),
                // File operations
                r"(?i)(\bLOAD\b\s+CSV\b|\bIMPORT\b)".to_string(),
                // Neo4j admin functions
                r"(?i)(dbms\.|db\.|apoc\.(?!meta|convert|text|map|coll))".to_string(),
                // Potentially dangerous patterns
                r"(?i)(\$\{.*\}|\%.*\%|javascript:|data:)".to_string(),
            ],
            validate_parameters: true,
        }
    }
}

/// Security levels for query validation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecurityLevel {
    /// Strict - Only basic read operations allowed
    Strict,
    /// Standard - Read operations and limited writes allowed
    Standard,
    /// Relaxed - Most operations allowed except dangerous ones
    Relaxed,
    /// Administrative - All operations allowed (for admin users)
    Administrative,
}

/// Result of query sanitization
#[derive(Debug, Clone)]
pub struct SanitizationResult {
    /// Whether the query is safe to execute
    pub is_safe: bool,
    /// The sanitized query (if modifications were made)
    pub sanitized_query: Option<String>,
    /// List of security issues found
    pub issues: Vec<SecurityIssue>,
    /// Risk level of the query
    pub risk_level: RiskLevel,
    /// Execution recommendations
    pub recommendations: Vec<String>,
}

/// Security issue found in query
#[derive(Debug, Clone)]
pub struct SecurityIssue {
    /// Type of security issue
    pub issue_type: SecurityIssueType,
    /// Description of the issue
    pub description: String,
    /// Severity level
    pub severity: Severity,
    /// Position in query where issue was found
    pub position: Option<usize>,
    /// Suggested fix
    pub suggestion: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecurityIssueType {
    DangerousOperation,
    SchemaModification,
    AdministrativeCommand,
    UnauthorizedFunction,
    SuspiciousPattern,
    ParameterInjection,
    QueryComplexity,
    SyntaxError,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RiskLevel {
    Safe,
    Low,
    Medium,
    High,
    Critical,
}

/// Cypher query sanitizer
pub struct CypherSanitizer {
    config: CypherSanitizerConfig,
    dangerous_keywords: HashSet<String>,
    schema_keywords: HashSet<String>,
    admin_keywords: HashSet<String>,
    forbidden_patterns: Vec<Regex>,
}

impl CypherSanitizer {
    /// Create a new sanitizer with configuration
    pub fn new(config: CypherSanitizerConfig) -> Result<Self> {
        let mut dangerous_keywords = HashSet::new();
        dangerous_keywords.insert("DELETE".to_string());
        dangerous_keywords.insert("DETACH DELETE".to_string());
        dangerous_keywords.insert("REMOVE".to_string());
        dangerous_keywords.insert("DROP".to_string());
        dangerous_keywords.insert("DESTROY".to_string());
        
        let mut schema_keywords = HashSet::new();
        schema_keywords.insert("CREATE CONSTRAINT".to_string());
        schema_keywords.insert("DROP CONSTRAINT".to_string());
        schema_keywords.insert("CREATE INDEX".to_string());
        schema_keywords.insert("DROP INDEX".to_string());
        schema_keywords.insert("CREATE DATABASE".to_string());
        schema_keywords.insert("DROP DATABASE".to_string());
        
        let mut admin_keywords = HashSet::new();
        admin_keywords.insert("SHOW USERS".to_string());
        admin_keywords.insert("CREATE USER".to_string());
        admin_keywords.insert("DROP USER".to_string());
        admin_keywords.insert("ALTER USER".to_string());
        admin_keywords.insert("SHOW ROLES".to_string());
        admin_keywords.insert("CREATE ROLE".to_string());
        admin_keywords.insert("DROP ROLE".to_string());
        admin_keywords.insert("GRANT".to_string());
        admin_keywords.insert("REVOKE".to_string());
        admin_keywords.insert("DENY".to_string());
        
        let forbidden_patterns: Result<Vec<Regex>, _> = config.forbidden_patterns
            .iter()
            .map(|pattern| Regex::new(pattern))
            .collect();
        
        let forbidden_patterns = forbidden_patterns
            .map_err(|e| anyhow!("Invalid regex pattern in configuration: {}", e))?;
        
        Ok(Self {
            config,
            dangerous_keywords,
            schema_keywords,
            admin_keywords,
            forbidden_patterns,
        })
    }
    
    /// Create sanitizer with default configuration
    pub fn default() -> Result<Self> {
        Self::new(CypherSanitizerConfig::default())
    }
    
    /// Sanitize and validate a Cypher query
    pub fn sanitize_query(&self, query: &str, security_level: SecurityLevel) -> SanitizationResult {
        let mut issues = Vec::new();
        let mut recommendations = Vec::new();
        let mut is_safe = true;
        let mut risk_level = RiskLevel::Safe;
        
        debug!("Sanitizing Cypher query with {:?} security level", security_level);
        
        // Check query length
        if query.len() > self.config.max_query_length {
            issues.push(SecurityIssue {
                issue_type: SecurityIssueType::QueryComplexity,
                description: format!("Query length ({}) exceeds maximum allowed ({})", 
                                   query.len(), self.config.max_query_length),
                severity: Severity::High,
                position: None,
                suggestion: Some("Break the query into smaller parts".to_string()),
            });
            is_safe = false;
            risk_level = risk_level.max(RiskLevel::High);
        }
        
        // Check for dangerous keywords
        let query_upper = query.to_uppercase();
        for keyword in &self.dangerous_keywords {
            if query_upper.contains(keyword) {
                let severity = match security_level {
                    SecurityLevel::Strict => Severity::Critical,
                    SecurityLevel::Standard if !self.config.allow_dangerous_operations => Severity::High,
                    SecurityLevel::Relaxed if !self.config.allow_dangerous_operations => Severity::Medium,
                    _ => Severity::Low,
                };
                
                issues.push(SecurityIssue {
                    issue_type: SecurityIssueType::DangerousOperation,
                    description: format!("Query contains dangerous operation: {}", keyword),
                    severity,
                    position: query_upper.find(keyword),
                    suggestion: Some("Consider using safer alternatives or ensure proper authorization".to_string()),
                });
                
                if severity >= Severity::High {
                    is_safe = false;
                    risk_level = risk_level.max(RiskLevel::High);
                }
            }
        }
        
        // Check for schema modifications
        for keyword in &self.schema_keywords {
            if query_upper.contains(keyword) {
                let severity = match security_level {
                    SecurityLevel::Strict | SecurityLevel::Standard => Severity::Critical,
                    SecurityLevel::Relaxed if !self.config.allow_schema_modifications => Severity::High,
                    _ => Severity::Low,
                };
                
                issues.push(SecurityIssue {
                    issue_type: SecurityIssueType::SchemaModification,
                    description: format!("Query contains schema modification: {}", keyword),
                    severity,
                    position: query_upper.find(keyword),
                    suggestion: Some("Schema modifications should be performed by administrators".to_string()),
                });
                
                if severity >= Severity::High {
                    is_safe = false;
                    risk_level = risk_level.max(RiskLevel::Critical);
                }
            }
        }
        
        // Check for administrative commands
        for keyword in &self.admin_keywords {
            if query_upper.contains(keyword) {
                let severity = match security_level {
                    SecurityLevel::Administrative => Severity::Low,
                    _ => Severity::Critical,
                };
                
                issues.push(SecurityIssue {
                    issue_type: SecurityIssueType::AdministrativeCommand,
                    description: format!("Query contains administrative command: {}", keyword),
                    severity,
                    position: query_upper.find(keyword),
                    suggestion: Some("Administrative commands require elevated privileges".to_string()),
                });
                
                if severity >= Severity::High {
                    is_safe = false;
                    risk_level = risk_level.max(RiskLevel::Critical);
                }
            }
        }
        
        // Check forbidden patterns
        for pattern in &self.forbidden_patterns {
            if let Some(mat) = pattern.find(query) {
                issues.push(SecurityIssue {
                    issue_type: SecurityIssueType::SuspiciousPattern,
                    description: "Query contains forbidden pattern".to_string(),
                    severity: Severity::High,
                    position: Some(mat.start()),
                    suggestion: Some("Remove suspicious patterns from query".to_string()),
                });
                is_safe = false;
                risk_level = risk_level.max(RiskLevel::High);
            }
        }
        
        // Check query complexity
        let complexity_issues = self.check_query_complexity(query);
        for issue in complexity_issues {
            if issue.severity >= Severity::High {
                is_safe = false;
                risk_level = risk_level.max(RiskLevel::Medium);
            }
            issues.push(issue);
        }
        
        // Check function usage
        let function_issues = self.check_function_usage(query, security_level);
        for issue in function_issues {
            if issue.severity >= Severity::High {
                is_safe = false;
                risk_level = risk_level.max(RiskLevel::Medium);
            }
            issues.push(issue);
        }
        
        // Generate recommendations
        if !is_safe {
            recommendations.push("Review and modify the query to address security issues".to_string());
        }
        
        if risk_level >= RiskLevel::Medium {
            recommendations.push("Consider using parameterized queries".to_string());
            recommendations.push("Implement additional authorization checks".to_string());
        }
        
        if issues.iter().any(|i| i.issue_type == SecurityIssueType::QueryComplexity) {
            recommendations.push("Break complex queries into smaller parts".to_string());
            recommendations.push("Use LIMIT clauses to restrict result size".to_string());
        }
        
        SanitizationResult {
            is_safe,
            sanitized_query: None, // TODO: Implement query sanitization
            issues,
            risk_level,
            recommendations,
        }
    }
    
    /// Validate query parameters for injection attacks
    pub fn validate_parameters(&self, parameters: &HashMap<String, serde_json::Value>) -> Vec<SecurityIssue> {
        let mut issues = Vec::new();
        
        if !self.config.validate_parameters {
            return issues;
        }
        
        for (key, value) in parameters {
            // Check parameter name for suspicious patterns
            if key.contains("..") || key.contains("/") || key.contains("\\") {
                issues.push(SecurityIssue {
                    issue_type: SecurityIssueType::ParameterInjection,
                    description: format!("Suspicious parameter name: {}", key),
                    severity: Severity::High,
                    position: None,
                    suggestion: Some("Use safe parameter names without special characters".to_string()),
                });
            }
            
            // Check string parameter values for injection patterns
            if let Some(str_value) = value.as_str() {
                for pattern in &self.forbidden_patterns {
                    if pattern.is_match(str_value) {
                        issues.push(SecurityIssue {
                            issue_type: SecurityIssueType::ParameterInjection,
                            description: format!("Parameter '{}' contains suspicious pattern", key),
                            severity: Severity::High,
                            position: None,
                            suggestion: Some("Sanitize parameter values before use".to_string()),
                        });
                        break;
                    }
                }
                
                // Check for extremely long string values (potential DoS)
                if str_value.len() > 100000 {
                    issues.push(SecurityIssue {
                        issue_type: SecurityIssueType::ParameterInjection,
                        description: format!("Parameter '{}' value is extremely long ({})", key, str_value.len()),
                        severity: Severity::Medium,
                        position: None,
                        suggestion: Some("Limit parameter value length".to_string()),
                    });
                }
            }
        }
        
        issues
    }
    
    /// Check query complexity
    fn check_query_complexity(&self, query: &str) -> Vec<SecurityIssue> {
        let mut issues = Vec::new();
        let query_upper = query.to_uppercase();
        
        // Count MATCH clauses
        let match_count = query_upper.matches("MATCH").count();
        if match_count > self.config.max_match_clauses {
            issues.push(SecurityIssue {
                issue_type: SecurityIssueType::QueryComplexity,
                description: format!("Too many MATCH clauses: {} (max: {})", 
                                   match_count, self.config.max_match_clauses),
                severity: Severity::Medium,
                position: None,
                suggestion: Some("Reduce the number of MATCH clauses".to_string()),
            });
        }
        
        // Count CREATE clauses
        let create_count = query_upper.matches("CREATE").count();
        if create_count > self.config.max_create_clauses {
            issues.push(SecurityIssue {
                issue_type: SecurityIssueType::QueryComplexity,
                description: format!("Too many CREATE clauses: {} (max: {})", 
                                   create_count, self.config.max_create_clauses),
                severity: Severity::High,
                position: None,
                suggestion: Some("Reduce the number of CREATE clauses or batch operations".to_string()),
            });
        }
        
        // Count DELETE clauses
        let delete_count = query_upper.matches("DELETE").count();
        if delete_count > self.config.max_delete_clauses {
            issues.push(SecurityIssue {
                issue_type: SecurityIssueType::QueryComplexity,
                description: format!("Too many DELETE clauses: {} (max: {})", 
                                   delete_count, self.config.max_delete_clauses),
                severity: Severity::High,
                position: None,
                suggestion: Some("Limit DELETE operations per query".to_string()),
            });
        }
        
        // Check for missing LIMIT clauses on potentially large result sets
        if query_upper.contains("MATCH") && !query_upper.contains("LIMIT") && !query_upper.contains("COUNT") {
            issues.push(SecurityIssue {
                issue_type: SecurityIssueType::QueryComplexity,
                description: "Query may return large result set without LIMIT clause".to_string(),
                severity: Severity::Low,
                position: None,
                suggestion: Some("Add LIMIT clause to control result size".to_string()),
            });
        }
        
        issues
    }
    
    /// Check function usage against whitelist
    fn check_function_usage(&self, query: &str, security_level: SecurityLevel) -> Vec<SecurityIssue> {
        let mut issues = Vec::new();
        
        // Extract function calls using regex
        let function_regex = Regex::new(r"(?i)\b([a-zA-Z_][a-zA-Z0-9_]*)\s*\(").unwrap();
        
        for captures in function_regex.captures_iter(query) {
            if let Some(function_name) = captures.get(1) {
                let func_name = function_name.as_str().toLowerCase();
                
                // Skip known safe keywords that look like functions but aren't
                if matches!(func_name.as_str(), "match" | "create" | "merge" | "return" | "where" | "with" | "order" | "limit") {
                    continue;
                }
                
                let is_allowed = match security_level {
                    SecurityLevel::Administrative => true,
                    SecurityLevel::Relaxed => true, // Most functions allowed in relaxed mode
                    SecurityLevel::Standard | SecurityLevel::Strict => {
                        self.config.allowed_functions.contains(&func_name)
                    }
                };
                
                if !is_allowed {
                    let severity = match security_level {
                        SecurityLevel::Strict => Severity::High,
                        SecurityLevel::Standard => Severity::Medium,
                        _ => Severity::Low,
                    };
                    
                    issues.push(SecurityIssue {
                        issue_type: SecurityIssueType::UnauthorizedFunction,
                        description: format!("Unauthorized function usage: {}", func_name),
                        severity,
                        position: Some(function_name.start()),
                        suggestion: Some("Use only whitelisted functions or request permission".to_string()),
                    });
                }
            }
        }
        
        issues
    }
    
    /// Get security recommendations for a query
    pub fn get_security_recommendations(&self, query: &str) -> Vec<String> {
        let mut recommendations = Vec::new();
        let query_upper = query.to_uppercase();
        
        // Recommend parameterization
        if query.contains("'") || query.contains("\"") {
            recommendations.push("Use parameterized queries instead of string literals".to_string());
        }
        
        // Recommend LIMIT clauses
        if query_upper.contains("MATCH") && !query_upper.contains("LIMIT") {
            recommendations.push("Add LIMIT clauses to prevent large result sets".to_string());
        }
        
        // Recommend read-only operations
        if query_upper.contains("CREATE") || query_upper.contains("DELETE") || query_upper.contains("SET") {
            recommendations.push("Consider using read-only operations when possible".to_string());
        }
        
        // Recommend input validation
        recommendations.push("Validate all user inputs before query execution".to_string());
        recommendations.push("Use proper authentication and authorization".to_string());
        recommendations.push("Log and monitor query execution for security audit".to_string());
        
        recommendations
    }
}

trait StringExt {
    fn toLowerCase(&self) -> String;
}

impl StringExt for str {
    fn toLowerCase(&self) -> String {
        self.to_lowercase()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_safe_query() {
        let sanitizer = CypherSanitizer::default().unwrap();
        let result = sanitizer.sanitize_query(
            "MATCH (n:Person) WHERE n.name = $name RETURN n LIMIT 10",
            SecurityLevel::Standard
        );
        
        assert!(result.is_safe);
        assert_eq!(result.risk_level, RiskLevel::Safe);
    }
    
    #[test]
    fn test_dangerous_query() {
        let sanitizer = CypherSanitizer::default().unwrap();
        let result = sanitizer.sanitize_query(
            "MATCH (n) DELETE n",
            SecurityLevel::Standard
        );
        
        assert!(!result.is_safe);
        assert!(result.risk_level >= RiskLevel::High);
        assert!(result.issues.iter().any(|i| i.issue_type == SecurityIssueType::DangerousOperation));
    }
    
    #[test]
    fn test_query_length_limit() {
        let sanitizer = CypherSanitizer::default().unwrap();
        let long_query = "MATCH (n) ".repeat(1000);
        let result = sanitizer.sanitize_query(&long_query, SecurityLevel::Standard);
        
        assert!(!result.is_safe);
        assert!(result.issues.iter().any(|i| i.issue_type == SecurityIssueType::QueryComplexity));
    }
    
    #[test]
    fn test_parameter_validation() {
        let sanitizer = CypherSanitizer::default().unwrap();
        let mut params = HashMap::new();
        params.insert("../malicious".to_string(), serde_json::Value::String("value".to_string()));
        
        let issues = sanitizer.validate_parameters(&params);
        assert!(!issues.is_empty());
        assert!(issues.iter().any(|i| i.issue_type == SecurityIssueType::ParameterInjection));
    }
    
    #[test]
    fn test_function_whitelist() {
        let sanitizer = CypherSanitizer::default().unwrap();
        let result = sanitizer.sanitize_query(
            "MATCH (n) RETURN unauthorizedFunction(n)",
            SecurityLevel::Strict
        );
        
        assert!(result.issues.iter().any(|i| i.issue_type == SecurityIssueType::UnauthorizedFunction));
    }
}
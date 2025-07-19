# Security Features Implementation

## Overview
The Knowledge Graph MCP Server includes comprehensive security features to protect against various attack vectors and ensure safe operation in production environments.

## Implemented Security Features

### 1. Cypher Query Sanitization ✅ COMPLETED
**File**: `src/security/cypher_sanitizer.rs`

Advanced input sanitization and validation for Cypher queries to prevent injection attacks and dangerous operations.

**Key Features**:
- Multi-level security modes (Strict, Standard, Relaxed, Administrative)
- Dangerous operation detection (DELETE, DROP, DETACH DELETE)
- Schema modification protection (CREATE/DROP CONSTRAINT/INDEX)
- Administrative command blocking (USER/ROLE management, GRANT/REVOKE)
- Function whitelist enforcement with safe read-only functions
- Query complexity limits (max clauses, depth, length)
- Forbidden pattern detection (SQL injection, XSS, file access)
- Parameter injection protection
- Comprehensive issue reporting with severity levels

**Security Levels**:
```rust
SecurityLevel::Strict      // Only basic read operations
SecurityLevel::Standard    // Read + limited write operations  
SecurityLevel::Relaxed     // Most operations except dangerous ones
SecurityLevel::Administrative // All operations (admin users only)
```

**Validation Features**:
- **Query Length Limits**: Prevents DoS attacks via extremely long queries
- **Clause Counting**: Limits MATCH, CREATE, DELETE clauses per query
- **Function Whitelisting**: Only allows safe functions like count(), size(), length()
- **Pattern Blacklisting**: Blocks SQL injection, XSS, and system access patterns
- **Parameter Validation**: Validates parameter names and values for injection attempts

**Configuration**:
```rust
CypherSanitizerConfig {
    max_query_length: 10000,
    max_clause_depth: 10,
    max_match_clauses: 20,
    max_create_clauses: 5,
    max_delete_clauses: 1,
    allow_dangerous_operations: false,
    allow_schema_modifications: false,
    allow_admin_commands: false,
    validate_parameters: true,
}
```

### 2. Session-Based Rate Limiting ✅ COMPLETED
**File**: `src/security/rate_limiter.rs`

Sophisticated rate limiting system with per-session, per-IP, and adaptive limiting based on system load.

**Key Features**:
- Token bucket algorithm for smooth rate limiting
- Multiple user tiers with different limits (Standard: 100/min, Premium: 500/min, Admin: 2000/min)
- Per-IP rate limiting to prevent abuse from single sources
- Adaptive rate limiting based on system load
- Session timeout and cleanup
- Comprehensive statistics and monitoring

**Rate Limiting Modes**:
- **Per-Session**: Individual limits for each authenticated session
- **Per-IP**: Aggregate limits per IP address to prevent IP-based attacks
- **Adaptive**: Dynamic limiting based on system resource usage
- **Tiered**: Different limits based on user tier (Standard/Premium/Admin)

**Configuration**:
```rust
RateLimiterConfig {
    requests_per_minute: 100,        // Standard users
    premium_requests_per_minute: 500, // Premium users  
    admin_requests_per_minute: 2000,  // Admin users
    window_size_seconds: 60,
    burst_size: 10,
    enable_ip_limiting: true,
    ip_requests_per_minute: 200,
    enable_adaptive_limiting: true,
    adaptive_load_threshold: 0.8,
}
```

**Features**:
- **Token Bucket**: Allows burst requests while maintaining average rate
- **Window-based Tracking**: Rolling window for accurate rate calculations
- **Automatic Cleanup**: Removes expired sessions and IP buckets
- **System Load Integration**: Reduces limits when system is under stress
- **Detailed Metrics**: Request counts, remaining tokens, reset times

### 3. Authentication & Authorization ✅ COMPLETED
**File**: `src/security/auth_manager.rs`

Comprehensive authentication system supporting multiple token types and fine-grained permissions.

**Authentication Methods**:
- **JWT Tokens**: Stateless authentication with configurable expiration
- **API Keys**: Long-lived keys for service-to-service communication
- **Sessions**: Stateful authentication with automatic timeout

**Permission System**:
```rust
enum Permission {
    // Read permissions
    ReadGraph, ReadMetrics, ReadHealth,
    
    // Write permissions  
    WriteGraph, ModifyEntities, DeleteEntities,
    
    // Analysis permissions
    AnalyzeCode, ExportData,
    
    // Administrative permissions
    ManageUsers, ManageConfiguration, ViewLogs,
    
    // System permissions
    SystemAdministration, DatabaseAdministration,
}
```

**User Tiers**:
- **Standard**: Basic read/write access with standard rate limits
- **Premium**: Extended capabilities with higher rate limits
- **Admin**: Full system access with maximum rate limits

**Security Features**:
- **JWT Validation**: Signature verification, expiration checking, revocation support
- **API Key Security**: HMAC-based key hashing, expiration tracking, usage monitoring
- **Session Management**: Timeout handling, concurrent session limits, IP tracking
- **Permission Checking**: Granular access control with inheritance
- **Token Revocation**: Ability to revoke JWTs and deactivate API keys/sessions

**Configuration**:
```rust
AuthConfig {
    jwt_expiration_hours: 24,
    api_key_expiration_days: 30,
    session_timeout_minutes: 60,
    max_sessions_per_user: 5,
    require_authentication: false, // Configurable for development
}
```

### 4. Request Validation & Size Limits ✅ COMPLETED
**File**: `src/security/request_validator.rs`

Comprehensive request validation to prevent DoS attacks and malicious input.

**Validation Features**:
- **Size Limits**: Maximum request body size, query length, parameter counts
- **Depth Limits**: Maximum object nesting to prevent stack overflow
- **Content Validation**: Detection of suspicious patterns and malicious content
- **Tool-Specific Validation**: Custom validation for each MCP tool
- **Format Validation**: JSON structure and type validation
- **Security Pattern Detection**: XSS, injection, path traversal prevention

**Configuration**:
```rust
RequestValidatorConfig {
    max_request_size: 10 * 1024 * 1024,  // 10MB
    max_query_length: 50000,             // 50KB
    max_parameters: 100,
    max_parameter_name_length: 100,
    max_parameter_value_length: 10000,
    max_array_size: 1000,
    max_nesting_depth: 10,
    max_string_length: 100000,
    max_batch_size: 100,
    strict_validation: true,
}
```

**Validation Types**:
- **Size Validation**: Prevents memory exhaustion attacks
- **Count Validation**: Limits array sizes and parameter counts
- **Depth Validation**: Prevents deeply nested object attacks
- **Content Validation**: Detects malicious patterns and injection attempts
- **Format Validation**: Ensures valid JSON structure and types

**Security Patterns Detected**:
- XSS attempts (`<script>`, `javascript:`, `onload=`)
- Injection patterns (`eval(`, `function(`, `${`)
- File access attempts (`../`, `/etc/`, `C:\`)
- SQL injection (`DROP TABLE`, `DELETE FROM`)
- Null byte injection (`\x00`)

### 5. Configuration Encryption ✅ COMPLETED
**File**: `src/security/config_encryption.rs`

Secure encryption of sensitive configuration values using AES-256-GCM encryption.

**Key Features**:
- **AES-256-GCM Encryption**: Industry-standard authenticated encryption
- **Automatic Field Detection**: Encrypts fields containing "password", "secret", "token", etc.
- **Key Management**: Environment-based key derivation with fallback
- **Selective Encryption**: Only encrypts sensitive fields, leaves others readable
- **Key Rotation**: Support for re-encrypting data with new keys
- **Strength Validation**: Validates encryption key strength and provides recommendations

**Encrypted Fields** (automatically detected):
- password, secret, token, key, credentials
- jwt_secret, database_password, redis_password
- api_key, auth_token, encryption_key

**Configuration**:
```rust
EncryptionConfig {
    master_key: derive_from_env_or_default(),
    encrypt_all_sensitive: true,
    encrypted_fields: vec![
        "password", "secret", "token", "key", 
        "credentials", "jwt_secret", "api_key"
    ],
}
```

**Security Features**:
- **Environment Key Derivation**: Uses `KG_ENCRYPTION_KEY` environment variable
- **Default Key Warning**: Warns when using default key in production
- **Key Strength Assessment**: Validates key length and entropy
- **Authenticated Encryption**: Prevents tampering with encrypted data
- **Nonce Management**: Unique nonce for each encryption operation
- **Safe Serialization**: Secure JSON serialization of encrypted values

**Usage Example**:
```rust
// Encrypt configuration
let encryptor = ConfigEncryption::default();
encryptor.store_encrypted_config(&config, "config.json")?;

// Load and decrypt
let config: MyConfig = encryptor.load_encrypted_config("config.json")?;
```

## Security Architecture Benefits

### Defense in Depth
- **Input Validation**: Multiple layers of request and query validation
- **Authentication**: Multi-method authentication with session management
- **Authorization**: Fine-grained permissions with role-based access
- **Rate Limiting**: Multiple strategies to prevent abuse
- **Encryption**: Sensitive data protection at rest

### Attack Prevention
- **Injection Attacks**: Cypher injection prevention through sanitization
- **DoS Attacks**: Rate limiting, size limits, complexity limits
- **XSS/CSRF**: Input validation and content sanitization
- **Data Exfiltration**: Permission-based access control
- **Configuration Exposure**: Automatic encryption of sensitive values

### Monitoring & Auditing
- **Security Events**: Detailed logging of authentication, authorization, and validation failures
- **Rate Limit Monitoring**: Real-time tracking of rate limit violations
- **Permission Auditing**: Tracking of permission checks and violations
- **Configuration Security**: Key strength validation and recommendations

### Production Readiness
- **Configurable Security**: Adjustable security levels for different environments
- **Performance Optimized**: Efficient validation and caching
- **Scalable Architecture**: Session-based rate limiting with cleanup
- **Standards Compliance**: Industry-standard encryption and authentication

## Security Configuration

### Environment Variables
```bash
# Encryption key (32+ bytes, base64 encoded)
export KG_ENCRYPTION_KEY="your-strong-encryption-key-here"

# Authentication settings
export KG_JWT_SECRET="your-jwt-secret-key"
export KG_REQUIRE_AUTH="true"

# Rate limiting
export KG_RATE_LIMIT_ENABLED="true"
export KG_ADAPTIVE_LIMITING="true"
```

### Security Headers & Integration
The security system integrates seamlessly with the MCP server to provide:

1. **Request Preprocessing**: All requests validated before processing
2. **Authentication Middleware**: Automatic token validation and session management
3. **Authorization Checks**: Permission verification for each operation
4. **Rate Limiting**: Per-session and per-IP request throttling
5. **Query Sanitization**: All Cypher queries validated before execution
6. **Configuration Protection**: Sensitive values encrypted at rest

### Health Monitoring
The health check tool provides comprehensive security status:

```json
{
  "security": {
    "authentication": {
      "total_users": 1,
      "active_sessions": 3,
      "active_api_keys": 5
    },
    "rate_limiting": {
      "active_sessions": 25,
      "active_ips": 12,
      "adaptive_limiting_active": false
    },
    "validation": {
      "strict_validation": true,
      "max_request_size": "10MB",
      "max_query_length": "50KB"
    },
    "encryption": {
      "key_strength": "Strong",
      "encrypted_fields": 8,
      "using_env_key": true
    }
  }
}
```

## Testing & Validation

### Security Test Scenarios

1. **Injection Attack Prevention**:
   - Malicious Cypher queries with injection attempts
   - XSS payloads in query parameters
   - Path traversal attempts in file operations

2. **Rate Limiting Validation**:
   - High-volume request testing
   - Burst request handling
   - IP-based limiting verification

3. **Authentication Testing**:
   - JWT token validation and expiration
   - API key authentication and rotation
   - Session management and timeout

4. **Authorization Testing**:
   - Permission boundary testing
   - Role escalation prevention
   - Resource access control

5. **Encryption Testing**:
   - Configuration encryption/decryption
   - Key rotation procedures
   - Tamper detection

This comprehensive security implementation ensures the Knowledge Graph MCP Server can operate safely in production environments while maintaining usability and performance.
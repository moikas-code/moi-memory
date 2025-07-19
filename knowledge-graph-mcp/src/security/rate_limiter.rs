use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{debug, warn, error};
use uuid::Uuid;

/// Configuration for rate limiting
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimiterConfig {
    /// Maximum requests per minute for standard users
    pub requests_per_minute: u32,
    /// Maximum requests per minute for premium users
    pub premium_requests_per_minute: u32,
    /// Maximum requests per minute for admin users
    pub admin_requests_per_minute: u32,
    /// Window size for rate limiting (in seconds)
    pub window_size_seconds: u64,
    /// Maximum burst size (requests that can be made instantly)
    pub burst_size: u32,
    /// Cleanup interval for old sessions (in seconds)
    pub cleanup_interval_seconds: u64,
    /// Session timeout (in seconds)
    pub session_timeout_seconds: u64,
    /// Enable per-IP rate limiting
    pub enable_ip_limiting: bool,
    /// Maximum requests per minute per IP
    pub ip_requests_per_minute: u32,
    /// Enable adaptive rate limiting based on system load
    pub enable_adaptive_limiting: bool,
    /// System load threshold for adaptive limiting (0.0 - 1.0)
    pub adaptive_load_threshold: f64,
}

impl Default for RateLimiterConfig {
    fn default() -> Self {
        Self {
            requests_per_minute: 100,
            premium_requests_per_minute: 500,
            admin_requests_per_minute: 2000,
            window_size_seconds: 60,
            burst_size: 10,
            cleanup_interval_seconds: 300, // 5 minutes
            session_timeout_seconds: 3600, // 1 hour
            enable_ip_limiting: true,
            ip_requests_per_minute: 200,
            enable_adaptive_limiting: true,
            adaptive_load_threshold: 0.8,
        }
    }
}

/// User tier for rate limiting
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserTier {
    Standard,
    Premium,
    Admin,
}

/// Rate limiting result
#[derive(Debug, Clone)]
pub struct RateLimitResult {
    /// Whether the request is allowed
    pub allowed: bool,
    /// Remaining requests in current window
    pub remaining: u32,
    /// Time until window reset
    pub reset_time: Duration,
    /// Current rate limit for this user/session
    pub limit: u32,
    /// Reason for denial (if denied)
    pub deny_reason: Option<String>,
}

/// Rate limiting bucket using token bucket algorithm
#[derive(Debug, Clone)]
struct RateBucket {
    /// Maximum tokens in bucket
    capacity: u32,
    /// Current tokens available
    tokens: f64,
    /// Rate of token refill per second
    refill_rate: f64,
    /// Last refill timestamp
    last_refill: Instant,
    /// Window start time
    window_start: Instant,
    /// Request count in current window
    request_count: u32,
    /// User tier
    user_tier: UserTier,
    /// Last activity timestamp
    last_activity: Instant,
}

impl RateBucket {
    fn new(capacity: u32, refill_rate: f64, user_tier: UserTier) -> Self {
        let now = Instant::now();
        Self {
            capacity,
            tokens: capacity as f64,
            refill_rate,
            last_refill: now,
            window_start: now,
            request_count: 0,
            user_tier,
            last_activity: now,
        }
    }
    
    /// Try to consume a token
    fn try_consume(&mut self, window_duration: Duration) -> bool {
        let now = Instant::now();
        self.last_activity = now;
        
        // Refill tokens based on time elapsed
        self.refill_tokens(now);
        
        // Reset window if needed
        if now.duration_since(self.window_start) >= window_duration {
            self.window_start = now;
            self.request_count = 0;
        }
        
        // Check if we have tokens and haven't exceeded window limit
        if self.tokens >= 1.0 && self.request_count < self.capacity {
            self.tokens -= 1.0;
            self.request_count += 1;
            true
        } else {
            false
        }
    }
    
    /// Refill tokens based on elapsed time
    fn refill_tokens(&mut self, now: Instant) {
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        let tokens_to_add = elapsed * self.refill_rate;
        
        self.tokens = (self.tokens + tokens_to_add).min(self.capacity as f64);
        self.last_refill = now;
    }
    
    /// Get remaining tokens
    fn remaining_tokens(&mut self) -> u32 {
        self.refill_tokens(Instant::now());
        self.tokens.floor() as u32
    }
    
    /// Get time until next token
    fn time_until_token(&self) -> Duration {
        if self.tokens >= 1.0 {
            Duration::ZERO
        } else {
            let tokens_needed = 1.0 - self.tokens;
            let time_needed = tokens_needed / self.refill_rate;
            Duration::from_secs_f64(time_needed)
        }
    }
    
    /// Check if bucket is expired
    fn is_expired(&self, timeout: Duration) -> bool {
        Instant::now().duration_since(self.last_activity) > timeout
    }
}

/// Session-based rate limiter
pub struct SessionRateLimiter {
    config: RateLimiterConfig,
    /// Session-based rate buckets
    session_buckets: Arc<RwLock<HashMap<String, RateBucket>>>,
    /// IP-based rate buckets
    ip_buckets: Arc<RwLock<HashMap<String, RateBucket>>>,
    /// System load monitor for adaptive limiting
    system_load: Arc<RwLock<f64>>,
}

impl SessionRateLimiter {
    /// Create a new rate limiter
    pub async fn new(config: RateLimiterConfig) -> Self {
        let limiter = Self {
            config,
            session_buckets: Arc::new(RwLock::new(HashMap::new())),
            ip_buckets: Arc::new(RwLock::new(HashMap::new())),
            system_load: Arc::new(RwLock::new(0.0)),
        };
        
        // Start background cleanup task
        limiter.start_cleanup_task().await;
        
        // Start system load monitoring if adaptive limiting is enabled
        if limiter.config.enable_adaptive_limiting {
            limiter.start_load_monitoring().await;
        }
        
        limiter
    }
    
    /// Check if a request is allowed for a session
    pub async fn check_rate_limit(
        &self,
        session_id: &str,
        ip_address: Option<&str>,
        user_tier: UserTier,
    ) -> RateLimitResult {
        // Check system load for adaptive limiting
        if self.config.enable_adaptive_limiting {
            let system_load = *self.system_load.read().await;
            if system_load > self.config.adaptive_load_threshold {
                return RateLimitResult {
                    allowed: false,
                    remaining: 0,
                    reset_time: Duration::from_secs(60),
                    limit: 0,
                    deny_reason: Some("System overloaded - rate limiting active".to_string()),
                };
            }
        }
        
        // Check IP-based rate limiting first
        if self.config.enable_ip_limiting {
            if let Some(ip) = ip_address {
                let ip_result = self.check_ip_rate_limit(ip).await;
                if !ip_result.allowed {
                    return ip_result;
                }
            }
        }
        
        // Check session-based rate limiting
        self.check_session_rate_limit(session_id, user_tier).await
    }
    
    /// Check IP-based rate limiting
    async fn check_ip_rate_limit(&self, ip_address: &str) -> RateLimitResult {
        let mut buckets = self.ip_buckets.write().await;
        
        let bucket = buckets.entry(ip_address.to_string()).or_insert_with(|| {
            RateBucket::new(
                self.config.ip_requests_per_minute,
                self.config.ip_requests_per_minute as f64 / 60.0,
                UserTier::Standard, // IP buckets use standard tier
            )
        });
        
        let window_duration = Duration::from_secs(self.config.window_size_seconds);
        let allowed = bucket.try_consume(window_duration);
        let remaining = bucket.remaining_tokens();
        let reset_time = if allowed {
            window_duration - Instant::now().duration_since(bucket.window_start)
        } else {
            bucket.time_until_token()
        };
        
        RateLimitResult {
            allowed,
            remaining,
            reset_time,
            limit: self.config.ip_requests_per_minute,
            deny_reason: if !allowed {
                Some("IP rate limit exceeded".to_string())
            } else {
                None
            },
        }
    }
    
    /// Check session-based rate limiting
    async fn check_session_rate_limit(&self, session_id: &str, user_tier: UserTier) -> RateLimitResult {
        let mut buckets = self.session_buckets.write().await;
        
        let limit = self.get_rate_limit_for_tier(user_tier);
        let refill_rate = limit as f64 / 60.0; // per second
        
        let bucket = buckets.entry(session_id.to_string()).or_insert_with(|| {
            RateBucket::new(limit, refill_rate, user_tier)
        });
        
        // Update bucket if user tier changed
        if bucket.user_tier != user_tier {
            bucket.user_tier = user_tier;
            bucket.capacity = limit;
            bucket.refill_rate = refill_rate;
        }
        
        let window_duration = Duration::from_secs(self.config.window_size_seconds);
        let allowed = bucket.try_consume(window_duration);
        let remaining = bucket.remaining_tokens();
        let reset_time = if allowed {
            window_duration - Instant::now().duration_since(bucket.window_start)
        } else {
            bucket.time_until_token()
        };
        
        RateLimitResult {
            allowed,
            remaining,
            reset_time,
            limit,
            deny_reason: if !allowed {
                Some("Session rate limit exceeded".to_string())
            } else {
                None
            },
        }
    }
    
    /// Get rate limit for user tier
    fn get_rate_limit_for_tier(&self, user_tier: UserTier) -> u32 {
        match user_tier {
            UserTier::Standard => self.config.requests_per_minute,
            UserTier::Premium => self.config.premium_requests_per_minute,
            UserTier::Admin => self.config.admin_requests_per_minute,
        }
    }
    
    /// Start background cleanup task
    async fn start_cleanup_task(&self) {
        let session_buckets = self.session_buckets.clone();
        let ip_buckets = self.ip_buckets.clone();
        let cleanup_interval = Duration::from_secs(self.config.cleanup_interval_seconds);
        let session_timeout = Duration::from_secs(self.config.session_timeout_seconds);
        
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(cleanup_interval);
            
            loop {
                interval.tick().await;
                
                // Cleanup expired session buckets
                {
                    let mut buckets = session_buckets.write().await;
                    buckets.retain(|_, bucket| !bucket.is_expired(session_timeout));
                    
                    debug!("Rate limiter cleanup: {} active session buckets", buckets.len());
                }
                
                // Cleanup expired IP buckets
                {
                    let mut buckets = ip_buckets.write().await;
                    buckets.retain(|_, bucket| !bucket.is_expired(session_timeout));
                    
                    debug!("Rate limiter cleanup: {} active IP buckets", buckets.len());
                }
            }
        });
    }
    
    /// Start system load monitoring for adaptive limiting
    async fn start_load_monitoring(&self) {
        let system_load = self.system_load.clone();
        
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(30));
            
            loop {
                interval.tick().await;
                
                // Simple system load estimation based on active connections
                // In a real implementation, this would use system APIs
                let estimated_load = Self::estimate_system_load().await;
                
                {
                    let mut load = system_load.write().await;
                    *load = estimated_load;
                }
                
                debug!("System load estimate: {:.2}", estimated_load);
            }
        });
    }
    
    /// Estimate system load (simplified implementation)
    async fn estimate_system_load() -> f64 {
        // This is a simplified estimation
        // In a real implementation, you would use system APIs to get:
        // - CPU usage
        // - Memory usage
        // - Network connections
        // - Database connection pool utilization
        
        // For now, return a random value between 0.0 and 1.0
        use rand::Rng;
        let mut rng = rand::thread_rng();
        rng.gen_range(0.0..1.0)
    }
    
    /// Get current rate limiter statistics
    pub async fn get_statistics(&self) -> RateLimiterStats {
        let session_buckets = self.session_buckets.read().await;
        let ip_buckets = self.ip_buckets.read().await;
        let system_load = *self.system_load.read().await;
        
        let active_sessions = session_buckets.len();
        let active_ips = ip_buckets.len();
        
        // Calculate tier distribution
        let mut tier_counts = HashMap::new();
        for bucket in session_buckets.values() {
            *tier_counts.entry(bucket.user_tier).or_insert(0) += 1;
        }
        
        RateLimiterStats {
            active_sessions,
            active_ips,
            system_load,
            tier_distribution: tier_counts,
            adaptive_limiting_active: self.config.enable_adaptive_limiting && 
                                    system_load > self.config.adaptive_load_threshold,
        }
    }
    
    /// Create a new session ID
    pub fn create_session_id() -> String {
        Uuid::new_v4().to_string()
    }
    
    /// Update system load manually (for testing or external monitoring)
    pub async fn update_system_load(&self, load: f64) {
        let mut system_load = self.system_load.write().await;
        *system_load = load.clamp(0.0, 1.0);
    }
}

/// Rate limiter statistics
#[derive(Debug, Clone, Serialize)]
pub struct RateLimiterStats {
    /// Number of active session buckets
    pub active_sessions: usize,
    /// Number of active IP buckets
    pub active_ips: usize,
    /// Current system load (0.0 - 1.0)
    pub system_load: f64,
    /// Distribution of user tiers
    pub tier_distribution: HashMap<UserTier, usize>,
    /// Whether adaptive limiting is currently active
    pub adaptive_limiting_active: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_rate_limiting_basic() {
        let config = RateLimiterConfig {
            requests_per_minute: 2,
            ..Default::default()
        };
        
        let limiter = SessionRateLimiter::new(config).await;
        let session_id = "test_session";
        
        // First request should be allowed
        let result1 = limiter.check_rate_limit(session_id, None, UserTier::Standard).await;
        assert!(result1.allowed);
        assert_eq!(result1.remaining, 1);
        
        // Second request should be allowed
        let result2 = limiter.check_rate_limit(session_id, None, UserTier::Standard).await;
        assert!(result2.allowed);
        assert_eq!(result2.remaining, 0);
        
        // Third request should be denied
        let result3 = limiter.check_rate_limit(session_id, None, UserTier::Standard).await;
        assert!(!result3.allowed);
    }
    
    #[tokio::test]
    async fn test_user_tier_limits() {
        let config = RateLimiterConfig {
            requests_per_minute: 1,
            premium_requests_per_minute: 2,
            admin_requests_per_minute: 3,
            ..Default::default()
        };
        
        let limiter = SessionRateLimiter::new(config).await;
        
        // Standard user gets 1 request
        let result = limiter.check_rate_limit("standard", None, UserTier::Standard).await;
        assert!(result.allowed);
        assert_eq!(result.limit, 1);
        
        let result = limiter.check_rate_limit("standard", None, UserTier::Standard).await;
        assert!(!result.allowed);
        
        // Premium user gets 2 requests
        let result = limiter.check_rate_limit("premium", None, UserTier::Premium).await;
        assert!(result.allowed);
        assert_eq!(result.limit, 2);
        
        let result = limiter.check_rate_limit("premium", None, UserTier::Premium).await;
        assert!(result.allowed);
        
        let result = limiter.check_rate_limit("premium", None, UserTier::Premium).await;
        assert!(!result.allowed);
    }
    
    #[tokio::test]
    async fn test_ip_rate_limiting() {
        let config = RateLimiterConfig {
            ip_requests_per_minute: 1,
            enable_ip_limiting: true,
            ..Default::default()
        };
        
        let limiter = SessionRateLimiter::new(config).await;
        
        // First request from IP should be allowed
        let result1 = limiter.check_rate_limit("session1", Some("192.168.1.1"), UserTier::Standard).await;
        assert!(result1.allowed);
        
        // Second request from same IP should be denied
        let result2 = limiter.check_rate_limit("session2", Some("192.168.1.1"), UserTier::Standard).await;
        assert!(!result2.allowed);
        assert!(result2.deny_reason.unwrap().contains("IP rate limit"));
        
        // Request from different IP should be allowed
        let result3 = limiter.check_rate_limit("session3", Some("192.168.1.2"), UserTier::Standard).await;
        assert!(result3.allowed);
    }
    
    #[tokio::test]
    async fn test_adaptive_limiting() {
        let config = RateLimiterConfig {
            enable_adaptive_limiting: true,
            adaptive_load_threshold: 0.5,
            ..Default::default()
        };
        
        let limiter = SessionRateLimiter::new(config).await;
        
        // Set high system load
        limiter.update_system_load(0.9).await;
        
        // Request should be denied due to high system load
        let result = limiter.check_rate_limit("test", None, UserTier::Standard).await;
        assert!(!result.allowed);
        assert!(result.deny_reason.unwrap().contains("System overloaded"));
        
        // Set low system load
        limiter.update_system_load(0.3).await;
        
        // Request should be allowed now
        let result = limiter.check_rate_limit("test", None, UserTier::Standard).await;
        assert!(result.allowed);
    }
}
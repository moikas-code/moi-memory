use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{info, warn, error, debug};
use serde_json::Value;

use crate::cache_manager::CacheManager;

#[derive(Debug, Clone)]
pub struct CacheDegradationConfig {
    pub health_check_interval: Duration,
    pub recovery_check_interval: Duration,
    pub max_memory_cache_size: usize,
    pub memory_cache_ttl: Duration,
    pub consecutive_failures_threshold: u32,
    pub consecutive_successes_threshold: u32,
}

impl Default for CacheDegradationConfig {
    fn default() -> Self {
        Self {
            health_check_interval: Duration::from_secs(30),
            recovery_check_interval: Duration::from_secs(10),
            max_memory_cache_size: 1000,
            memory_cache_ttl: Duration::from_secs(300), // 5 minutes
            consecutive_failures_threshold: 3,
            consecutive_successes_threshold: 2,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum CacheState {
    Healthy,    // Redis is working normally
    Degraded,   // Redis is failing, using in-memory fallback
    Recovering, // Testing Redis recovery
}

#[derive(Debug, Clone)]
pub struct CacheEntry {
    pub value: Value,
    pub created_at: Instant,
    pub ttl: Duration,
}

impl CacheEntry {
    pub fn new(value: Value, ttl: Duration) -> Self {
        Self {
            value,
            created_at: Instant::now(),
            ttl,
        }
    }
    
    pub fn is_expired(&self) -> bool {
        self.created_at.elapsed() > self.ttl
    }
}

#[derive(Debug, Clone)]
pub struct CacheDegradationStats {
    pub current_state: CacheState,
    pub redis_failures: u32,
    pub redis_successes: u32,
    pub last_redis_failure: Option<Instant>,
    pub last_redis_success: Option<Instant>,
    pub memory_cache_hits: u64,
    pub memory_cache_misses: u64,
    pub memory_cache_entries: usize,
    pub redis_cache_hits: u64,
    pub redis_cache_misses: u64,
    pub degraded_mode_duration: Duration,
    pub recovery_attempts: u32,
}

impl Default for CacheDegradationStats {
    fn default() -> Self {
        Self {
            current_state: CacheState::Healthy,
            redis_failures: 0,
            redis_successes: 0,
            last_redis_failure: None,
            last_redis_success: None,
            memory_cache_hits: 0,
            memory_cache_misses: 0,
            memory_cache_entries: 0,
            redis_cache_hits: 0,
            redis_cache_misses: 0,
            degraded_mode_duration: Duration::from_secs(0),
            recovery_attempts: 0,
        }
    }
}

pub struct GracefulCacheManager {
    config: CacheDegradationConfig,
    redis_cache: Arc<tokio::sync::Mutex<CacheManager>>,
    memory_cache: Arc<RwLock<HashMap<String, CacheEntry>>>,
    state: Arc<RwLock<CacheState>>,
    stats: Arc<RwLock<CacheDegradationStats>>,
    degraded_since: Arc<RwLock<Option<Instant>>>,
}

impl GracefulCacheManager {
    pub async fn new(redis_cache: CacheManager) -> Self {
        Self::with_config(redis_cache, CacheDegradationConfig::default()).await
    }
    
    pub async fn with_config(redis_cache: CacheManager, config: CacheDegradationConfig) -> Self {
        let manager = Self {
            config,
            redis_cache: Arc::new(tokio::sync::Mutex::new(redis_cache)),
            memory_cache: Arc::new(RwLock::new(HashMap::new())),
            state: Arc::new(RwLock::new(CacheState::Healthy)),
            stats: Arc::new(RwLock::new(CacheDegradationStats::default())),
            degraded_since: Arc::new(RwLock::new(None)),
        };
        
        // Start background health monitoring
        manager.start_health_monitoring().await;
        
        manager
    }
    
    /// Get a value from cache with automatic fallback
    pub async fn get(&self, key: &str) -> Result<Option<Value>, CacheError> {
        let current_state = self.state.read().await.clone();
        
        match current_state {
            CacheState::Healthy => {
                // Try Redis first
                match self.get_from_redis(key).await {
                    Ok(value) => {
                        self.record_redis_success().await;
                        Ok(value)
                    }
                    Err(e) => {
                        self.record_redis_failure().await;
                        self.check_degradation().await;
                        
                        // Fallback to memory cache
                        self.get_from_memory(key).await
                    }
                }
            }
            CacheState::Degraded | CacheState::Recovering => {
                // Use memory cache in degraded mode
                self.get_from_memory(key).await
            }
        }
    }
    
    /// Set a value in cache with automatic fallback
    pub async fn set(&self, key: &str, value: Value, ttl: Option<Duration>) -> Result<(), CacheError> {
        let current_state = self.state.read().await.clone();
        let ttl = ttl.unwrap_or(self.config.memory_cache_ttl);
        
        match current_state {
            CacheState::Healthy => {
                // Try Redis first
                match self.set_in_redis(key, &value, ttl).await {
                    Ok(_) => {
                        self.record_redis_success().await;
                        
                        // Also store in memory as backup
                        self.set_in_memory(key, value, ttl).await;
                        Ok(())
                    }
                    Err(e) => {
                        self.record_redis_failure().await;
                        self.check_degradation().await;
                        
                        // Fallback to memory cache
                        self.set_in_memory(key, value, ttl).await;
                        Ok(())
                    }
                }
            }
            CacheState::Degraded | CacheState::Recovering => {
                // Use memory cache in degraded mode
                self.set_in_memory(key, value, ttl).await;
                Ok(())
            }
        }
    }
    
    /// Delete a value from cache
    pub async fn delete(&self, key: &str) -> Result<(), CacheError> {
        let current_state = self.state.read().await.clone();
        
        match current_state {
            CacheState::Healthy => {
                // Try Redis first
                let redis_result = self.delete_from_redis(key).await;
                if redis_result.is_err() {
                    self.record_redis_failure().await;
                    self.check_degradation().await;
                } else {
                    self.record_redis_success().await;
                }
                
                // Always delete from memory cache too
                self.delete_from_memory(key).await;
                Ok(())
            }
            CacheState::Degraded | CacheState::Recovering => {
                // Use memory cache in degraded mode
                self.delete_from_memory(key).await;
                Ok(())
            }
        }
    }
    
    /// Clear all cache entries
    pub async fn clear(&self) -> Result<(), CacheError> {
        let current_state = self.state.read().await.clone();
        
        match current_state {
            CacheState::Healthy => {
                // Try Redis first
                let redis_result = self.clear_redis().await;
                if redis_result.is_err() {
                    self.record_redis_failure().await;
                    self.check_degradation().await;
                } else {
                    self.record_redis_success().await;
                }
                
                // Always clear memory cache too
                self.clear_memory().await;
                Ok(())
            }
            CacheState::Degraded | CacheState::Recovering => {
                // Use memory cache in degraded mode
                self.clear_memory().await;
                Ok(())
            }
        }
    }
    
    async fn get_from_redis(&self, key: &str) -> Result<Option<Value>, CacheError> {
        let cache = self.redis_cache.lock().await;
        
        // Try to get from Redis with a timeout
        match tokio::time::timeout(Duration::from_secs(2), cache.get_cached_query_result(key)).await {
            Ok(Ok(value)) => {
                self.update_stats(|stats| {
                    if value.is_some() {
                        stats.redis_cache_hits += 1;
                    } else {
                        stats.redis_cache_misses += 1;
                    }
                }).await;
                Ok(value)
            }
            Ok(Err(e)) => Err(CacheError::Redis(format!("Redis error: {}", e))),
            Err(_) => Err(CacheError::Timeout("Redis get timeout".to_string())),
        }
    }
    
    async fn set_in_redis(&self, key: &str, value: &Value, ttl: Duration) -> Result<(), CacheError> {
        let cache = self.redis_cache.lock().await;
        
        // Try to set in Redis with a timeout
        match tokio::time::timeout(Duration::from_secs(2), cache.cache_query_result(key, value, Some(ttl))).await {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(e)) => Err(CacheError::Redis(format!("Redis set error: {}", e))),
            Err(_) => Err(CacheError::Timeout("Redis set timeout".to_string())),
        }
    }
    
    async fn delete_from_redis(&self, key: &str) -> Result<(), CacheError> {
        let cache = self.redis_cache.lock().await;
        
        // Try to delete from Redis with a timeout
        match tokio::time::timeout(Duration::from_secs(2), cache.invalidate_cache_pattern(key)).await {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(e)) => Err(CacheError::Redis(format!("Redis delete error: {}", e))),
            Err(_) => Err(CacheError::Timeout("Redis delete timeout".to_string())),
        }
    }
    
    async fn clear_redis(&self) -> Result<(), CacheError> {
        let cache = self.redis_cache.lock().await;
        
        // Try to clear Redis with a timeout
        match tokio::time::timeout(Duration::from_secs(5), cache.clear_cache()).await {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(e)) => Err(CacheError::Redis(format!("Redis clear error: {}", e))),
            Err(_) => Err(CacheError::Timeout("Redis clear timeout".to_string())),
        }
    }
    
    async fn get_from_memory(&self, key: &str) -> Result<Option<Value>, CacheError> {
        let mut cache = self.memory_cache.write().await;
        
        if let Some(entry) = cache.get(key) {
            if entry.is_expired() {
                cache.remove(key);
                self.update_stats(|stats| {
                    stats.memory_cache_misses += 1;
                    stats.memory_cache_entries = cache.len();
                }).await;
                Ok(None)
            } else {
                self.update_stats(|stats| {
                    stats.memory_cache_hits += 1;
                }).await;
                Ok(Some(entry.value.clone()))
            }
        } else {
            self.update_stats(|stats| {
                stats.memory_cache_misses += 1;
                stats.memory_cache_entries = cache.len();
            }).await;
            Ok(None)
        }
    }
    
    async fn set_in_memory(&self, key: &str, value: Value, ttl: Duration) {
        let mut cache = self.memory_cache.write().await;
        
        // Implement LRU eviction if cache is full
        if cache.len() >= self.config.max_memory_cache_size {
            // Remove expired entries first
            let expired_keys: Vec<String> = cache
                .iter()
                .filter(|(_, entry)| entry.is_expired())
                .map(|(k, _)| k.clone())
                .collect();
            
            for key in expired_keys {
                cache.remove(&key);
            }
            
            // If still full, remove oldest entries
            if cache.len() >= self.config.max_memory_cache_size {
                let mut entries: Vec<(String, Instant)> = cache
                    .iter()
                    .map(|(k, v)| (k.clone(), v.created_at))
                    .collect();
                
                entries.sort_by_key(|(_, created_at)| *created_at);
                
                let to_remove = entries.len() - self.config.max_memory_cache_size + 1;
                for (key, _) in entries.into_iter().take(to_remove) {
                    cache.remove(&key);
                }
            }
        }
        
        cache.insert(key.to_string(), CacheEntry::new(value, ttl));
        
        self.update_stats(|stats| {
            stats.memory_cache_entries = cache.len();
        }).await;
    }
    
    async fn delete_from_memory(&self, key: &str) {
        let mut cache = self.memory_cache.write().await;
        cache.remove(key);
        
        self.update_stats(|stats| {
            stats.memory_cache_entries = cache.len();
        }).await;
    }
    
    async fn clear_memory(&self) {
        let mut cache = self.memory_cache.write().await;
        cache.clear();
        
        self.update_stats(|stats| {
            stats.memory_cache_entries = 0;
        }).await;
    }
    
    async fn record_redis_success(&self) {
        let mut stats = self.stats.write().await;
        stats.redis_successes += 1;
        stats.last_redis_success = Some(Instant::now());
        
        // Reset failure count on success
        stats.redis_failures = 0;
    }
    
    async fn record_redis_failure(&self) {
        let mut stats = self.stats.write().await;
        stats.redis_failures += 1;
        stats.last_redis_failure = Some(Instant::now());
        
        // Reset success count on failure
        stats.redis_successes = 0;
    }
    
    async fn check_degradation(&self) {
        let stats = self.stats.read().await;
        let current_state = self.state.read().await.clone();
        
        if current_state == CacheState::Healthy 
            && stats.redis_failures >= self.config.consecutive_failures_threshold {
            drop(stats);
            
            warn!("Redis cache failing, switching to degraded mode with in-memory fallback");
            *self.state.write().await = CacheState::Degraded;
            *self.degraded_since.write().await = Some(Instant::now());
            
            self.update_stats(|stats| {
                stats.current_state = CacheState::Degraded;
            }).await;
        }
    }
    
    async fn check_recovery(&self) {
        let current_state = self.state.read().await.clone();
        
        if current_state == CacheState::Degraded {
            // Try a simple Redis operation to test recovery
            if self.test_redis_health().await {
                info!("Redis appears to be recovering, entering recovery mode");
                *self.state.write().await = CacheState::Recovering;
                
                self.update_stats(|stats| {
                    stats.current_state = CacheState::Recovering;
                    stats.recovery_attempts += 1;
                }).await;
            }
        } else if current_state == CacheState::Recovering {
            let stats = self.stats.read().await;
            if stats.redis_successes >= self.config.consecutive_successes_threshold {
                drop(stats);
                
                info!("Redis recovery confirmed, returning to healthy mode");
                *self.state.write().await = CacheState::Healthy;
                
                // Calculate degraded mode duration
                if let Some(degraded_since) = *self.degraded_since.read().await {
                    let degraded_duration = degraded_since.elapsed();
                    *self.degraded_since.write().await = None;
                    
                    self.update_stats(|stats| {
                        stats.current_state = CacheState::Healthy;
                        stats.degraded_mode_duration += degraded_duration;
                    }).await;
                }
            }
        }
    }
    
    async fn test_redis_health(&self) -> bool {
        let test_key = format!("health_check_{}", uuid::Uuid::new_v4());
        let test_value = serde_json::json!({"test": true});
        
        // Try a simple set/get/delete cycle
        match self.set_in_redis(&test_key, &test_value, Duration::from_secs(10)).await {
            Ok(_) => {
                match self.get_from_redis(&test_key).await {
                    Ok(Some(_)) => {
                        let _ = self.delete_from_redis(&test_key).await;
                        true
                    }
                    _ => false,
                }
            }
            Err(_) => false,
        }
    }
    
    async fn start_health_monitoring(&self) {
        let state = self.state.clone();
        let stats = self.stats.clone();
        let degraded_since = self.degraded_since.clone();
        let config = self.config.clone();
        let redis_cache = self.redis_cache.clone();
        
        tokio::spawn(async move {
            let mut health_interval = tokio::time::interval(config.health_check_interval);
            let mut recovery_interval = tokio::time::interval(config.recovery_check_interval);
            
            loop {
                tokio::select! {
                    _ = health_interval.tick() => {
                        // Cleanup expired memory cache entries
                        // This is done in a separate task to avoid blocking
                    }
                    
                    _ = recovery_interval.tick() => {
                        let current_state = state.read().await.clone();
                        if current_state != CacheState::Healthy {
                            // Test Redis recovery
                            let test_key = format!("health_check_{}", uuid::Uuid::new_v4());
                            let test_value = serde_json::json!({"test": true});
                            
                            let cache = redis_cache.lock().await;
                            let health_ok = match tokio::time::timeout(
                                Duration::from_secs(2), 
                                cache.cache_query_result(&test_key, &test_value, Some(Duration::from_secs(10)))
                            ).await {
                                Ok(Ok(_)) => {
                                    // Test get as well
                                    match tokio::time::timeout(
                                        Duration::from_secs(2),
                                        cache.get_cached_query_result(&test_key)
                                    ).await {
                                        Ok(Ok(Some(_))) => {
                                            let _ = cache.invalidate_cache_pattern(&test_key).await;
                                            true
                                        }
                                        _ => false,
                                    }
                                }
                                _ => false,
                            };
                            
                            drop(cache);
                            
                            if health_ok {
                                let mut stats_guard = stats.write().await;
                                stats_guard.redis_successes += 1;
                                stats_guard.last_redis_success = Some(Instant::now());
                                stats_guard.redis_failures = 0;
                                
                                if current_state == CacheState::Degraded {
                                    info!("Redis recovery detected, entering recovery mode");
                                    *state.write().await = CacheState::Recovering;
                                    stats_guard.current_state = CacheState::Recovering;
                                    stats_guard.recovery_attempts += 1;
                                } else if current_state == CacheState::Recovering 
                                    && stats_guard.redis_successes >= config.consecutive_successes_threshold {
                                    info!("Redis recovery confirmed, returning to healthy mode");
                                    *state.write().await = CacheState::Healthy;
                                    stats_guard.current_state = CacheState::Healthy;
                                    
                                    if let Some(degraded_start) = *degraded_since.read().await {
                                        stats_guard.degraded_mode_duration += degraded_start.elapsed();
                                        *degraded_since.write().await = None;
                                    }
                                }
                            } else if current_state == CacheState::Recovering {
                                // Recovery failed, go back to degraded
                                warn!("Redis recovery test failed, returning to degraded mode");
                                *state.write().await = CacheState::Degraded;
                                
                                let mut stats_guard = stats.write().await;
                                stats_guard.current_state = CacheState::Degraded;
                                stats_guard.redis_failures += 1;
                                stats_guard.last_redis_failure = Some(Instant::now());
                                stats_guard.redis_successes = 0;
                            }
                        }
                    }
                }
            }
        });
    }
    
    async fn update_stats<F>(&self, update_fn: F)
    where
        F: FnOnce(&mut CacheDegradationStats),
    {
        let mut stats = self.stats.write().await;
        update_fn(&mut *stats);
    }
    
    pub async fn get_stats(&self) -> CacheDegradationStats {
        self.stats.read().await.clone()
    }
    
    pub async fn get_state(&self) -> CacheState {
        self.state.read().await.clone()
    }
    
    pub async fn is_healthy(&self) -> bool {
        matches!(*self.state.read().await, CacheState::Healthy)
    }
    
    pub async fn force_degraded_mode(&self) {
        warn!("Manually forcing cache degraded mode");
        *self.state.write().await = CacheState::Degraded;
        *self.degraded_since.write().await = Some(Instant::now());
        
        self.update_stats(|stats| {
            stats.current_state = CacheState::Degraded;
        }).await;
    }
    
    pub async fn force_recovery_test(&self) {
        info!("Manually triggering cache recovery test");
        self.check_recovery().await;
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CacheError {
    #[error("Redis error: {0}")]
    Redis(String),
    
    #[error("Timeout error: {0}")]
    Timeout(String),
    
    #[error("Serialization error: {0}")]
    Serialization(String),
    
    #[error("Memory cache full")]
    MemoryFull,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::sleep;
    
    // Note: These are integration tests that would need a real Redis instance
    // In a real implementation, you'd use a mock Redis for unit tests
    
    #[tokio::test]
    async fn test_cache_degradation_concept() {
        let mut config = CacheDegradationConfig::default();
        config.consecutive_failures_threshold = 2;
        config.max_memory_cache_size = 10;
        
        // This test would need a mock CacheManager
        // For now, just test the configuration and data structures
        
        assert_eq!(config.consecutive_failures_threshold, 2);
        assert_eq!(config.max_memory_cache_size, 10);
        
        let entry = CacheEntry::new(serde_json::json!({"test": true}), Duration::from_secs(1));
        assert!(!entry.is_expired());
        
        sleep(Duration::from_millis(1100)).await;
        assert!(entry.is_expired());
    }
}
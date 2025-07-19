use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{RwLock, Mutex};
use tracing::{debug, warn, info};

use crate::knowledge_graph::CodeEntity;

/// Configuration for the Entity LRU Cache
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityCacheConfig {
    /// Maximum number of entities to cache
    pub max_entries: usize,
    
    /// TTL for cached entities (seconds)
    pub ttl_seconds: u64,
    
    /// Enable cache warming on startup
    pub enable_warming: bool,
    
    /// Maximum memory usage for cache (MB)
    pub max_memory_mb: f64,
    
    /// Enable automatic cache cleanup
    pub enable_cleanup: bool,
    
    /// Cleanup interval (seconds)
    pub cleanup_interval_seconds: u64,
    
    /// Enable cache compression for large entities
    pub enable_compression: bool,
    
    /// Compression threshold (bytes)
    pub compression_threshold: usize,
}

impl Default for EntityCacheConfig {
    fn default() -> Self {
        Self {
            max_entries: 10000,
            ttl_seconds: 3600, // 1 hour
            enable_warming: true,
            max_memory_mb: 100.0,
            enable_cleanup: true,
            cleanup_interval_seconds: 300, // 5 minutes
            enable_compression: true,
            compression_threshold: 1024, // 1KB
        }
    }
}

/// Cache entry with metadata
#[derive(Debug, Clone)]
struct CacheEntry {
    entity: CodeEntity,
    last_accessed: Instant,
    access_count: u64,
    created_at: Instant,
    compressed: bool,
    size_bytes: usize,
}

impl CacheEntry {
    fn new(entity: CodeEntity) -> Self {
        let size_bytes = estimate_entity_size(&entity);
        Self {
            entity,
            last_accessed: Instant::now(),
            access_count: 1,
            created_at: Instant::now(),
            compressed: false,
            size_bytes,
        }
    }
    
    fn touch(&mut self) {
        self.last_accessed = Instant::now();
        self.access_count += 1;
    }
    
    fn is_expired(&self, ttl: Duration) -> bool {
        self.created_at.elapsed() > ttl
    }
    
    fn score(&self) -> f64 {
        // Score based on access frequency and recency
        let recency_factor = 1.0 / (1.0 + self.last_accessed.elapsed().as_secs() as f64);
        let frequency_factor = (self.access_count as f64).ln();
        recency_factor * frequency_factor
    }
}

/// Statistics for cache performance monitoring
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EntityCacheStats {
    pub total_entries: usize,
    pub memory_usage_mb: f64,
    pub hit_rate: f64,
    pub miss_rate: f64,
    pub eviction_count: u64,
    pub compression_ratio: f64,
    pub average_access_time_us: f64,
    pub cache_efficiency: f64,
    pub total_requests: u64,
    pub total_hits: u64,
    pub total_misses: u64,
    pub cleanup_count: u64,
    pub last_cleanup: Option<Instant>,
}

/// High-performance LRU cache for code entities
pub struct EntityLRUCache {
    config: EntityCacheConfig,
    cache: Arc<RwLock<HashMap<String, CacheEntry>>>,
    access_order: Arc<Mutex<VecDeque<String>>>,
    stats: Arc<RwLock<EntityCacheStats>>,
    
    // Performance optimizations
    key_lookup: Arc<RwLock<HashMap<String, String>>>, // Fast key normalization
    hot_keys: Arc<RwLock<Vec<String>>>, // Most frequently accessed keys
    size_tracker: Arc<RwLock<f64>>, // Track total memory usage
    
    // Background tasks
    cleanup_handle: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
}

impl EntityLRUCache {
    pub async fn new(config: EntityCacheConfig) -> Self {
        let cache = Arc::new(RwLock::new(HashMap::new()));
        let access_order = Arc::new(Mutex::new(VecDeque::new()));
        let stats = Arc::new(RwLock::new(EntityCacheStats::default()));
        let key_lookup = Arc::new(RwLock::new(HashMap::new()));
        let hot_keys = Arc::new(RwLock::new(Vec::new()));
        let size_tracker = Arc::new(RwLock::new(0.0));
        
        let entity_cache = Self {
            config: config.clone(),
            cache,
            access_order,
            stats,
            key_lookup,
            hot_keys,
            size_tracker,
            cleanup_handle: Arc::new(Mutex::new(None)),
        };
        
        // Start background cleanup task
        if config.enable_cleanup {
            entity_cache.start_cleanup_task().await;
        }
        
        info!("Entity LRU Cache initialized with {} max entries", config.max_entries);
        
        entity_cache
    }
    
    /// Get entity from cache
    pub async fn get(&self, key: &str) -> Option<CodeEntity> {
        let start_time = Instant::now();
        
        // Normalize key
        let normalized_key = self.normalize_key(key).await;
        
        let mut cache = self.cache.write().await;
        let mut stats = self.stats.write().await;
        
        stats.total_requests += 1;
        
        if let Some(entry) = cache.get_mut(&normalized_key) {
            if entry.is_expired(Duration::from_secs(self.config.ttl_seconds)) {
                // Remove expired entry
                cache.remove(&normalized_key);
                self.remove_from_access_order(&normalized_key).await;
                stats.total_misses += 1;
                stats.miss_rate = stats.total_misses as f64 / stats.total_requests as f64;
                None
            } else {
                // Cache hit
                entry.touch();
                self.update_access_order(&normalized_key).await;
                
                stats.total_hits += 1;
                stats.hit_rate = stats.total_hits as f64 / stats.total_requests as f64;
                
                let access_time = start_time.elapsed().as_micros() as f64;
                stats.average_access_time_us = 
                    (stats.average_access_time_us * (stats.total_requests - 1) as f64 + access_time) / stats.total_requests as f64;
                
                debug!("Cache hit for entity: {}", normalized_key);
                Some(entry.entity.clone())
            }
        } else {
            // Cache miss
            stats.total_misses += 1;
            stats.miss_rate = stats.total_misses as f64 / stats.total_requests as f64;
            debug!("Cache miss for entity: {}", normalized_key);
            None
        }
    }
    
    /// Put entity into cache
    pub async fn put(&self, key: &str, entity: CodeEntity) -> Result<()> {
        let normalized_key = self.normalize_key(key).await;
        
        let mut cache = self.cache.write().await;
        let mut stats = self.stats.write().await;
        
        // Check if we need to evict entries
        if cache.len() >= self.config.max_entries {
            self.evict_lru(&mut cache, &mut stats).await;
        }
        
        // Check memory usage
        let current_memory = *self.size_tracker.read().await;
        if current_memory > self.config.max_memory_mb {
            self.evict_by_memory(&mut cache, &mut stats).await;
        }
        
        // Create cache entry
        let mut entry = CacheEntry::new(entity);
        
        // Compress if enabled and size exceeds threshold
        if self.config.enable_compression && entry.size_bytes > self.config.compression_threshold {
            entry.compressed = true;
            // In a real implementation, you'd compress the entity data here
            debug!("Compressed entity: {}", normalized_key);
        }
        
        // Update size tracking
        let entry_size_mb = entry.size_bytes as f64 / (1024.0 * 1024.0);
        *self.size_tracker.write().await += entry_size_mb;
        
        // Insert into cache
        cache.insert(normalized_key.clone(), entry);
        
        // Update access order
        self.add_to_access_order(&normalized_key).await;
        
        // Update stats
        stats.total_entries = cache.len();
        stats.memory_usage_mb = *self.size_tracker.read().await;
        
        debug!("Cached entity: {}", normalized_key);
        Ok(())
    }
    
    /// Remove entity from cache
    pub async fn remove(&self, key: &str) -> bool {
        let normalized_key = self.normalize_key(key).await;
        
        let mut cache = self.cache.write().await;
        let mut stats = self.stats.write().await;
        
        if let Some(entry) = cache.remove(&normalized_key) {
            // Update size tracking
            let entry_size_mb = entry.size_bytes as f64 / (1024.0 * 1024.0);
            *self.size_tracker.write().await -= entry_size_mb;
            
            // Remove from access order
            self.remove_from_access_order(&normalized_key).await;
            
            // Update stats
            stats.total_entries = cache.len();
            stats.memory_usage_mb = *self.size_tracker.read().await;
            
            debug!("Removed entity from cache: {}", normalized_key);
            true
        } else {
            false
        }
    }
    
    /// Clear all cached entities
    pub async fn clear(&self) {
        let mut cache = self.cache.write().await;
        let mut stats = self.stats.write().await;
        let mut access_order = self.access_order.lock().await;
        
        cache.clear();
        access_order.clear();
        *self.size_tracker.write().await = 0.0;
        
        stats.total_entries = 0;
        stats.memory_usage_mb = 0.0;
        
        info!("Entity cache cleared");
    }
    
    /// Get cache statistics
    pub async fn get_stats(&self) -> EntityCacheStats {
        let mut stats = self.stats.read().await.clone();
        
        // Calculate cache efficiency (hit rate weighted by recency)
        if stats.total_requests > 0 {
            stats.cache_efficiency = stats.hit_rate * 0.8 + 
                (1.0 - stats.memory_usage_mb / self.config.max_memory_mb) * 0.2;
        }
        
        // Calculate compression ratio
        let cache = self.cache.read().await;
        let compressed_entries = cache.values().filter(|e| e.compressed).count();
        if !cache.is_empty() {
            stats.compression_ratio = compressed_entries as f64 / cache.len() as f64;
        }
        
        stats
    }
    
    /// Warm cache with frequently accessed entities
    pub async fn warm_cache(&self, entities: Vec<(String, CodeEntity)>) -> Result<()> {
        if !self.config.enable_warming {
            return Ok(());
        }
        
        info!("Warming entity cache with {} entities", entities.len());
        
        for (key, entity) in entities {
            self.put(&key, entity).await?;
        }
        
        info!("Cache warming completed");
        Ok(())
    }
    
    /// Get hot keys (most frequently accessed)
    pub async fn get_hot_keys(&self, limit: usize) -> Vec<String> {
        let cache = self.cache.read().await;
        let mut entries: Vec<(String, f64)> = cache
            .iter()
            .map(|(key, entry)| (key.clone(), entry.score()))
            .collect();
        
        entries.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        entries.into_iter()
            .take(limit)
            .map(|(key, _)| key)
            .collect()
    }
    
    /// Force cleanup of expired entries
    pub async fn cleanup_expired(&self) -> usize {
        let mut cache = self.cache.write().await;
        let mut stats = self.stats.write().await;
        let ttl = Duration::from_secs(self.config.ttl_seconds);
        
        let initial_size = cache.len();
        let expired_keys: Vec<String> = cache
            .iter()
            .filter(|(_, entry)| entry.is_expired(ttl))
            .map(|(key, _)| key.clone())
            .collect();
        
        for key in &expired_keys {
            if let Some(entry) = cache.remove(key) {
                let entry_size_mb = entry.size_bytes as f64 / (1024.0 * 1024.0);
                *self.size_tracker.write().await -= entry_size_mb;
                self.remove_from_access_order(key).await;
            }
        }
        
        let cleaned = initial_size - cache.len();
        stats.cleanup_count += 1;
        stats.last_cleanup = Some(Instant::now());
        stats.total_entries = cache.len();
        stats.memory_usage_mb = *self.size_tracker.read().await;
        
        if cleaned > 0 {
            info!("Cleaned up {} expired entities from cache", cleaned);
        }
        
        cleaned
    }
    
    // Private helper methods
    
    async fn normalize_key(&self, key: &str) -> String {
        // Simple key normalization - could be more sophisticated
        key.to_lowercase().replace(' ', "_")
    }
    
    async fn evict_lru(&self, cache: &mut HashMap<String, CacheEntry>, stats: &mut EntityCacheStats) {
        let mut access_order = self.access_order.lock().await;
        
        if let Some(lru_key) = access_order.pop_front() {
            if let Some(entry) = cache.remove(&lru_key) {
                let entry_size_mb = entry.size_bytes as f64 / (1024.0 * 1024.0);
                *self.size_tracker.write().await -= entry_size_mb;
                
                stats.eviction_count += 1;
                debug!("Evicted LRU entity: {}", lru_key);
            }
        }
    }
    
    async fn evict_by_memory(&self, cache: &mut HashMap<String, CacheEntry>, stats: &mut EntityCacheStats) {
        // Evict largest entries first until memory usage is under limit
        let mut entries: Vec<(String, usize)> = cache
            .iter()
            .map(|(key, entry)| (key.clone(), entry.size_bytes))
            .collect();
        
        entries.sort_by(|a, b| b.1.cmp(&a.1));
        
        let target_memory = self.config.max_memory_mb * 0.8; // Leave 20% buffer
        let mut current_memory = *self.size_tracker.read().await;
        
        for (key, size_bytes) in entries {
            if current_memory <= target_memory {
                break;
            }
            
            if cache.remove(&key).is_some() {
                let entry_size_mb = size_bytes as f64 / (1024.0 * 1024.0);
                current_memory -= entry_size_mb;
                *self.size_tracker.write().await -= entry_size_mb;
                self.remove_from_access_order(&key).await;
                
                stats.eviction_count += 1;
                debug!("Evicted large entity: {} ({} bytes)", key, size_bytes);
            }
        }
    }
    
    async fn update_access_order(&self, key: &str) {
        let mut access_order = self.access_order.lock().await;
        
        // Remove from current position and add to end
        access_order.retain(|k| k != key);
        access_order.push_back(key.to_string());
    }
    
    async fn add_to_access_order(&self, key: &str) {
        let mut access_order = self.access_order.lock().await;
        access_order.push_back(key.to_string());
    }
    
    async fn remove_from_access_order(&self, key: &str) {
        let mut access_order = self.access_order.lock().await;
        access_order.retain(|k| k != key);
    }
    
    async fn start_cleanup_task(&self) {
        let cache_clone = self.cache.clone();
        let stats_clone = self.stats.clone();
        let size_tracker_clone = self.size_tracker.clone();
        let access_order_clone = self.access_order.clone();
        let config = self.config.clone();
        
        let handle = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(config.cleanup_interval_seconds));
            
            loop {
                interval.tick().await;
                
                // Cleanup expired entries
                let mut cache = cache_clone.write().await;
                let mut stats = stats_clone.write().await;
                let ttl = Duration::from_secs(config.ttl_seconds);
                
                let initial_size = cache.len();
                let expired_keys: Vec<String> = cache
                    .iter()
                    .filter(|(_, entry)| entry.is_expired(ttl))
                    .map(|(key, _)| key.clone())
                    .collect();
                
                for key in &expired_keys {
                    if let Some(entry) = cache.remove(key) {
                        let entry_size_mb = entry.size_bytes as f64 / (1024.0 * 1024.0);
                        *size_tracker_clone.write().await -= entry_size_mb;
                        
                        let mut access_order = access_order_clone.lock().await;
                        access_order.retain(|k| k != key);
                    }
                }
                
                let cleaned = initial_size - cache.len();
                if cleaned > 0 {
                    stats.cleanup_count += 1;
                    stats.last_cleanup = Some(Instant::now());
                    stats.total_entries = cache.len();
                    stats.memory_usage_mb = *size_tracker_clone.read().await;
                    
                    debug!("Background cleanup removed {} expired entities", cleaned);
                }
            }
        });
        
        *self.cleanup_handle.lock().await = Some(handle);
    }
}

impl Drop for EntityLRUCache {
    fn drop(&mut self) {
        // Cancel background task if it exists
        if let Ok(mut handle) = self.cleanup_handle.try_lock() {
            if let Some(task) = handle.take() {
                task.abort();
            }
        }
    }
}

/// Estimate the memory size of a CodeEntity
fn estimate_entity_size(entity: &CodeEntity) -> usize {
    let base_size = std::mem::size_of::<CodeEntity>();
    let name_size = entity.name.len();
    let entity_type_size = entity.entity_type.len();
    let file_path_size = entity.file_path.as_ref().map(|p| p.len()).unwrap_or(0);
    let content_size = entity.content.as_ref().map(|c| c.len()).unwrap_or(0);
    let metadata_size = entity.metadata.iter()
        .map(|(k, v)| k.len() + v.len())
        .sum::<usize>();
    
    base_size + name_size + entity_type_size + file_path_size + content_size + metadata_size
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::knowledge_graph::CodeEntity;
    use std::collections::HashMap;
    
    #[tokio::test]
    async fn test_entity_cache_basic_operations() {
        let config = EntityCacheConfig::default();
        let cache = EntityLRUCache::new(config).await;
        
        let entity = CodeEntity {
            id: "test-id".to_string(),
            name: "test_function".to_string(),
            entity_type: "function".to_string(),
            file_path: Some("test.rs".to_string()),
            content: Some("fn test() {}".to_string()),
            metadata: HashMap::new(),
        };
        
        // Test put and get
        cache.put("test_key", entity.clone()).await.unwrap();
        let retrieved = cache.get("test_key").await;
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().name, entity.name);
        
        // Test cache miss
        let missing = cache.get("nonexistent_key").await;
        assert!(missing.is_none());
        
        // Test remove
        assert!(cache.remove("test_key").await);
        assert!(cache.get("test_key").await.is_none());
    }
    
    #[tokio::test]
    async fn test_cache_eviction() {
        let mut config = EntityCacheConfig::default();
        config.max_entries = 2;
        let cache = EntityLRUCache::new(config).await;
        
        let entity1 = CodeEntity {
            id: "id1".to_string(),
            name: "entity1".to_string(),
            entity_type: "function".to_string(),
            file_path: None,
            content: None,
            metadata: HashMap::new(),
        };
        
        let entity2 = entity1.clone();
        let entity3 = entity1.clone();
        
        // Fill cache to capacity
        cache.put("key1", entity1).await.unwrap();
        cache.put("key2", entity2).await.unwrap();
        
        // This should evict key1 (LRU)
        cache.put("key3", entity3).await.unwrap();
        
        assert!(cache.get("key1").await.is_none());
        assert!(cache.get("key2").await.is_some());
        assert!(cache.get("key3").await.is_some());
    }
    
    #[tokio::test]
    async fn test_cache_stats() {
        let config = EntityCacheConfig::default();
        let cache = EntityLRUCache::new(config).await;
        
        let entity = CodeEntity {
            id: "test-id".to_string(),
            name: "test_function".to_string(),
            entity_type: "function".to_string(),
            file_path: None,
            content: None,
            metadata: HashMap::new(),
        };
        
        // Test stats tracking
        cache.put("test_key", entity).await.unwrap();
        let _ = cache.get("test_key").await; // Hit
        let _ = cache.get("missing_key").await; // Miss
        
        let stats = cache.get_stats().await;
        assert_eq!(stats.total_requests, 2);
        assert_eq!(stats.total_hits, 1);
        assert_eq!(stats.total_misses, 1);
        assert_eq!(stats.hit_rate, 0.5);
    }
}
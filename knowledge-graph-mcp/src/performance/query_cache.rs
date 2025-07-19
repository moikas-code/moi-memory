use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{debug, warn, info};
use sha2::{Sha256, Digest};

/// Configuration for query plan caching
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryCacheConfig {
    /// Maximum number of cached plans
    pub max_cache_size: usize,
    /// TTL for cached plans in seconds
    pub plan_ttl_seconds: u64,
    /// Enable query normalization
    pub enable_normalization: bool,
    /// Enable statistics collection
    pub enable_statistics: bool,
    /// Cache cleanup interval in seconds
    pub cleanup_interval_seconds: u64,
    /// Maximum query length to cache
    pub max_query_length: usize,
    /// Enable parameter-based caching
    pub enable_parameter_caching: bool,
    /// Maximum parameter cache size
    pub max_parameter_cache_size: usize,
    /// Enable adaptive cache sizing based on memory pressure
    pub enable_adaptive_sizing: bool,
}

impl Default for QueryCacheConfig {
    fn default() -> Self {
        Self {
            max_cache_size: 10000,
            plan_ttl_seconds: 3600, // 1 hour
            enable_normalization: true,
            enable_statistics: true,
            cleanup_interval_seconds: 300, // 5 minutes
            max_query_length: 50000,
            enable_parameter_caching: true,
            max_parameter_cache_size: 5000,
            enable_adaptive_sizing: true,
        }
    }
}

/// Cached query plan
#[derive(Debug, Clone)]
pub struct CachedPlan {
    /// Original query
    pub query: String,
    /// Normalized query (for matching similar queries)
    pub normalized_query: String,
    /// Execution plan hint
    pub plan_hint: String,
    /// Estimated cost
    pub estimated_cost: f64,
    /// Expected row count
    pub expected_rows: Option<u64>,
    /// Cached at timestamp
    pub cached_at: Instant,
    /// Last accessed timestamp
    pub last_accessed: Instant,
    /// Access count
    pub access_count: u64,
    /// Average execution time in milliseconds
    pub avg_execution_time_ms: f64,
    /// Parameters used (if parameter caching enabled)
    pub parameters: Option<HashMap<String, Value>>,
}

impl CachedPlan {
    /// Check if the plan is expired
    pub fn is_expired(&self, ttl: Duration) -> bool {
        Instant::now().duration_since(self.cached_at) > ttl
    }
    
    /// Update access statistics
    pub fn update_access(&mut self, execution_time_ms: f64) {
        self.last_accessed = Instant::now();
        self.access_count += 1;
        
        // Update rolling average execution time
        let alpha = 0.1; // Smoothing factor
        self.avg_execution_time_ms = alpha * execution_time_ms + (1.0 - alpha) * self.avg_execution_time_ms;
    }
    
    /// Calculate cache value score (for eviction decisions)
    pub fn cache_value_score(&self) -> f64 {
        let age_factor = 1.0 / (1.0 + self.cached_at.elapsed().as_secs() as f64 / 3600.0);
        let frequency_factor = (self.access_count as f64).ln();
        let recency_factor = 1.0 / (1.0 + self.last_accessed.elapsed().as_secs() as f64 / 3600.0);
        let performance_factor = 1.0 / (1.0 + self.avg_execution_time_ms / 1000.0);
        
        age_factor * frequency_factor * recency_factor * performance_factor
    }
}

/// Query cache statistics
#[derive(Debug, Clone, Serialize)]
pub struct QueryCacheStats {
    pub total_queries: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub hit_ratio: f64,
    pub cached_plans: usize,
    pub evicted_plans: u64,
    pub average_lookup_time_ns: f64,
    pub memory_usage_bytes: usize,
    pub top_queries: Vec<TopQuery>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TopQuery {
    pub normalized_query: String,
    pub access_count: u64,
    pub avg_execution_time_ms: f64,
    pub cache_value_score: f64,
}

/// LRU eviction queue entry
#[derive(Debug, Clone)]
struct EvictionEntry {
    query_hash: String,
    last_accessed: Instant,
    cache_value_score: f64,
}

/// Query plan cache with LRU eviction and statistics
pub struct QueryPlanCache {
    config: QueryCacheConfig,
    
    /// Main cache storage
    cache: Arc<RwLock<HashMap<String, CachedPlan>>>,
    
    /// Parameter-based cache for parameterized queries
    parameter_cache: Arc<RwLock<HashMap<String, HashMap<String, CachedPlan>>>>,
    
    /// LRU tracking for eviction
    eviction_queue: Arc<RwLock<VecDeque<EvictionEntry>>>,
    
    /// Cache statistics
    stats: Arc<RwLock<QueryCacheStats>>,
    
    /// Query normalizer
    normalizer: QueryNormalizer,
}

impl QueryPlanCache {
    /// Create a new query plan cache
    pub async fn new(config: QueryCacheConfig) -> Self {
        let cache = Self {
            config: config.clone(),
            cache: Arc::new(RwLock::new(HashMap::new())),
            parameter_cache: Arc::new(RwLock::new(HashMap::new())),
            eviction_queue: Arc::new(RwLock::new(VecDeque::new())),
            stats: Arc::new(RwLock::new(QueryCacheStats {
                total_queries: 0,
                cache_hits: 0,
                cache_misses: 0,
                hit_ratio: 0.0,
                cached_plans: 0,
                evicted_plans: 0,
                average_lookup_time_ns: 0.0,
                memory_usage_bytes: 0,
                top_queries: Vec::new(),
            })),
            normalizer: QueryNormalizer::new(),
        };
        
        // Start background cleanup task
        cache.start_cleanup_task().await;
        
        cache
    }
    
    /// Get cached plan for a query
    pub async fn get_plan(&self, query: &str, parameters: Option<&HashMap<String, Value>>) -> Option<CachedPlan> {
        let start_time = Instant::now();
        
        // Skip caching for very long queries
        if query.len() > self.config.max_query_length {
            return None;
        }
        
        let query_hash = self.hash_query(query);
        let normalized_query = if self.config.enable_normalization {
            self.normalizer.normalize(query)
        } else {
            query.to_string()
        };
        
        // Try main cache first
        let mut cache_hit = false;
        let mut cached_plan = None;
        
        {
            let mut cache = self.cache.write().await;
            if let Some(plan) = cache.get_mut(&query_hash) {
                if !plan.is_expired(Duration::from_secs(self.config.plan_ttl_seconds)) {
                    cached_plan = Some(plan.clone());
                    cache_hit = true;
                } else {
                    // Remove expired plan
                    cache.remove(&query_hash);
                }
            }
        }
        
        // Try parameter cache if enabled and no hit in main cache
        if !cache_hit && self.config.enable_parameter_caching {
            if let Some(params) = parameters {
                let param_hash = self.hash_parameters(params);
                let param_cache = self.parameter_cache.read().await;
                
                if let Some(param_plans) = param_cache.get(&normalized_query) {
                    if let Some(plan) = param_plans.get(&param_hash) {
                        if !plan.is_expired(Duration::from_secs(self.config.plan_ttl_seconds)) {
                            cached_plan = Some(plan.clone());
                            cache_hit = true;
                        }
                    }
                }
            }
        }
        
        // Update statistics
        {
            let mut stats = self.stats.write().await;
            stats.total_queries += 1;
            
            if cache_hit {
                stats.cache_hits += 1;
            } else {
                stats.cache_misses += 1;
            }
            
            stats.hit_ratio = stats.cache_hits as f64 / stats.total_queries as f64;
            
            // Update average lookup time
            let lookup_time_ns = start_time.elapsed().as_nanos() as f64;
            let alpha = 0.1;
            stats.average_lookup_time_ns = alpha * lookup_time_ns + (1.0 - alpha) * stats.average_lookup_time_ns;
        }
        
        if let Some(mut plan) = cached_plan {
            // Update access statistics
            plan.update_access(0.0); // Execution time will be updated later
            
            // Update LRU queue
            self.update_eviction_queue(&query_hash, &plan).await;
            
            Some(plan)
        } else {
            None
        }
    }
    
    /// Cache a query plan
    pub async fn cache_plan(&self, query: &str, plan_hint: &str, estimated_cost: f64, 
                           expected_rows: Option<u64>, parameters: Option<&HashMap<String, Value>>) -> Result<()> {
        // Skip caching for very long queries
        if query.len() > self.config.max_query_length {
            return Ok(());
        }
        
        let query_hash = self.hash_query(query);
        let normalized_query = if self.config.enable_normalization {
            self.normalizer.normalize(query)
        } else {
            query.to_string()
        };
        
        let cached_plan = CachedPlan {
            query: query.to_string(),
            normalized_query: normalized_query.clone(),
            plan_hint: plan_hint.to_string(),
            estimated_cost,
            expected_rows,
            cached_at: Instant::now(),
            last_accessed: Instant::now(),
            access_count: 0,
            avg_execution_time_ms: 0.0,
            parameters: parameters.cloned(),
        };
        
        // Cache in main cache
        {
            let mut cache = self.cache.write().await;
            
            // Check if we need to evict
            if cache.len() >= self.config.max_cache_size {
                self.evict_least_valuable(&mut cache).await;
            }
            
            cache.insert(query_hash.clone(), cached_plan.clone());
        }
        
        // Cache in parameter cache if enabled
        if self.config.enable_parameter_caching {
            if let Some(params) = parameters {
                let param_hash = self.hash_parameters(params);
                let mut param_cache = self.parameter_cache.write().await;
                
                let param_plans = param_cache.entry(normalized_query).or_insert_with(HashMap::new);
                
                // Check parameter cache size
                if param_plans.len() >= self.config.max_parameter_cache_size {
                    // Remove oldest entry
                    if let Some((oldest_key, _)) = param_plans.iter()
                        .min_by_key(|(_, plan)| plan.cached_at) {
                        let oldest_key = oldest_key.clone();
                        param_plans.remove(&oldest_key);
                    }
                }
                
                param_plans.insert(param_hash, cached_plan.clone());
            }
        }
        
        // Update eviction queue
        self.update_eviction_queue(&query_hash, &cached_plan).await;
        
        // Update statistics
        {
            let mut stats = self.stats.write().await;
            stats.cached_plans = self.cache.read().await.len();
        }
        
        debug!("Cached query plan for query hash: {}", query_hash);
        Ok(())
    }
    
    /// Update execution statistics for a cached plan
    pub async fn update_execution_stats(&self, query: &str, execution_time_ms: f64) {
        let query_hash = self.hash_query(query);
        
        let mut cache = self.cache.write().await;
        if let Some(plan) = cache.get_mut(&query_hash) {
            plan.update_access(execution_time_ms);
        }
    }
    
    /// Clear all cached plans
    pub async fn clear(&self) {
        let mut cache = self.cache.write().await;
        let mut param_cache = self.parameter_cache.write().await;
        let mut eviction_queue = self.eviction_queue.write().await;
        
        cache.clear();
        param_cache.clear();
        eviction_queue.clear();
        
        info!("Query plan cache cleared");
    }
    
    /// Get cache statistics
    pub async fn get_stats(&self) -> QueryCacheStats {
        let mut stats = self.stats.read().await.clone();
        
        // Update current cache size
        stats.cached_plans = self.cache.read().await.len();
        
        // Calculate memory usage estimation
        stats.memory_usage_bytes = self.estimate_memory_usage().await;
        
        // Get top queries
        stats.top_queries = self.get_top_queries(10).await;
        
        stats
    }
    
    /// Hash a query for caching
    fn hash_query(&self, query: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(query.as_bytes());
        format!("{:x}", hasher.finalize())
    }
    
    /// Hash parameters for parameter-based caching
    fn hash_parameters(&self, parameters: &HashMap<String, Value>) -> String {
        let mut hasher = Sha256::new();
        
        // Sort parameters for consistent hashing
        let mut sorted_params: Vec<_> = parameters.iter().collect();
        sorted_params.sort_by_key(|(k, _)| *k);
        
        for (key, value) in sorted_params {
            hasher.update(key.as_bytes());
            hasher.update(value.to_string().as_bytes());
        }
        
        format!("{:x}", hasher.finalize())
    }
    
    /// Update eviction queue for LRU tracking
    async fn update_eviction_queue(&self, query_hash: &str, plan: &CachedPlan) {
        let mut queue = self.eviction_queue.write().await;
        
        // Remove existing entry if present
        queue.retain(|entry| entry.query_hash != *query_hash);
        
        // Add new entry
        queue.push_back(EvictionEntry {
            query_hash: query_hash.to_string(),
            last_accessed: plan.last_accessed,
            cache_value_score: plan.cache_value_score(),
        });
    }
    
    /// Evict least valuable plan from cache
    async fn evict_least_valuable(&self, cache: &mut HashMap<String, CachedPlan>) {
        let mut queue = self.eviction_queue.write().await;
        
        // Find entry with lowest cache value score
        if let Some(min_entry_idx) = queue.iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| a.cache_value_score.partial_cmp(&b.cache_value_score).unwrap())
            .map(|(idx, _)| idx) {
            
            let entry = queue.remove(min_entry_idx).unwrap();
            cache.remove(&entry.query_hash);
            
            // Update statistics
            if let Ok(mut stats) = self.stats.try_write() {
                stats.evicted_plans += 1;
            }
            
            debug!("Evicted query plan: {}", entry.query_hash);
        }
    }
    
    /// Estimate memory usage of cache
    async fn estimate_memory_usage(&self) -> usize {
        let cache = self.cache.read().await;
        let param_cache = self.parameter_cache.read().await;
        
        let main_cache_size = cache.len() * std::mem::size_of::<CachedPlan>();
        let param_cache_size = param_cache.values()
            .map(|plans| plans.len() * std::mem::size_of::<CachedPlan>())
            .sum::<usize>();
        
        main_cache_size + param_cache_size
    }
    
    /// Get top queries by access count
    async fn get_top_queries(&self, limit: usize) -> Vec<TopQuery> {
        let cache = self.cache.read().await;
        
        let mut queries: Vec<_> = cache.values()
            .map(|plan| TopQuery {
                normalized_query: plan.normalized_query.clone(),
                access_count: plan.access_count,
                avg_execution_time_ms: plan.avg_execution_time_ms,
                cache_value_score: plan.cache_value_score(),
            })
            .collect();
        
        queries.sort_by(|a, b| b.access_count.cmp(&a.access_count));
        queries.truncate(limit);
        
        queries
    }
    
    /// Start background cleanup task
    async fn start_cleanup_task(&self) {
        let cache = self.cache.clone();
        let parameter_cache = self.parameter_cache.clone();
        let eviction_queue = self.eviction_queue.clone();
        let ttl = Duration::from_secs(self.config.plan_ttl_seconds);
        let cleanup_interval = Duration::from_secs(self.config.cleanup_interval_seconds);
        
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(cleanup_interval);
            
            loop {
                interval.tick().await;
                
                // Clean expired plans from main cache
                {
                    let mut cache = cache.write().await;
                    let mut eviction_queue = eviction_queue.write().await;
                    
                    let expired_keys: Vec<_> = cache.iter()
                        .filter(|(_, plan)| plan.is_expired(ttl))
                        .map(|(key, _)| key.clone())
                        .collect();
                    
                    for key in expired_keys {
                        cache.remove(&key);
                        eviction_queue.retain(|entry| entry.query_hash != key);
                    }
                }
                
                // Clean expired plans from parameter cache
                {
                    let mut param_cache = parameter_cache.write().await;
                    
                    for (_, param_plans) in param_cache.iter_mut() {
                        let expired_keys: Vec<_> = param_plans.iter()
                            .filter(|(_, plan)| plan.is_expired(ttl))
                            .map(|(key, _)| key.clone())
                            .collect();
                        
                        for key in expired_keys {
                            param_plans.remove(&key);
                        }
                    }
                    
                    // Remove empty parameter groups
                    param_cache.retain(|_, plans| !plans.is_empty());
                }
                
                debug!("Query cache cleanup completed");
            }
        });
    }
}

/// Query normalizer for identifying similar queries
pub struct QueryNormalizer;

impl QueryNormalizer {
    pub fn new() -> Self {
        Self
    }
    
    /// Normalize a Cypher query for caching
    pub fn normalize(&self, query: &str) -> String {
        let mut normalized = query.to_uppercase();
        
        // Remove extra whitespace
        normalized = self.normalize_whitespace(&normalized);
        
        // Normalize string literals
        normalized = self.normalize_literals(&normalized);
        
        // Normalize parameter placeholders
        normalized = self.normalize_parameters(&normalized);
        
        normalized
    }
    
    fn normalize_whitespace(&self, query: &str) -> String {
        // Replace multiple whitespace with single space
        let mut result = String::new();
        let mut prev_char = ' ';
        
        for ch in query.chars() {
            if ch.is_whitespace() {
                if prev_char != ' ' {
                    result.push(' ');
                    prev_char = ' ';
                }
            } else {
                result.push(ch);
                prev_char = ch;
            }
        }
        
        result.trim().to_string()
    }
    
    fn normalize_literals(&self, query: &str) -> String {
        // Replace string literals with placeholder
        let mut result = String::new();
        let mut chars = query.chars().peekable();
        let mut in_string = false;
        let mut string_char = '"';
        
        while let Some(ch) = chars.next() {
            if !in_string && (ch == '"' || ch == '\'') {
                in_string = true;
                string_char = ch;
                result.push_str("?STRING?");
            } else if in_string && ch == string_char {
                in_string = false;
                // Skip the closing quote
            } else if !in_string {
                result.push(ch);
            }
            // Skip characters inside strings
        }
        
        result
    }
    
    fn normalize_parameters(&self, query: &str) -> String {
        // Replace $param with ?PARAM?
        query.replace("$", "?PARAM?")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_query_cache_basic() {
        let config = QueryCacheConfig::default();
        let cache = QueryPlanCache::new(config).await;
        
        let query = "MATCH (n:Person) RETURN n";
        
        // Should be cache miss initially
        assert!(cache.get_plan(query, None).await.is_none());
        
        // Cache a plan
        cache.cache_plan(query, "NodeIndexSeek", 1.0, Some(10), None).await.unwrap();
        
        // Should be cache hit now
        let cached_plan = cache.get_plan(query, None).await;
        assert!(cached_plan.is_some());
        assert_eq!(cached_plan.unwrap().plan_hint, "NodeIndexSeek");
    }
    
    #[tokio::test]
    async fn test_query_normalization() {
        let normalizer = QueryNormalizer::new();
        
        let query1 = "MATCH (n:Person {name: 'John'}) RETURN n";
        let query2 = "MATCH (n:Person {name: 'Jane'}) RETURN n";
        
        let norm1 = normalizer.normalize(query1);
        let norm2 = normalizer.normalize(query2);
        
        // Should normalize to same pattern
        assert_eq!(norm1, norm2);
        assert!(norm1.contains("?STRING?"));
    }
    
    #[tokio::test]
    async fn test_cache_eviction() {
        let config = QueryCacheConfig {
            max_cache_size: 2,
            ..Default::default()
        };
        let cache = QueryPlanCache::new(config).await;
        
        // Fill cache to capacity
        cache.cache_plan("QUERY1", "plan1", 1.0, None, None).await.unwrap();
        cache.cache_plan("QUERY2", "plan2", 1.0, None, None).await.unwrap();
        
        // Add one more to trigger eviction
        cache.cache_plan("QUERY3", "plan3", 1.0, None, None).await.unwrap();
        
        let stats = cache.get_stats().await;
        assert_eq!(stats.cached_plans, 2);
        assert_eq!(stats.evicted_plans, 1);
    }
    
    #[tokio::test]
    async fn test_parameter_caching() {
        let config = QueryCacheConfig {
            enable_parameter_caching: true,
            ..Default::default()
        };
        let cache = QueryPlanCache::new(config).await;
        
        let query = "MATCH (n:Person {name: $name}) RETURN n";
        let mut params1 = HashMap::new();
        params1.insert("name".to_string(), Value::String("John".to_string()));
        
        let mut params2 = HashMap::new();
        params2.insert("name".to_string(), Value::String("Jane".to_string()));
        
        // Cache with different parameters
        cache.cache_plan(query, "plan1", 1.0, None, Some(&params1)).await.unwrap();
        cache.cache_plan(query, "plan2", 1.0, None, Some(&params2)).await.unwrap();
        
        // Should find different cached plans for different parameters
        let plan1 = cache.get_plan(query, Some(&params1)).await;
        let plan2 = cache.get_plan(query, Some(&params2)).await;
        
        assert!(plan1.is_some());
        assert!(plan2.is_some());
        assert_eq!(plan1.unwrap().plan_hint, "plan1");
        assert_eq!(plan2.unwrap().plan_hint, "plan2");
    }
}
use anyhow::{Result, anyhow};
use redis::{Client, Connection, Commands, AsyncCommands, aio::ConnectionManager};
use serde::{Serialize, Deserialize};
use std::time::Duration;
use tracing::{info, debug, warn};
use uuid::Uuid;

const CACHE_TTL_SECONDS: u64 = 3600; // 1 hour default TTL
const QUERY_CACHE_PREFIX: &str = "query:";
const SESSION_PREFIX: &str = "session:";
const METRICS_PREFIX: &str = "metrics:";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheStats {
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
    pub total_queries: u64,
}

pub struct CacheManager {
    connection: ConnectionManager,
}

impl CacheManager {
    pub async fn new() -> Result<Self> {
        info!("Initializing Cache Manager...");
        
        let client = Client::open("redis://127.0.0.1:6379")
            .map_err(|e| anyhow!("Failed to create Redis client: {}", e))?;
        
        let connection = ConnectionManager::new(client).await
            .map_err(|e| anyhow!("Failed to establish Redis connection: {}", e))?;
        
        info!("Cache Manager initialized");
        
        Ok(Self { connection })
    }
    
    pub async fn cache_query_result(
        &mut self,
        query: &str,
        result: &serde_json::Value,
        ttl: Option<Duration>,
    ) -> Result<()> {
        let key = format!("{}{}", QUERY_CACHE_PREFIX, self.hash_query(query));
        let ttl_seconds = ttl.unwrap_or(Duration::from_secs(CACHE_TTL_SECONDS)).as_secs() as usize;
        
        let serialized = serde_json::to_string(result)?;
        
        self.connection
            .set_ex(&key, serialized, ttl_seconds)
            .await
            .map_err(|e| anyhow!("Failed to cache query result: {}", e))?;
        
        // Update metrics
        self.increment_metric("total_cached_queries").await?;
        
        debug!("Cached query result with key: {}", key);
        Ok(())
    }
    
    pub async fn get_cached_query_result(&mut self, query: &str) -> Result<Option<serde_json::Value>> {
        let key = format!("{}{}", QUERY_CACHE_PREFIX, self.hash_query(query));
        
        let result: Option<String> = self.connection
            .get(&key)
            .await
            .map_err(|e| anyhow!("Failed to get cached result: {}", e))?;
        
        match result {
            Some(json_str) => {
                self.increment_metric("cache_hits").await?;
                let value = serde_json::from_str(&json_str)?;
                debug!("Cache hit for query: {}", query);
                Ok(Some(value))
            }
            None => {
                self.increment_metric("cache_misses").await?;
                debug!("Cache miss for query: {}", query);
                Ok(None)
            }
        }
    }
    
    pub async fn invalidate_cache_pattern(&mut self, pattern: &str) -> Result<u64> {
        let keys: Vec<String> = self.connection
            .keys(format!("{}*{}", QUERY_CACHE_PREFIX, pattern))
            .await
            .map_err(|e| anyhow!("Failed to find keys: {}", e))?;
        
        if keys.is_empty() {
            return Ok(0);
        }
        
        let count = keys.len() as u64;
        
        for key in keys {
            self.connection
                .del(&key)
                .await
                .map_err(|e| anyhow!("Failed to delete key: {}", e))?;
        }
        
        self.increment_metric_by("cache_evictions", count).await?;
        
        info!("Invalidated {} cache entries matching pattern: {}", count, pattern);
        Ok(count)
    }
    
    pub async fn store_session_data(
        &mut self,
        session_id: &str,
        data: &serde_json::Value,
        ttl: Option<Duration>,
    ) -> Result<()> {
        let key = format!("{}{}", SESSION_PREFIX, session_id);
        let ttl_seconds = ttl.unwrap_or(Duration::from_secs(86400)).as_secs() as usize; // 24 hours default
        
        let serialized = serde_json::to_string(data)?;
        
        self.connection
            .set_ex(&key, serialized, ttl_seconds)
            .await
            .map_err(|e| anyhow!("Failed to store session data: {}", e))?;
        
        debug!("Stored session data for: {}", session_id);
        Ok(())
    }
    
    pub async fn get_session_data(&mut self, session_id: &str) -> Result<Option<serde_json::Value>> {
        let key = format!("{}{}", SESSION_PREFIX, session_id);
        
        let result: Option<String> = self.connection
            .get(&key)
            .await
            .map_err(|e| anyhow!("Failed to get session data: {}", e))?;
        
        match result {
            Some(json_str) => {
                let value = serde_json::from_str(&json_str)?;
                Ok(Some(value))
            }
            None => Ok(None)
        }
    }
    
    pub async fn get_cache_stats(&mut self) -> Result<CacheStats> {
        let hits = self.get_metric("cache_hits").await?;
        let misses = self.get_metric("cache_misses").await?;
        let evictions = self.get_metric("cache_evictions").await?;
        let total_queries = self.get_metric("total_cached_queries").await?;
        
        Ok(CacheStats {
            hits,
            misses,
            evictions,
            total_queries,
        })
    }
    
    pub async fn warm_cache(&mut self, common_queries: Vec<(&str, serde_json::Value)>) -> Result<()> {
        info!("Warming cache with {} common queries", common_queries.len());
        
        for (query, result) in common_queries {
            self.cache_query_result(query, &result, Some(Duration::from_secs(7200))).await?;
        }
        
        info!("Cache warming complete");
        Ok(())
    }
    
    async fn increment_metric(&mut self, metric: &str) -> Result<()> {
        let key = format!("{}{}", METRICS_PREFIX, metric);
        self.connection
            .incr(&key, 1)
            .await
            .map_err(|e| anyhow!("Failed to increment metric: {}", e))?;
        Ok(())
    }
    
    async fn increment_metric_by(&mut self, metric: &str, value: u64) -> Result<()> {
        let key = format!("{}{}", METRICS_PREFIX, metric);
        self.connection
            .incr(&key, value)
            .await
            .map_err(|e| anyhow!("Failed to increment metric: {}", e))?;
        Ok(())
    }
    
    async fn get_metric(&mut self, metric: &str) -> Result<u64> {
        let key = format!("{}{}", METRICS_PREFIX, metric);
        let value: Option<u64> = self.connection
            .get(&key)
            .await
            .map_err(|e| anyhow!("Failed to get metric: {}", e))?;
        Ok(value.unwrap_or(0))
    }
    
    fn hash_query(&self, query: &str) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        
        let mut hasher = DefaultHasher::new();
        query.hash(&mut hasher);
        format!("{:x}", hasher.finish())
    }
    
    pub async fn clear_all_cache(&mut self) -> Result<()> {
        warn!("Clearing all cache entries!");
        
        let patterns = vec![
            format!("{}*", QUERY_CACHE_PREFIX),
            format!("{}*", SESSION_PREFIX),
        ];
        
        for pattern in patterns {
            let keys: Vec<String> = self.connection
                .keys(&pattern)
                .await
                .map_err(|e| anyhow!("Failed to find keys: {}", e))?;
            
            for key in keys {
                self.connection
                    .del(&key)
                    .await
                    .map_err(|e| anyhow!("Failed to delete key: {}", e))?;
            }
        }
        
        info!("All cache entries cleared");
        Ok(())
    }
}
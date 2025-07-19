use std::time::{Duration, Instant};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use serde::{Serialize, Deserialize};
use tracing::{info, debug, warn};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryMetrics {
    pub query: String,
    pub execution_time_ms: u64,
    pub result_count: usize,
    pub cache_hit: bool,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceStats {
    pub total_queries: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub average_execution_time_ms: f64,
    pub p95_execution_time_ms: u64,
    pub p99_execution_time_ms: u64,
    pub slowest_queries: Vec<QueryMetrics>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemMetrics {
    pub memory_usage_mb: f64,
    pub graph_nodes: u64,
    pub graph_relationships: u64,
    pub cache_size_mb: f64,
    pub uptime_seconds: u64,
}

pub struct MetricsCollector {
    query_metrics: Arc<RwLock<Vec<QueryMetrics>>>,
    start_time: Instant,
    query_times: Arc<RwLock<Vec<u64>>>,
}

impl MetricsCollector {
    pub fn new() -> Self {
        Self {
            query_metrics: Arc::new(RwLock::new(Vec::new())),
            start_time: Instant::now(),
            query_times: Arc::new(RwLock::new(Vec::new())),
        }
    }
    
    pub async fn record_query(
        &self,
        query: String,
        execution_time: Duration,
        result_count: usize,
        cache_hit: bool,
    ) {
        let metric = QueryMetrics {
            query: query.clone(),
            execution_time_ms: execution_time.as_millis() as u64,
            result_count,
            cache_hit,
            timestamp: chrono::Utc::now(),
        };
        
        let mut metrics = self.query_metrics.write().await;
        metrics.push(metric.clone());
        
        // Keep only the last 1000 queries
        if metrics.len() > 1000 {
            metrics.drain(0..100);
        }
        
        // Record execution time for percentile calculations
        let mut times = self.query_times.write().await;
        times.push(execution_time.as_millis() as u64);
        if times.len() > 1000 {
            times.drain(0..100);
        }
        
        debug!(
            "Query executed: {} ({}ms, {} results, cache: {})",
            if query.len() > 50 { &query[..50] } else { &query },
            execution_time.as_millis(),
            result_count,
            cache_hit
        );
    }
    
    pub async fn get_performance_stats(&self) -> PerformanceStats {
        let metrics = self.query_metrics.read().await;
        let times = self.query_times.read().await;
        
        let total_queries = metrics.len() as u64;
        let cache_hits = metrics.iter().filter(|m| m.cache_hit).count() as u64;
        let cache_misses = total_queries - cache_hits;
        
        let average_execution_time_ms = if !times.is_empty() {
            times.iter().sum::<u64>() as f64 / times.len() as f64
        } else {
            0.0
        };
        
        // Calculate percentiles
        let mut sorted_times = times.clone();
        sorted_times.sort();
        
        let p95_execution_time_ms = if !sorted_times.is_empty() {
            let idx = ((sorted_times.len() as f64 * 0.95) as usize).min(sorted_times.len() - 1);
            sorted_times[idx]
        } else {
            0
        };
        
        let p99_execution_time_ms = if !sorted_times.is_empty() {
            let idx = ((sorted_times.len() as f64 * 0.99) as usize).min(sorted_times.len() - 1);
            sorted_times[idx]
        } else {
            0
        };
        
        // Get slowest queries
        let mut slowest_queries: Vec<QueryMetrics> = metrics.iter()
            .cloned()
            .collect();
        slowest_queries.sort_by(|a, b| b.execution_time_ms.cmp(&a.execution_time_ms));
        slowest_queries.truncate(10);
        
        PerformanceStats {
            total_queries,
            cache_hits,
            cache_misses,
            average_execution_time_ms,
            p95_execution_time_ms,
            p99_execution_time_ms,
            slowest_queries,
        }
    }
    
    pub async fn get_system_metrics(
        &self,
        graph_nodes: u64,
        graph_relationships: u64,
        cache_size_mb: f64,
    ) -> SystemMetrics {
        // Get memory usage (simplified - in production use more accurate methods)
        let memory_usage_mb = self.estimate_memory_usage();
        
        let uptime_seconds = self.start_time.elapsed().as_secs();
        
        SystemMetrics {
            memory_usage_mb,
            graph_nodes,
            graph_relationships,
            cache_size_mb,
            uptime_seconds,
        }
    }
    
    fn estimate_memory_usage(&self) -> f64 {
        // This is a placeholder - in production, use actual memory measurement
        // For now, return a rough estimate
        100.0 // MB
    }
    
    pub async fn log_performance_summary(&self) {
        let stats = self.get_performance_stats().await;
        
        info!(
            "Performance Summary - Queries: {}, Cache Hit Rate: {:.1}%, Avg Time: {:.1}ms, P95: {}ms, P99: {}ms",
            stats.total_queries,
            if stats.total_queries > 0 {
                (stats.cache_hits as f64 / stats.total_queries as f64) * 100.0
            } else {
                0.0
            },
            stats.average_execution_time_ms,
            stats.p95_execution_time_ms,
            stats.p99_execution_time_ms
        );
        
        if !stats.slowest_queries.is_empty() {
            info!("Slowest queries:");
            for (i, query) in stats.slowest_queries.iter().take(3).enumerate() {
                info!(
                    "  {}. {} ({}ms)",
                    i + 1,
                    if query.query.len() > 60 { 
                        format!("{}...", &query.query[..60]) 
                    } else { 
                        query.query.clone() 
                    },
                    query.execution_time_ms
                );
            }
        }
    }
}

pub struct QueryTimer {
    start: Instant,
    collector: Arc<MetricsCollector>,
    query: String,
    cache_hit: bool,
}

impl QueryTimer {
    pub fn new(collector: Arc<MetricsCollector>, query: String) -> Self {
        Self {
            start: Instant::now(),
            collector,
            query,
            cache_hit: false,
        }
    }
    
    pub fn set_cache_hit(&mut self, hit: bool) {
        self.cache_hit = hit;
    }
    
    pub async fn finish(self, result_count: usize) {
        let duration = self.start.elapsed();
        self.collector.record_query(
            self.query,
            duration,
            result_count,
            self.cache_hit
        ).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::sleep;
    
    #[tokio::test]
    async fn test_metrics_collection() {
        let collector = MetricsCollector::new();
        
        // Record some queries
        collector.record_query(
            "MATCH (n) RETURN n".to_string(),
            Duration::from_millis(50),
            10,
            false
        ).await;
        
        collector.record_query(
            "MATCH (n:Function) RETURN n".to_string(),
            Duration::from_millis(25),
            5,
            true
        ).await;
        
        let stats = collector.get_performance_stats().await;
        assert_eq!(stats.total_queries, 2);
        assert_eq!(stats.cache_hits, 1);
        assert_eq!(stats.cache_misses, 1);
        assert!(stats.average_execution_time_ms > 0.0);
    }
    
    #[tokio::test]
    async fn test_query_timer() {
        let collector = Arc::new(MetricsCollector::new());
        let mut timer = QueryTimer::new(collector.clone(), "Test query".to_string());
        
        // Simulate some work
        sleep(Duration::from_millis(10)).await;
        
        timer.set_cache_hit(true);
        timer.finish(5).await;
        
        let stats = collector.get_performance_stats().await;
        assert_eq!(stats.total_queries, 1);
        assert_eq!(stats.cache_hits, 1);
        assert!(stats.average_execution_time_ms >= 10.0);
    }
}
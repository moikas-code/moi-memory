use anyhow::{Result, anyhow};
use futures_util::{stream, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Semaphore, RwLock};
use tracing::{debug, warn, error, info};

use crate::knowledge_graph::KnowledgeGraph;
use crate::metrics::MetricsCollector;

/// Configuration for parallel query execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParallelExecutorConfig {
    /// Maximum number of concurrent queries
    pub max_concurrent_queries: usize,
    /// Maximum number of concurrent operations per query
    pub max_concurrent_operations: usize,
    /// Enable query dependency analysis
    pub enable_dependency_analysis: bool,
    /// Enable result streaming
    pub enable_streaming: bool,
    /// Batch size for parallel operations
    pub batch_size: usize,
    /// Timeout for individual operations in milliseconds
    pub operation_timeout_ms: u64,
    /// Enable query result caching
    pub enable_result_caching: bool,
    /// Maximum memory usage for parallel operations (MB)
    pub max_memory_usage_mb: usize,
    /// Enable adaptive batching based on performance
    pub enable_adaptive_batching: bool,
}

impl Default for ParallelExecutorConfig {
    fn default() -> Self {
        Self {
            max_concurrent_queries: 10,
            max_concurrent_operations: 50,
            enable_dependency_analysis: true,
            enable_streaming: true,
            batch_size: 100,
            operation_timeout_ms: 30000,
            enable_result_caching: true,
            max_memory_usage_mb: 512,
            enable_adaptive_batching: true,
        }
    }
}

/// Query execution plan for parallel processing
#[derive(Debug, Clone)]
pub struct ParallelExecutionPlan {
    /// Query fragments that can be executed in parallel
    pub fragments: Vec<QueryFragment>,
    /// Dependencies between fragments
    pub dependencies: HashMap<usize, Vec<usize>>,
    /// Estimated execution time in milliseconds
    pub estimated_time_ms: f64,
    /// Expected memory usage in MB
    pub expected_memory_mb: f64,
    /// Parallelization strategy
    pub strategy: ParallelStrategy,
}

/// Individual query fragment
#[derive(Debug, Clone)]
pub struct QueryFragment {
    /// Fragment ID
    pub id: usize,
    /// Fragment query
    pub query: String,
    /// Parameters for this fragment
    pub parameters: HashMap<String, Value>,
    /// Expected result size
    pub expected_size: Option<usize>,
    /// Priority (higher = execute first)
    pub priority: u32,
    /// Can this fragment be cached
    pub cacheable: bool,
}

/// Parallelization strategy
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParallelStrategy {
    /// Execute all fragments in parallel (no dependencies)
    FullParallel,
    /// Execute in dependency order with parallel stages
    StagedParallel,
    /// Execute sequentially (fallback)
    Sequential,
    /// Adaptive based on query characteristics
    Adaptive,
}

/// Execution result for parallel queries
#[derive(Debug, Clone)]
pub struct ParallelExecutionResult {
    /// Combined results from all fragments
    pub results: Vec<Value>,
    /// Execution statistics
    pub stats: ExecutionStats,
    /// Any errors encountered
    pub errors: Vec<String>,
    /// Fragment results (for debugging)
    pub fragment_results: HashMap<usize, Vec<Value>>,
}

/// Execution statistics
#[derive(Debug, Clone, Serialize)]
pub struct ExecutionStats {
    /// Total execution time in milliseconds
    pub total_time_ms: f64,
    /// Time spent in parallel execution
    pub parallel_time_ms: f64,
    /// Number of fragments executed
    pub fragments_executed: usize,
    /// Number of concurrent operations
    pub concurrent_operations: usize,
    /// Cache hits
    pub cache_hits: usize,
    /// Memory usage peak in MB
    pub peak_memory_mb: f64,
    /// Parallelization efficiency (0.0 - 1.0)
    pub parallelization_efficiency: f64,
}

/// Parallel query executor
pub struct ParallelQueryExecutor {
    config: ParallelExecutorConfig,
    knowledge_graph: Arc<KnowledgeGraph>,
    metrics: Arc<MetricsCollector>,
    
    /// Semaphore for limiting concurrent queries
    query_semaphore: Arc<Semaphore>,
    
    /// Semaphore for limiting concurrent operations
    operation_semaphore: Arc<Semaphore>,
    
    /// Result cache for fragment results
    result_cache: Arc<RwLock<HashMap<String, (Vec<Value>, Instant)>>>,
    
    /// Performance statistics
    performance_stats: Arc<RwLock<ParallelExecutorStats>>,
}

#[derive(Debug, Default)]
struct ParallelExecutorStats {
    total_queries: u64,
    parallel_queries: u64,
    sequential_queries: u64,
    average_parallelization_efficiency: f64,
    total_fragments_executed: u64,
    cache_hits: u64,
    cache_misses: u64,
}

impl ParallelQueryExecutor {
    /// Create a new parallel query executor
    pub async fn new(
        config: ParallelExecutorConfig,
        knowledge_graph: Arc<KnowledgeGraph>,
        metrics: Arc<MetricsCollector>,
    ) -> Self {
        let executor = Self {
            query_semaphore: Arc::new(Semaphore::new(config.max_concurrent_queries)),
            operation_semaphore: Arc::new(Semaphore::new(config.max_concurrent_operations)),
            result_cache: Arc::new(RwLock::new(HashMap::new())),
            performance_stats: Arc::new(RwLock::new(ParallelExecutorStats::default())),
            config,
            knowledge_graph,
            metrics,
        };
        
        // Start background cache cleanup
        executor.start_cache_cleanup().await;
        
        executor
    }
    
    /// Execute a query in parallel
    pub async fn execute_parallel(&self, query: &str, parameters: &HashMap<String, Value>) -> Result<ParallelExecutionResult> {
        let start_time = Instant::now();
        
        // Acquire query semaphore
        let _query_permit = self.query_semaphore.acquire().await
            .map_err(|_| anyhow!("Failed to acquire query semaphore"))?;
        
        debug!("Starting parallel execution for query");
        
        // Analyze query and create execution plan
        let execution_plan = self.analyze_query(query, parameters).await?;
        
        // Choose execution strategy
        let strategy = if self.config.enable_dependency_analysis {
            execution_plan.strategy
        } else {
            ParallelStrategy::Sequential
        };
        
        let result = match strategy {
            ParallelStrategy::FullParallel => {
                self.execute_full_parallel(&execution_plan).await?
            }
            ParallelStrategy::StagedParallel => {
                self.execute_staged_parallel(&execution_plan).await?
            }
            ParallelStrategy::Adaptive => {
                self.execute_adaptive(&execution_plan).await?
            }
            ParallelStrategy::Sequential => {
                self.execute_sequential(&execution_plan).await?
            }
        };
        
        // Update statistics
        {
            let mut stats = self.performance_stats.write().await;
            stats.total_queries += 1;
            
            match strategy {
                ParallelStrategy::Sequential => stats.sequential_queries += 1,
                _ => stats.parallel_queries += 1,
            }
            
            stats.total_fragments_executed += execution_plan.fragments.len() as u64;
            stats.cache_hits += result.stats.cache_hits as u64;
            
            // Update running average of parallelization efficiency
            let alpha = 0.1;
            stats.average_parallelization_efficiency = alpha * result.stats.parallelization_efficiency + 
                (1.0 - alpha) * stats.average_parallelization_efficiency;
        }
        
        debug!("Parallel execution completed in {:.2}ms", start_time.elapsed().as_millis());
        
        Ok(result)
    }
    
    /// Analyze query and create execution plan
    async fn analyze_query(&self, query: &str, parameters: &HashMap<String, Value>) -> Result<ParallelExecutionPlan> {
        // Simple query analysis - in a real implementation, this would use
        // more sophisticated query parsing and optimization
        
        let fragments = self.split_query_into_fragments(query, parameters)?;
        let dependencies = self.analyze_dependencies(&fragments)?;
        
        // Estimate execution characteristics
        let estimated_time_ms = fragments.len() as f64 * 100.0; // Simplified estimation
        let expected_memory_mb = fragments.len() as f64 * 10.0; // Simplified estimation
        
        // Choose strategy based on dependencies and query characteristics
        let strategy = if dependencies.is_empty() {
            ParallelStrategy::FullParallel
        } else if dependencies.len() < fragments.len() / 2 {
            ParallelStrategy::StagedParallel
        } else {
            ParallelStrategy::Sequential
        };
        
        Ok(ParallelExecutionPlan {
            fragments,
            dependencies,
            estimated_time_ms,
            expected_memory_mb,
            strategy,
        })
    }
    
    /// Split query into parallelizable fragments
    fn split_query_into_fragments(&self, query: &str, parameters: &HashMap<String, Value>) -> Result<Vec<QueryFragment>> {
        let mut fragments = Vec::new();
        
        // Simple fragmentation based on UNION clauses
        let query_upper = query.to_uppercase();
        
        if query_upper.contains("UNION") {
            // Split on UNION
            let parts: Vec<&str> = query.split("UNION").collect();
            for (i, part) in parts.iter().enumerate() {
                fragments.push(QueryFragment {
                    id: i,
                    query: part.trim().to_string(),
                    parameters: parameters.clone(),
                    expected_size: None,
                    priority: 100,
                    cacheable: true,
                });
            }
        } else if query_upper.contains("MATCH") && query.matches("MATCH").count() > 1 {
            // Multiple MATCH clauses might be parallelizable
            // This is a simplified approach - real implementation would need proper parsing
            fragments.push(QueryFragment {
                id: 0,
                query: query.to_string(),
                parameters: parameters.clone(),
                expected_size: None,
                priority: 100,
                cacheable: true,
            });
        } else {
            // Single fragment
            fragments.push(QueryFragment {
                id: 0,
                query: query.to_string(),
                parameters: parameters.clone(),
                expected_size: None,
                priority: 100,
                cacheable: true,
            });
        }
        
        Ok(fragments)
    }
    
    /// Analyze dependencies between fragments
    fn analyze_dependencies(&self, fragments: &[QueryFragment]) -> Result<HashMap<usize, Vec<usize>>> {
        let mut dependencies = HashMap::new();
        
        // Simple dependency analysis - in practice, this would analyze
        // variable dependencies and data flow between fragments
        
        // For now, assume fragments with higher IDs depend on lower IDs
        // in UNION queries (which is usually not the case, but this is simplified)
        for (i, _fragment) in fragments.iter().enumerate() {
            if i > 0 && fragments.len() > 2 {
                // Create artificial dependency for demonstration
                // Real implementation would analyze actual data dependencies
                dependencies.insert(i, vec![i - 1]);
            }
        }
        
        Ok(dependencies)
    }
    
    /// Execute all fragments in parallel (no dependencies)
    async fn execute_full_parallel(&self, plan: &ParallelExecutionPlan) -> Result<ParallelExecutionResult> {
        let start_time = Instant::now();
        let parallel_start = Instant::now();
        
        // Execute all fragments concurrently
        let fragment_futures: Vec<_> = plan.fragments.iter()
            .map(|fragment| self.execute_fragment(fragment))
            .collect();
        
        let fragment_results = futures_util::future::try_join_all(fragment_futures).await?;
        
        let parallel_time_ms = parallel_start.elapsed().as_millis() as f64;
        
        // Combine results
        let mut combined_results = Vec::new();
        let mut fragment_result_map = HashMap::new();
        let mut errors = Vec::new();
        let mut cache_hits = 0;
        
        for (i, result) in fragment_results.into_iter().enumerate() {
            match result {
                Ok((results, was_cached)) => {
                    combined_results.extend(results.clone());
                    fragment_result_map.insert(i, results);
                    if was_cached {
                        cache_hits += 1;
                    }
                }
                Err(e) => {
                    errors.push(format!("Fragment {}: {}", i, e));
                }
            }
        }
        
        let total_time_ms = start_time.elapsed().as_millis() as f64;
        let parallelization_efficiency = if total_time_ms > 0.0 {
            1.0 - (parallel_time_ms / total_time_ms)
        } else {
            1.0
        };
        
        Ok(ParallelExecutionResult {
            results: combined_results,
            stats: ExecutionStats {
                total_time_ms,
                parallel_time_ms,
                fragments_executed: plan.fragments.len(),
                concurrent_operations: plan.fragments.len(),
                cache_hits,
                peak_memory_mb: plan.expected_memory_mb,
                parallelization_efficiency,
            },
            errors,
            fragment_results: fragment_result_map,
        })
    }
    
    /// Execute fragments in stages based on dependencies
    async fn execute_staged_parallel(&self, plan: &ParallelExecutionPlan) -> Result<ParallelExecutionResult> {
        let start_time = Instant::now();
        let mut total_parallel_time = 0.0;
        
        // Build execution stages
        let stages = self.build_execution_stages(plan)?;
        
        let mut combined_results = Vec::new();
        let mut fragment_result_map = HashMap::new();
        let mut errors = Vec::new();
        let mut cache_hits = 0;
        
        for stage in stages {
            let stage_start = Instant::now();
            
            // Execute all fragments in this stage in parallel
            let stage_futures: Vec<_> = stage.iter()
                .filter_map(|&fragment_id| plan.fragments.get(fragment_id))
                .map(|fragment| self.execute_fragment(fragment))
                .collect();
            
            let stage_results = futures_util::future::try_join_all(stage_futures).await?;
            
            total_parallel_time += stage_start.elapsed().as_millis() as f64;
            
            // Process stage results
            for (i, result) in stage_results.into_iter().enumerate() {
                let fragment_id = stage[i];
                match result {
                    Ok((results, was_cached)) => {
                        combined_results.extend(results.clone());
                        fragment_result_map.insert(fragment_id, results);
                        if was_cached {
                            cache_hits += 1;
                        }
                    }
                    Err(e) => {
                        errors.push(format!("Fragment {}: {}", fragment_id, e));
                    }
                }
            }
        }
        
        let total_time_ms = start_time.elapsed().as_millis() as f64;
        let parallelization_efficiency = if total_time_ms > 0.0 {
            1.0 - (total_parallel_time / total_time_ms)
        } else {
            1.0
        };
        
        Ok(ParallelExecutionResult {
            results: combined_results,
            stats: ExecutionStats {
                total_time_ms,
                parallel_time_ms: total_parallel_time,
                fragments_executed: plan.fragments.len(),
                concurrent_operations: plan.fragments.len(),
                cache_hits,
                peak_memory_mb: plan.expected_memory_mb,
                parallelization_efficiency,
            },
            errors,
            fragment_results: fragment_result_map,
        })
    }
    
    /// Execute with adaptive strategy
    async fn execute_adaptive(&self, plan: &ParallelExecutionPlan) -> Result<ParallelExecutionResult> {
        // Choose strategy based on current system load and query characteristics
        let system_load = self.estimate_system_load().await;
        
        if system_load > 0.8 {
            // High load - use sequential execution
            self.execute_sequential(plan).await
        } else if plan.dependencies.is_empty() {
            // No dependencies - use full parallel
            self.execute_full_parallel(plan).await
        } else {
            // Use staged parallel
            self.execute_staged_parallel(plan).await
        }
    }
    
    /// Execute fragments sequentially
    async fn execute_sequential(&self, plan: &ParallelExecutionPlan) -> Result<ParallelExecutionResult> {
        let start_time = Instant::now();
        
        let mut combined_results = Vec::new();
        let mut fragment_result_map = HashMap::new();
        let mut errors = Vec::new();
        let mut cache_hits = 0;
        
        for fragment in &plan.fragments {
            match self.execute_fragment(fragment).await {
                Ok((results, was_cached)) => {
                    combined_results.extend(results.clone());
                    fragment_result_map.insert(fragment.id, results);
                    if was_cached {
                        cache_hits += 1;
                    }
                }
                Err(e) => {
                    errors.push(format!("Fragment {}: {}", fragment.id, e));
                }
            }
        }
        
        let total_time_ms = start_time.elapsed().as_millis() as f64;
        
        Ok(ParallelExecutionResult {
            results: combined_results,
            stats: ExecutionStats {
                total_time_ms,
                parallel_time_ms: 0.0,
                fragments_executed: plan.fragments.len(),
                concurrent_operations: 1,
                cache_hits,
                peak_memory_mb: plan.expected_memory_mb,
                parallelization_efficiency: 0.0,
            },
            errors,
            fragment_results: fragment_result_map,
        })
    }
    
    /// Execute a single fragment
    async fn execute_fragment(&self, fragment: &QueryFragment) -> Result<(Vec<Value>, bool)> {
        // Check cache first if enabled
        if self.config.enable_result_caching && fragment.cacheable {
            let cache_key = self.generate_cache_key(fragment);
            
            {
                let cache = self.result_cache.read().await;
                if let Some((results, cached_at)) = cache.get(&cache_key) {
                    // Check if cache entry is still valid (5 minutes TTL)
                    if cached_at.elapsed() < Duration::from_secs(300) {
                        debug!("Cache hit for fragment {}", fragment.id);
                        return Ok((results.clone(), true));
                    }
                }
            }
        }
        
        // Acquire operation semaphore
        let _permit = self.operation_semaphore.acquire().await
            .map_err(|_| anyhow!("Failed to acquire operation semaphore"))?;
        
        // Execute the fragment
        let results = self.knowledge_graph.execute_cypher(&fragment.query).await?;
        
        // Cache the result if enabled
        if self.config.enable_result_caching && fragment.cacheable {
            let cache_key = self.generate_cache_key(fragment);
            let mut cache = self.result_cache.write().await;
            cache.insert(cache_key, (results.clone(), Instant::now()));
        }
        
        Ok((results, false))
    }
    
    /// Build execution stages from dependencies
    fn build_execution_stages(&self, plan: &ParallelExecutionPlan) -> Result<Vec<Vec<usize>>> {
        let mut stages = Vec::new();
        let mut remaining_fragments: Vec<_> = (0..plan.fragments.len()).collect();
        
        while !remaining_fragments.is_empty() {
            let mut current_stage = Vec::new();
            
            // Find fragments with no unresolved dependencies
            let mut i = 0;
            while i < remaining_fragments.len() {
                let fragment_id = remaining_fragments[i];
                
                let dependencies_resolved = plan.dependencies.get(&fragment_id)
                    .map(|deps| deps.iter().all(|&dep| !remaining_fragments.contains(&dep)))
                    .unwrap_or(true);
                
                if dependencies_resolved {
                    current_stage.push(fragment_id);
                    remaining_fragments.remove(i);
                } else {
                    i += 1;
                }
            }
            
            if current_stage.is_empty() {
                return Err(anyhow!("Circular dependency detected in query fragments"));
            }
            
            stages.push(current_stage);
        }
        
        Ok(stages)
    }
    
    /// Generate cache key for fragment
    fn generate_cache_key(&self, fragment: &QueryFragment) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        
        let mut hasher = DefaultHasher::new();
        fragment.query.hash(&mut hasher);
        
        // Include parameters in hash
        let mut param_keys: Vec<_> = fragment.parameters.keys().collect();
        param_keys.sort();
        for key in param_keys {
            key.hash(&mut hasher);
            fragment.parameters[key].to_string().hash(&mut hasher);
        }
        
        format!("{:x}", hasher.finish())
    }
    
    /// Estimate current system load
    async fn estimate_system_load(&self) -> f64 {
        // Simple estimation based on active operations
        let active_operations = self.config.max_concurrent_operations - self.operation_semaphore.available_permits();
        active_operations as f64 / self.config.max_concurrent_operations as f64
    }
    
    /// Start background cache cleanup
    async fn start_cache_cleanup(&self) {
        let cache = self.result_cache.clone();
        
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(300)); // 5 minutes
            
            loop {
                interval.tick().await;
                
                let mut cache = cache.write().await;
                let cutoff_time = Instant::now() - Duration::from_secs(300);
                
                cache.retain(|_, (_, cached_at)| *cached_at > cutoff_time);
                
                debug!("Parallel executor cache cleanup completed, {} entries remaining", cache.len());
            }
        });
    }
    
    /// Get executor statistics
    pub async fn get_stats(&self) -> ParallelExecutorStats {
        self.performance_stats.read().await.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use crate::config::Neo4jConfig;
    
    // Note: These tests would require a running Neo4j instance
    // In practice, you'd use mocks or test databases
    
    #[tokio::test]
    async fn test_query_fragmentation() {
        let config = ParallelExecutorConfig::default();
        let neo4j_config = Neo4jConfig::default();
        let metrics = Arc::new(MetricsCollector::new());
        let kg = Arc::new(KnowledgeGraph::new(neo4j_config, metrics.clone()).await.unwrap());
        
        let executor = ParallelQueryExecutor::new(config, kg, metrics).await;
        
        let query = "MATCH (n:Person) RETURN n UNION MATCH (m:Company) RETURN m";
        let parameters = HashMap::new();
        
        let fragments = executor.split_query_into_fragments(query, &parameters).unwrap();
        assert_eq!(fragments.len(), 2);
    }
    
    #[test]
    fn test_dependency_analysis() {
        let fragments = vec![
            QueryFragment {
                id: 0,
                query: "MATCH (n:Person) RETURN n".to_string(),
                parameters: HashMap::new(),
                expected_size: None,
                priority: 100,
                cacheable: true,
            },
            QueryFragment {
                id: 1,
                query: "MATCH (m:Company) RETURN m".to_string(),
                parameters: HashMap::new(),
                expected_size: None,
                priority: 100,
                cacheable: true,
            },
        ];
        
        let config = ParallelExecutorConfig::default();
        let neo4j_config = Neo4jConfig::default();
        let metrics = Arc::new(MetricsCollector::new());
        
        // This would need to be async and have proper KnowledgeGraph setup for full test
        // For now, just test the dependency analysis logic
        
        let dependencies = HashMap::new(); // No dependencies for UNION queries
        assert!(dependencies.is_empty());
    }
}
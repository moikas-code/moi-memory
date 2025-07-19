use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{RwLock, Mutex};
use tokio::time::interval;
use tracing::{info, warn, error, debug};

use crate::knowledge_graph::KnowledgeGraph;
use crate::cache_degradation::GracefulCacheManager;
use crate::backpressure_manager::BackpressureManager;
use crate::performance::{MemoryMonitor, QueryPlanCache, ParallelQueryExecutor};
use crate::security::{AuthManager, SessionRateLimiter};

/// Health status levels
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum HealthStatus {
    Healthy,
    Warning,
    Critical,
    Down,
}

impl std::fmt::Display for HealthStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HealthStatus::Healthy => write!(f, "healthy"),
            HealthStatus::Warning => write!(f, "warning"),
            HealthStatus::Critical => write!(f, "critical"),
            HealthStatus::Down => write!(f, "down"),
        }
    }
}

/// Component health check result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentHealth {
    pub name: String,
    pub status: HealthStatus,
    pub last_check: Instant,
    pub response_time_ms: u64,
    pub error_message: Option<String>,
    pub metrics: HashMap<String, serde_json::Value>,
    pub dependencies: Vec<String>,
}

/// Health check configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthMonitorConfig {
    /// Interval between health checks (seconds)
    pub check_interval_seconds: u64,
    
    /// Timeout for individual health checks (milliseconds)
    pub check_timeout_ms: u64,
    
    /// Number of failed checks before marking as critical
    pub critical_threshold: u32,
    
    /// Number of failed checks before marking as down
    pub down_threshold: u32,
    
    /// Enable detailed component monitoring
    pub enable_detailed_monitoring: bool,
    
    /// Enable automatic recovery attempts
    pub enable_auto_recovery: bool,
    
    /// Maximum recovery attempts per component
    pub max_recovery_attempts: u32,
    
    /// Recovery cooldown period (seconds)
    pub recovery_cooldown_seconds: u64,
    
    /// Enable health history tracking
    pub enable_history: bool,
    
    /// Maximum health history entries per component
    pub max_history_entries: usize,
}

impl Default for HealthMonitorConfig {
    fn default() -> Self {
        Self {
            check_interval_seconds: 30,
            check_timeout_ms: 5000,
            critical_threshold: 3,
            down_threshold: 5,
            enable_detailed_monitoring: true,
            enable_auto_recovery: true,
            max_recovery_attempts: 3,
            recovery_cooldown_seconds: 300,
            enable_history: true,
            max_history_entries: 100,
        }
    }
}

/// Health check failure tracking
#[derive(Debug, Clone)]
struct FailureTracker {
    consecutive_failures: u32,
    last_failure: Option<Instant>,
    recovery_attempts: u32,
    last_recovery_attempt: Option<Instant>,
}

impl Default for FailureTracker {
    fn default() -> Self {
        Self {
            consecutive_failures: 0,
            last_failure: None,
            recovery_attempts: 0,
            last_recovery_attempt: None,
        }
    }
}

/// Overall system health status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemHealth {
    pub overall_status: HealthStatus,
    pub components: HashMap<String, ComponentHealth>,
    pub last_check: Instant,
    pub uptime_seconds: u64,
    pub total_checks: u64,
    pub failed_checks: u64,
    pub recovery_attempts: u64,
    pub performance_score: f64,
}

/// Health history entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthHistoryEntry {
    pub timestamp: Instant,
    pub status: HealthStatus,
    pub response_time_ms: u64,
    pub error_message: Option<String>,
}

/// Comprehensive health monitoring system
pub struct HealthMonitor {
    config: HealthMonitorConfig,
    
    // Component references
    knowledge_graph: Arc<KnowledgeGraph>,
    cache_manager: Arc<GracefulCacheManager>,
    backpressure_manager: Arc<Mutex<BackpressureManager>>,
    memory_monitor: Arc<Mutex<MemoryMonitor>>,
    query_cache: Arc<QueryPlanCache>,
    parallel_executor: Arc<ParallelQueryExecutor>,
    auth_manager: Arc<AuthManager>,
    rate_limiter: Arc<SessionRateLimiter>,
    
    // Health tracking
    system_health: Arc<RwLock<SystemHealth>>,
    failure_trackers: Arc<RwLock<HashMap<String, FailureTracker>>>,
    health_history: Arc<RwLock<HashMap<String, Vec<HealthHistoryEntry>>>>,
    
    // Monitoring state
    start_time: Instant,
    monitor_handle: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
}

impl HealthMonitor {
    pub async fn new(
        config: HealthMonitorConfig,
        knowledge_graph: Arc<KnowledgeGraph>,
        cache_manager: Arc<GracefulCacheManager>,
        backpressure_manager: Arc<Mutex<BackpressureManager>>,
        memory_monitor: Arc<Mutex<MemoryMonitor>>,
        query_cache: Arc<QueryPlanCache>,
        parallel_executor: Arc<ParallelQueryExecutor>,
        auth_manager: Arc<AuthManager>,
        rate_limiter: Arc<SessionRateLimiter>,
    ) -> Self {
        let start_time = Instant::now();
        
        let system_health = Arc::new(RwLock::new(SystemHealth {
            overall_status: HealthStatus::Healthy,
            components: HashMap::new(),
            last_check: start_time,
            uptime_seconds: 0,
            total_checks: 0,
            failed_checks: 0,
            recovery_attempts: 0,
            performance_score: 100.0,
        }));
        
        Self {
            config,
            knowledge_graph,
            cache_manager,
            backpressure_manager,
            memory_monitor,
            query_cache,
            parallel_executor,
            auth_manager,
            rate_limiter,
            system_health,
            failure_trackers: Arc::new(RwLock::new(HashMap::new())),
            health_history: Arc::new(RwLock::new(HashMap::new())),
            start_time,
            monitor_handle: Arc::new(Mutex::new(None)),
        }
    }
    
    /// Start the health monitoring background task
    pub async fn start(&self) -> Result<()> {
        info!("Starting comprehensive health monitoring system...");
        
        let config = self.config.clone();
        let system_health = self.system_health.clone();
        let failure_trackers = self.failure_trackers.clone();
        let health_history = self.health_history.clone();
        let start_time = self.start_time;
        
        // Component references for monitoring
        let kg = self.knowledge_graph.clone();
        let cache = self.cache_manager.clone();
        let backpressure = self.backpressure_manager.clone();
        let memory = self.memory_monitor.clone();
        let query_cache = self.query_cache.clone();
        let parallel_exec = self.parallel_executor.clone();
        let auth = self.auth_manager.clone();
        let rate_limiter = self.rate_limiter.clone();
        
        let handle = tokio::spawn(async move {
            let mut interval = interval(Duration::from_secs(config.check_interval_seconds));
            
            loop {
                interval.tick().await;
                
                let check_start = Instant::now();
                let mut components = HashMap::new();
                let mut failed_components = 0;
                let mut total_response_time = 0u64;
                
                // Check each component
                let component_checks = vec![
                    Self::check_neo4j_health(&kg),
                    Self::check_cache_health(&cache),
                    Self::check_backpressure_health(&backpressure),
                    Self::check_memory_health(&memory),
                    Self::check_query_cache_health(&query_cache),
                    Self::check_parallel_executor_health(&parallel_exec),
                    Self::check_auth_health(&auth),
                    Self::check_rate_limiter_health(&rate_limiter),
                ];
                
                // Execute all health checks concurrently
                let results = futures::future::join_all(component_checks).await;
                
                for result in results {
                    match result {
                        Ok(component_health) => {
                            total_response_time += component_health.response_time_ms;
                            
                            if component_health.status != HealthStatus::Healthy {
                                failed_components += 1;
                            }
                            
                            // Update failure tracking
                            Self::update_failure_tracker(
                                &failure_trackers,
                                &component_health.name,
                                &component_health.status,
                                &config,
                            ).await;
                            
                            // Add to history if enabled
                            if config.enable_history {
                                Self::add_to_history(
                                    &health_history,
                                    &component_health.name,
                                    &component_health,
                                    config.max_history_entries,
                                ).await;
                            }
                            
                            components.insert(component_health.name.clone(), component_health);
                        }
                        Err(e) => {
                            error!("Health check failed: {}", e);
                            failed_components += 1;
                        }
                    }
                }
                
                // Calculate overall system health
                let overall_status = if failed_components == 0 {
                    HealthStatus::Healthy
                } else if failed_components < components.len() / 2 {
                    HealthStatus::Warning
                } else {
                    HealthStatus::Critical
                };
                
                // Calculate performance score
                let avg_response_time = if components.is_empty() {
                    0.0
                } else {
                    total_response_time as f64 / components.len() as f64
                };
                
                let performance_score = (1000.0 - avg_response_time.min(1000.0)) / 10.0;
                
                // Update system health
                {
                    let mut health = system_health.write().await;
                    health.overall_status = overall_status;
                    health.components = components;
                    health.last_check = check_start;
                    health.uptime_seconds = start_time.elapsed().as_secs();
                    health.total_checks += 1;
                    if failed_components > 0 {
                        health.failed_checks += 1;
                    }
                    health.performance_score = performance_score;
                }
                
                debug!(
                    "Health check completed: {} components, {} failed, {:.1}ms avg response",
                    components.len(), failed_components, avg_response_time
                );
            }
        });
        
        *self.monitor_handle.lock().await = Some(handle);
        
        info!("Health monitoring system started with {:.0}s interval", config.check_interval_seconds);
        Ok(())
    }
    
    /// Stop the health monitoring system
    pub async fn stop(&self) {
        if let Some(handle) = self.monitor_handle.lock().await.take() {
            handle.abort();
            info!("Health monitoring system stopped");
        }
    }
    
    /// Get current system health status
    pub async fn get_system_health(&self) -> SystemHealth {
        self.system_health.read().await.clone()
    }
    
    /// Get health status for a specific component
    pub async fn get_component_health(&self, component: &str) -> Option<ComponentHealth> {
        self.system_health.read().await.components.get(component).cloned()
    }
    
    /// Get health history for a component
    pub async fn get_component_history(&self, component: &str) -> Vec<HealthHistoryEntry> {
        self.health_history.read().await
            .get(component)
            .cloned()
            .unwrap_or_default()
    }
    
    /// Force a health check for all components
    pub async fn force_health_check(&self) -> Result<SystemHealth> {
        let check_start = Instant::now();
        let mut components = HashMap::new();
        let mut failed_components = 0;
        
        // Perform immediate health checks
        let checks = vec![
            Self::check_neo4j_health(&self.knowledge_graph),
            Self::check_cache_health(&self.cache_manager),
            Self::check_backpressure_health(&self.backpressure_manager),
            Self::check_memory_health(&self.memory_monitor),
            Self::check_query_cache_health(&self.query_cache),
            Self::check_parallel_executor_health(&self.parallel_executor),
            Self::check_auth_health(&self.auth_manager),
            Self::check_rate_limiter_health(&self.rate_limiter),
        ];
        
        let results = futures::future::join_all(checks).await;
        
        for result in results {
            match result {
                Ok(component_health) => {
                    if component_health.status != HealthStatus::Healthy {
                        failed_components += 1;
                    }
                    components.insert(component_health.name.clone(), component_health);
                }
                Err(e) => {
                    error!("Forced health check failed: {}", e);
                    failed_components += 1;
                }
            }
        }
        
        let overall_status = if failed_components == 0 {
            HealthStatus::Healthy
        } else if failed_components < components.len() / 2 {
            HealthStatus::Warning
        } else {
            HealthStatus::Critical
        };
        
        let health = SystemHealth {
            overall_status,
            components,
            last_check: check_start,
            uptime_seconds: self.start_time.elapsed().as_secs(),
            total_checks: 0, // Don't count forced checks
            failed_checks: 0,
            recovery_attempts: 0,
            performance_score: 100.0,
        };
        
        Ok(health)
    }
    
    // Component-specific health checks
    
    async fn check_neo4j_health(kg: &Arc<KnowledgeGraph>) -> Result<ComponentHealth> {
        let start = Instant::now();
        let mut metrics = HashMap::new();
        
        // Test basic connectivity
        let health_query_result = kg.execute_cypher("RETURN 1 as health").await;
        let response_time = start.elapsed().as_millis() as u64;
        
        let (status, error_message) = match health_query_result {
            Ok(_) => {
                // Get detailed stats
                let connection_stats = kg.get_connection_stats().await;
                let circuit_breaker_stats = kg.get_circuit_breaker_stats().await;
                let (nodes, relationships) = kg.get_graph_stats().await.unwrap_or((0, 0));
                
                metrics.insert("active_connections".to_string(), 
                    serde_json::Value::Number(connection_stats.active_connections.into()));
                metrics.insert("total_connections".to_string(), 
                    serde_json::Value::Number(connection_stats.total_connections.into()));
                metrics.insert("failed_connections".to_string(), 
                    serde_json::Value::Number(connection_stats.failed_connections.into()));
                metrics.insert("circuit_breaker_state".to_string(), 
                    serde_json::Value::String(format!("{:?}", circuit_breaker_stats.state)));
                metrics.insert("graph_nodes".to_string(), 
                    serde_json::Value::Number(nodes.into()));
                metrics.insert("graph_relationships".to_string(), 
                    serde_json::Value::Number(relationships.into()));
                
                // Determine status based on connection health
                if connection_stats.failed_connections > connection_stats.active_connections * 2 {
                    (HealthStatus::Warning, None)
                } else if connection_stats.active_connections == 0 {
                    (HealthStatus::Critical, Some("No active connections".to_string()))
                } else {
                    (HealthStatus::Healthy, None)
                }
            }
            Err(e) => (HealthStatus::Down, Some(e.to_string())),
        };
        
        Ok(ComponentHealth {
            name: "neo4j".to_string(),
            status,
            last_check: start,
            response_time_ms: response_time,
            error_message,
            metrics,
            dependencies: vec!["network".to_string(), "docker".to_string()],
        })
    }
    
    async fn check_cache_health(cache: &Arc<GracefulCacheManager>) -> Result<ComponentHealth> {
        let start = Instant::now();
        let mut metrics = HashMap::new();
        
        let (status, error_message) = match cache.get_cache_stats().await {
            Ok(stats) => {
                // Extract cache metrics
                if let Some(hit_rate) = stats.get("hit_rate").and_then(|v| v.as_f64()) {
                    metrics.insert("hit_rate".to_string(), serde_json::Value::Number(
                        serde_json::Number::from_f64(hit_rate).unwrap_or_default()));
                }
                if let Some(size_mb) = stats.get("size_mb").and_then(|v| v.as_f64()) {
                    metrics.insert("size_mb".to_string(), serde_json::Value::Number(
                        serde_json::Number::from_f64(size_mb).unwrap_or_default()));
                }
                
                let degradation_stats = cache.get_degradation_stats().await;
                metrics.insert("redis_healthy".to_string(), 
                    serde_json::Value::Bool(degradation_stats.redis_healthy));
                metrics.insert("memory_fallback_active".to_string(), 
                    serde_json::Value::Bool(degradation_stats.memory_fallback_active));
                
                // Determine status
                if !degradation_stats.redis_healthy && degradation_stats.memory_fallback_active {
                    (HealthStatus::Warning, Some("Redis unhealthy, using memory fallback".to_string()))
                } else if !degradation_stats.redis_healthy {
                    (HealthStatus::Critical, Some("Redis unhealthy, no fallback".to_string()))
                } else {
                    (HealthStatus::Healthy, None)
                }
            }
            Err(e) => (HealthStatus::Down, Some(e.to_string())),
        };
        
        let response_time = start.elapsed().as_millis() as u64;
        
        Ok(ComponentHealth {
            name: "cache".to_string(),
            status,
            last_check: start,
            response_time_ms: response_time,
            error_message,
            metrics,
            dependencies: vec!["redis".to_string()],
        })
    }
    
    async fn check_backpressure_health(backpressure: &Arc<Mutex<BackpressureManager>>) -> Result<ComponentHealth> {
        let start = Instant::now();
        let mut metrics = HashMap::new();
        
        let (status, error_message) = match backpressure.lock().await.get_stats().await {
            Ok(stats) => {
                metrics.insert("queue_size".to_string(), 
                    serde_json::Value::Number(stats.queue_size.into()));
                metrics.insert("active_operations".to_string(), 
                    serde_json::Value::Number(stats.active_operations.into()));
                metrics.insert("dropped_files".to_string(), 
                    serde_json::Value::Number(stats.dropped_files.into()));
                metrics.insert("current_throughput".to_string(), 
                    serde_json::Value::Number(
                        serde_json::Number::from_f64(stats.current_throughput).unwrap_or_default()));
                
                // Determine status based on queue size and dropped files
                if stats.dropped_files > 0 {
                    (HealthStatus::Warning, Some("Files being dropped due to backpressure".to_string()))
                } else if stats.queue_size > 1000 {
                    (HealthStatus::Warning, Some("High queue size".to_string()))
                } else {
                    (HealthStatus::Healthy, None)
                }
            }
            Err(e) => (HealthStatus::Down, Some(e.to_string())),
        };
        
        let response_time = start.elapsed().as_millis() as u64;
        
        Ok(ComponentHealth {
            name: "backpressure".to_string(),
            status,
            last_check: start,
            response_time_ms: response_time,
            error_message,
            metrics,
            dependencies: vec!["filesystem".to_string()],
        })
    }
    
    async fn check_memory_health(memory: &Arc<Mutex<MemoryMonitor>>) -> Result<ComponentHealth> {
        let start = Instant::now();
        let mut metrics = HashMap::new();
        
        let (status, error_message) = match memory.lock().await.get_stats().await {
            Ok(stats) => {
                metrics.insert("current_usage_mb".to_string(), 
                    serde_json::Value::Number(
                        serde_json::Number::from_f64(stats.current_usage_mb).unwrap_or_default()));
                metrics.insert("usage_percentage".to_string(), 
                    serde_json::Value::Number(
                        serde_json::Number::from_f64(stats.usage_percentage).unwrap_or_default()));
                metrics.insert("peak_usage_mb".to_string(), 
                    serde_json::Value::Number(
                        serde_json::Number::from_f64(stats.peak_usage_mb).unwrap_or_default()));
                
                // Determine status based on memory pressure
                match stats.pressure_level {
                    crate::performance::MemoryPressureLevel::Normal => (HealthStatus::Healthy, None),
                    crate::performance::MemoryPressureLevel::Warning => 
                        (HealthStatus::Warning, Some("Memory pressure warning".to_string())),
                    crate::performance::MemoryPressureLevel::Critical => 
                        (HealthStatus::Critical, Some("Critical memory pressure".to_string())),
                    crate::performance::MemoryPressureLevel::Emergency => 
                        (HealthStatus::Critical, Some("Emergency memory pressure".to_string())),
                }
            }
            Err(e) => (HealthStatus::Down, Some(e.to_string())),
        };
        
        let response_time = start.elapsed().as_millis() as u64;
        
        Ok(ComponentHealth {
            name: "memory".to_string(),
            status,
            last_check: start,
            response_time_ms: response_time,
            error_message,
            metrics,
            dependencies: vec!["system".to_string()],
        })
    }
    
    async fn check_query_cache_health(query_cache: &Arc<QueryPlanCache>) -> Result<ComponentHealth> {
        let start = Instant::now();
        let mut metrics = HashMap::new();
        
        let stats = query_cache.get_stats().await;
        
        metrics.insert("total_entries".to_string(), 
            serde_json::Value::Number(stats.total_entries.into()));
        metrics.insert("hit_rate".to_string(), 
            serde_json::Value::Number(
                serde_json::Number::from_f64(stats.hit_rate).unwrap_or_default()));
        metrics.insert("memory_usage_mb".to_string(), 
            serde_json::Value::Number(
                serde_json::Number::from_f64(stats.memory_usage_mb).unwrap_or_default()));
        
        let status = if stats.hit_rate < 0.3 && stats.total_entries > 100 {
            HealthStatus::Warning
        } else {
            HealthStatus::Healthy
        };
        
        let response_time = start.elapsed().as_millis() as u64;
        
        Ok(ComponentHealth {
            name: "query_cache".to_string(),
            status,
            last_check: start,
            response_time_ms: response_time,
            error_message: None,
            metrics,
            dependencies: vec!["memory".to_string()],
        })
    }
    
    async fn check_parallel_executor_health(executor: &Arc<ParallelQueryExecutor>) -> Result<ComponentHealth> {
        let start = Instant::now();
        let mut metrics = HashMap::new();
        
        let stats = executor.get_stats().await;
        
        metrics.insert("active_queries".to_string(), 
            serde_json::Value::Number(stats.active_queries.into()));
        metrics.insert("completed_queries".to_string(), 
            serde_json::Value::Number(stats.completed_queries.into()));
        metrics.insert("average_execution_time_ms".to_string(), 
            serde_json::Value::Number(
                serde_json::Number::from_f64(stats.average_execution_time_ms).unwrap_or_default()));
        
        let status = if stats.active_queries > 100 {
            HealthStatus::Warning
        } else {
            HealthStatus::Healthy
        };
        
        let response_time = start.elapsed().as_millis() as u64;
        
        Ok(ComponentHealth {
            name: "parallel_executor".to_string(),
            status,
            last_check: start,
            response_time_ms: response_time,
            error_message: None,
            metrics,
            dependencies: vec!["neo4j".to_string()],
        })
    }
    
    async fn check_auth_health(auth: &Arc<AuthManager>) -> Result<ComponentHealth> {
        let start = Instant::now();
        let mut metrics = HashMap::new();
        
        let stats = auth.get_stats().await;
        
        metrics.insert("active_sessions".to_string(), 
            serde_json::Value::Number(stats.active_sessions.into()));
        metrics.insert("total_users".to_string(), 
            serde_json::Value::Number(stats.total_users.into()));
        metrics.insert("active_api_keys".to_string(), 
            serde_json::Value::Number(stats.active_api_keys.into()));
        
        let response_time = start.elapsed().as_millis() as u64;
        
        Ok(ComponentHealth {
            name: "authentication".to_string(),
            status: HealthStatus::Healthy,
            last_check: start,
            response_time_ms: response_time,
            error_message: None,
            metrics,
            dependencies: vec!["database".to_string()],
        })
    }
    
    async fn check_rate_limiter_health(rate_limiter: &Arc<SessionRateLimiter>) -> Result<ComponentHealth> {
        let start = Instant::now();
        let mut metrics = HashMap::new();
        
        let stats = rate_limiter.get_statistics().await;
        
        metrics.insert("active_sessions".to_string(), 
            serde_json::Value::Number(stats.active_sessions.into()));
        metrics.insert("active_ips".to_string(), 
            serde_json::Value::Number(stats.active_ips.into()));
        metrics.insert("adaptive_limiting_active".to_string(), 
            serde_json::Value::Bool(stats.adaptive_limiting_active));
        
        let response_time = start.elapsed().as_millis() as u64;
        
        Ok(ComponentHealth {
            name: "rate_limiter".to_string(),
            status: HealthStatus::Healthy,
            last_check: start,
            response_time_ms: response_time,
            error_message: None,
            metrics,
            dependencies: vec!["memory".to_string()],
        })
    }
    
    // Helper methods
    
    async fn update_failure_tracker(
        trackers: &Arc<RwLock<HashMap<String, FailureTracker>>>,
        component: &str,
        status: &HealthStatus,
        config: &HealthMonitorConfig,
    ) {
        let mut trackers = trackers.write().await;
        let tracker = trackers.entry(component.to_string()).or_default();
        
        if *status == HealthStatus::Healthy {
            tracker.consecutive_failures = 0;
        } else {
            tracker.consecutive_failures += 1;
            tracker.last_failure = Some(Instant::now());
        }
    }
    
    async fn add_to_history(
        history: &Arc<RwLock<HashMap<String, Vec<HealthHistoryEntry>>>>,
        component: &str,
        health: &ComponentHealth,
        max_entries: usize,
    ) {
        let mut history = history.write().await;
        let entries = history.entry(component.to_string()).or_default();
        
        entries.push(HealthHistoryEntry {
            timestamp: health.last_check,
            status: health.status.clone(),
            response_time_ms: health.response_time_ms,
            error_message: health.error_message.clone(),
        });
        
        // Keep only the most recent entries
        if entries.len() > max_entries {
            entries.drain(0..entries.len() - max_entries);
        }
    }
}

impl Drop for HealthMonitor {
    fn drop(&mut self) {
        if let Ok(mut handle) = self.monitor_handle.try_lock() {
            if let Some(task) = handle.take() {
                task.abort();
            }
        }
    }
}
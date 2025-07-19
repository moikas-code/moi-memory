use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tokio::time::timeout;
use tracing::{warn, error, debug};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct TimeoutConfig {
    pub default_timeout: Duration,
    pub operation_timeouts: HashMap<OperationType, Duration>,
    pub cleanup_interval: Duration,
    pub max_concurrent_operations: usize,
}

impl Default for TimeoutConfig {
    fn default() -> Self {
        let mut operation_timeouts = HashMap::new();
        operation_timeouts.insert(OperationType::Query, Duration::from_secs(30));
        operation_timeouts.insert(OperationType::Analysis, Duration::from_secs(120));
        operation_timeouts.insert(OperationType::Export, Duration::from_secs(300));
        operation_timeouts.insert(OperationType::HealthCheck, Duration::from_secs(10));
        operation_timeouts.insert(OperationType::CacheOperation, Duration::from_secs(5));
        operation_timeouts.insert(OperationType::FileWatch, Duration::from_secs(60));
        
        Self {
            default_timeout: Duration::from_secs(60),
            operation_timeouts,
            cleanup_interval: Duration::from_secs(30),
            max_concurrent_operations: 1000,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OperationType {
    Query,
    Analysis,
    Export,
    HealthCheck,
    CacheOperation,
    FileWatch,
    DatabaseConnection,
    Custom(u8), // For extensibility
}

#[derive(Debug, Clone)]
pub struct OperationContext {
    pub id: String,
    pub operation_type: OperationType,
    pub started_at: Instant,
    pub timeout_duration: Duration,
    pub description: String,
    pub user_session: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TimeoutStats {
    pub total_operations: u64,
    pub timed_out_operations: u64,
    pub completed_operations: u64,
    pub average_duration_ms: u64,
    pub current_active_operations: usize,
    pub timeout_rate_percent: f64,
    pub slowest_operation_ms: u64,
    pub by_operation_type: HashMap<OperationType, OperationTypeStats>,
}

#[derive(Debug, Clone)]
pub struct OperationTypeStats {
    pub total: u64,
    pub timeouts: u64,
    pub average_duration_ms: u64,
    pub max_duration_ms: u64,
}

impl Default for TimeoutStats {
    fn default() -> Self {
        Self {
            total_operations: 0,
            timed_out_operations: 0,
            completed_operations: 0,
            average_duration_ms: 0,
            current_active_operations: 0,
            timeout_rate_percent: 0.0,
            slowest_operation_ms: 0,
            by_operation_type: HashMap::new(),
        }
    }
}

pub struct TimeoutManager {
    config: TimeoutConfig,
    active_operations: Arc<RwLock<HashMap<String, OperationContext>>>,
    stats: Arc<RwLock<TimeoutStats>>,
    operation_history: Arc<RwLock<Vec<(OperationContext, Duration, bool)>>>,
}

impl TimeoutManager {
    pub fn new(config: TimeoutConfig) -> Self {
        let manager = Self {
            config,
            active_operations: Arc::new(RwLock::new(HashMap::new())),
            stats: Arc::new(RwLock::new(TimeoutStats::default())),
            operation_history: Arc::new(RwLock::new(Vec::new())),
        };
        
        // Start cleanup task
        manager.start_cleanup_task();
        
        manager
    }
    
    pub fn with_default_config() -> Self {
        Self::new(TimeoutConfig::default())
    }
    
    /// Execute an operation with timeout protection
    pub async fn execute_with_timeout<F, T, E>(
        &self,
        operation_type: OperationType,
        description: String,
        user_session: Option<String>,
        operation: F,
    ) -> Result<T, TimeoutError<E>>
    where
        F: std::future::Future<Output = Result<T, E>>,
    {
        // Check if we're at max concurrent operations
        if self.active_operations.read().await.len() >= self.config.max_concurrent_operations {
            warn!("Max concurrent operations reached, rejecting new operation: {}", description);
            return Err(TimeoutError::TooManyOperations);
        }
        
        let timeout_duration = self.get_timeout_for_operation(operation_type);
        let operation_id = Uuid::new_v4().to_string();
        
        let context = OperationContext {
            id: operation_id.clone(),
            operation_type,
            started_at: Instant::now(),
            timeout_duration,
            description: description.clone(),
            user_session,
        };
        
        // Register operation
        self.register_operation(context.clone()).await;
        
        debug!(
            "Starting operation {} ({}) with timeout {:?}",
            operation_id, description, timeout_duration
        );
        
        // Execute with timeout
        let result = timeout(timeout_duration, operation).await;
        
        // Unregister and record result
        let duration = self.unregister_operation(&operation_id).await;
        
        match result {
            Ok(Ok(value)) => {
                self.record_completion(context, duration, false).await;
                debug!("Operation {} completed successfully in {:?}", operation_id, duration);
                Ok(value)
            }
            Ok(Err(error)) => {
                self.record_completion(context, duration, false).await;
                debug!("Operation {} failed in {:?}", operation_id, duration);
                Err(TimeoutError::OperationError(error))
            }
            Err(_) => {
                self.record_completion(context.clone(), duration, true).await;
                warn!(
                    "Operation {} ({}) timed out after {:?}",
                    operation_id, context.description, timeout_duration
                );
                Err(TimeoutError::TimedOut {
                    operation_id,
                    description: context.description,
                    timeout_duration,
                })
            }
        }
    }
    
    /// Execute with custom timeout
    pub async fn execute_with_custom_timeout<F, T, E>(
        &self,
        operation_type: OperationType,
        description: String,
        user_session: Option<String>,
        custom_timeout: Duration,
        operation: F,
    ) -> Result<T, TimeoutError<E>>
    where
        F: std::future::Future<Output = Result<T, E>>,
    {
        let operation_id = Uuid::new_v4().to_string();
        
        let context = OperationContext {
            id: operation_id.clone(),
            operation_type,
            started_at: Instant::now(),
            timeout_duration: custom_timeout,
            description: description.clone(),
            user_session,
        };
        
        self.register_operation(context.clone()).await;
        
        let result = timeout(custom_timeout, operation).await;
        let duration = self.unregister_operation(&operation_id).await;
        
        match result {
            Ok(Ok(value)) => {
                self.record_completion(context, duration, false).await;
                Ok(value)
            }
            Ok(Err(error)) => {
                self.record_completion(context, duration, false).await;
                Err(TimeoutError::OperationError(error))
            }
            Err(_) => {
                self.record_completion(context.clone(), duration, true).await;
                Err(TimeoutError::TimedOut {
                    operation_id,
                    description: context.description,
                    timeout_duration: custom_timeout,
                })
            }
        }
    }
    
    fn get_timeout_for_operation(&self, operation_type: OperationType) -> Duration {
        self.config.operation_timeouts
            .get(&operation_type)
            .copied()
            .unwrap_or(self.config.default_timeout)
    }
    
    async fn register_operation(&self, context: OperationContext) {
        let mut active = self.active_operations.write().await;
        active.insert(context.id.clone(), context);
        
        // Update stats
        let mut stats = self.stats.write().await;
        stats.total_operations += 1;
        stats.current_active_operations = active.len();
    }
    
    async fn unregister_operation(&self, operation_id: &str) -> Duration {
        let mut active = self.active_operations.write().await;
        let context = active.remove(operation_id);
        
        let duration = context
            .map(|ctx| ctx.started_at.elapsed())
            .unwrap_or(Duration::from_secs(0));
        
        // Update stats
        let mut stats = self.stats.write().await;
        stats.current_active_operations = active.len();
        
        duration
    }
    
    async fn record_completion(&self, context: OperationContext, duration: Duration, timed_out: bool) {
        let duration_ms = duration.as_millis() as u64;
        
        // Update stats
        {
            let mut stats = self.stats.write().await;
            
            if timed_out {
                stats.timed_out_operations += 1;
            } else {
                stats.completed_operations += 1;
            }
            
            // Update averages and maximums
            let total_completed = stats.completed_operations + stats.timed_out_operations;
            if total_completed > 0 {
                stats.timeout_rate_percent = (stats.timed_out_operations as f64 / total_completed as f64) * 100.0;
            }
            
            if duration_ms > stats.slowest_operation_ms {
                stats.slowest_operation_ms = duration_ms;
            }
            
            // Update per-operation-type stats
            let type_stats = stats.by_operation_type
                .entry(context.operation_type)
                .or_insert_with(|| OperationTypeStats {
                    total: 0,
                    timeouts: 0,
                    average_duration_ms: 0,
                    max_duration_ms: 0,
                });
            
            type_stats.total += 1;
            if timed_out {
                type_stats.timeouts += 1;
            }
            
            if duration_ms > type_stats.max_duration_ms {
                type_stats.max_duration_ms = duration_ms;
            }
            
            // Simple moving average
            type_stats.average_duration_ms = 
                (type_stats.average_duration_ms * (type_stats.total - 1) + duration_ms) / type_stats.total;
        }
        
        // Add to history (keep last 1000 operations)
        {
            let mut history = self.operation_history.write().await;
            history.push((context, duration, timed_out));
            
            // Keep only last 1000 operations
            if history.len() > 1000 {
                history.drain(0..history.len() - 1000);
            }
        }
    }
    
    pub async fn get_active_operations(&self) -> Vec<OperationContext> {
        self.active_operations.read().await.values().cloned().collect()
    }
    
    pub async fn get_stats(&self) -> TimeoutStats {
        self.stats.read().await.clone()
    }
    
    pub async fn get_long_running_operations(&self, threshold: Duration) -> Vec<OperationContext> {
        let active = self.active_operations.read().await;
        active.values()
            .filter(|ctx| ctx.started_at.elapsed() > threshold)
            .cloned()
            .collect()
    }
    
    pub async fn cancel_operation(&self, operation_id: &str) -> bool {
        let mut active = self.active_operations.write().await;
        if let Some(context) = active.remove(operation_id) {
            warn!("Manually cancelled operation: {} ({})", operation_id, context.description);
            
            // Record as timeout for stats purposes
            let duration = context.started_at.elapsed();
            drop(active); // Release lock before calling record_completion
            self.record_completion(context, duration, true).await;
            true
        } else {
            false
        }
    }
    
    fn start_cleanup_task(&self) {
        let active_operations = self.active_operations.clone();
        let stats = self.stats.clone();
        let cleanup_interval = self.config.cleanup_interval;
        
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(cleanup_interval);
            
            loop {
                interval.tick().await;
                
                // Clean up any operations that might have been orphaned
                let mut to_remove = Vec::new();
                {
                    let active = active_operations.read().await;
                    for (id, context) in active.iter() {
                        // Remove operations that have been running for more than 2x their timeout
                        if context.started_at.elapsed() > context.timeout_duration * 2 {
                            to_remove.push(id.clone());
                        }
                    }
                }
                
                if !to_remove.is_empty() {
                    let mut active = active_operations.write().await;
                    let mut stats_guard = stats.write().await;
                    
                    for id in to_remove {
                        if let Some(context) = active.remove(&id) {
                            warn!("Cleaning up orphaned operation: {} ({})", id, context.description);
                            stats_guard.timed_out_operations += 1;
                        }
                    }
                    
                    stats_guard.current_active_operations = active.len();
                }
            }
        });
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TimeoutError<E> {
    #[error("Operation timed out after {timeout_duration:?}: {description} (ID: {operation_id})")]
    TimedOut {
        operation_id: String,
        description: String,
        timeout_duration: Duration,
    },
    
    #[error("Operation failed: {0}")]
    OperationError(E),
    
    #[error("Too many concurrent operations, request rejected")]
    TooManyOperations,
}

impl<E> TimeoutError<E> {
    pub fn is_timeout(&self) -> bool {
        matches!(self, TimeoutError::TimedOut { .. })
    }
    
    pub fn is_operation_error(&self) -> bool {
        matches!(self, TimeoutError::OperationError(_))
    }
    
    pub fn is_capacity_error(&self) -> bool {
        matches!(self, TimeoutError::TooManyOperations)
    }
}

// Helper macro for easy timeout wrapping
#[macro_export]
macro_rules! with_timeout {
    ($timeout_manager:expr, $op_type:expr, $description:expr, $operation:expr) => {
        $timeout_manager.execute_with_timeout(
            $op_type,
            $description.to_string(),
            None,
            $operation
        ).await
    };
    
    ($timeout_manager:expr, $op_type:expr, $description:expr, $session:expr, $operation:expr) => {
        $timeout_manager.execute_with_timeout(
            $op_type,
            $description.to_string(),
            Some($session.to_string()),
            $operation
        ).await
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::sleep;
    
    #[tokio::test]
    async fn test_successful_operation() {
        let manager = TimeoutManager::with_default_config();
        
        let result = manager.execute_with_timeout(
            OperationType::Query,
            "test operation".to_string(),
            None,
            async { Ok::<i32, String>(42) }
        ).await;
        
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 42);
        
        let stats = manager.get_stats().await;
        assert_eq!(stats.completed_operations, 1);
        assert_eq!(stats.timed_out_operations, 0);
    }
    
    #[tokio::test]
    async fn test_operation_timeout() {
        let mut config = TimeoutConfig::default();
        config.operation_timeouts.insert(OperationType::Query, Duration::from_millis(100));
        
        let manager = TimeoutManager::new(config);
        
        let result = manager.execute_with_timeout(
            OperationType::Query,
            "slow operation".to_string(),
            None,
            async {
                sleep(Duration::from_millis(200)).await;
                Ok::<i32, String>(42)
            }
        ).await;
        
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), TimeoutError::TimedOut { .. }));
        
        let stats = manager.get_stats().await;
        assert_eq!(stats.timed_out_operations, 1);
        assert_eq!(stats.completed_operations, 0);
    }
    
    #[tokio::test]
    async fn test_operation_error() {
        let manager = TimeoutManager::with_default_config();
        
        let result = manager.execute_with_timeout(
            OperationType::Query,
            "failing operation".to_string(),
            None,
            async { Err::<i32, String>("operation failed".to_string()) }
        ).await;
        
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), TimeoutError::OperationError(_)));
        
        let stats = manager.get_stats().await;
        assert_eq!(stats.completed_operations, 1); // Failed operations count as completed
        assert_eq!(stats.timed_out_operations, 0);
    }
    
    #[tokio::test]
    async fn test_concurrent_operations_limit() {
        let mut config = TimeoutConfig::default();
        config.max_concurrent_operations = 2;
        
        let manager = Arc::new(TimeoutManager::new(config));
        
        // Start two long-running operations
        let manager1 = manager.clone();
        let manager2 = manager.clone();
        let manager3 = manager.clone();
        
        let handle1 = tokio::spawn(async move {
            manager1.execute_with_timeout(
                OperationType::Query,
                "operation 1".to_string(),
                None,
                async {
                    sleep(Duration::from_millis(100)).await;
                    Ok::<i32, String>(1)
                }
            ).await
        });
        
        let handle2 = tokio::spawn(async move {
            manager2.execute_with_timeout(
                OperationType::Query,
                "operation 2".to_string(),
                None,
                async {
                    sleep(Duration::from_millis(100)).await;
                    Ok::<i32, String>(2)
                }
            ).await
        });
        
        // Give the operations time to start
        sleep(Duration::from_millis(10)).await;
        
        // Third operation should be rejected
        let result3 = manager3.execute_with_timeout(
            OperationType::Query,
            "operation 3".to_string(),
            None,
            async { Ok::<i32, String>(3) }
        ).await;
        
        assert!(matches!(result3.unwrap_err(), TimeoutError::TooManyOperations));
        
        // Wait for first two to complete
        let result1 = handle1.await.unwrap();
        let result2 = handle2.await.unwrap();
        
        assert!(result1.is_ok());
        assert!(result2.is_ok());
    }
}
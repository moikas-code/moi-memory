use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{debug, warn, error, info};

/// Configuration for memory monitoring
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryMonitorConfig {
    /// Memory limit in MB (0 = no limit)
    pub memory_limit_mb: usize,
    /// Warning threshold as percentage of limit (0.0 - 1.0)
    pub warning_threshold: f64,
    /// Critical threshold as percentage of limit (0.0 - 1.0)
    pub critical_threshold: f64,
    /// Monitoring interval in seconds
    pub monitoring_interval_seconds: u64,
    /// Enable garbage collection hints
    pub enable_gc_hints: bool,
    /// Enable detailed memory tracking
    pub enable_detailed_tracking: bool,
    /// Enable memory pressure notifications
    pub enable_pressure_notifications: bool,
    /// Memory check interval for operations (milliseconds)
    pub operation_check_interval_ms: u64,
    /// Enable adaptive limits based on system memory
    pub enable_adaptive_limits: bool,
}

impl Default for MemoryMonitorConfig {
    fn default() -> Self {
        Self {
            memory_limit_mb: 1024, // 1GB default
            warning_threshold: 0.8,
            critical_threshold: 0.95,
            monitoring_interval_seconds: 30,
            enable_gc_hints: true,
            enable_detailed_tracking: true,
            enable_pressure_notifications: true,
            operation_check_interval_ms: 1000,
            enable_adaptive_limits: true,
        }
    }
}

/// Memory statistics and usage information
#[derive(Debug, Clone, Serialize)]
pub struct MemoryStats {
    /// Current memory usage in MB
    pub current_usage_mb: f64,
    /// Peak memory usage in MB
    pub peak_usage_mb: f64,
    /// Memory limit in MB
    pub limit_mb: usize,
    /// Usage percentage (0.0 - 1.0)
    pub usage_percentage: f64,
    /// Memory pressure level
    pub pressure_level: MemoryPressureLevel,
    /// Number of GC hints triggered
    pub gc_hints_triggered: u64,
    /// Number of out-of-memory warnings
    pub oom_warnings: u64,
    /// Last memory check timestamp
    pub last_check: Instant,
    /// Memory breakdown by component
    pub component_usage: MemoryComponentUsage,
    /// System memory info
    pub system_memory: SystemMemoryInfo,
}

/// Memory pressure levels
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum MemoryPressureLevel {
    /// Normal memory usage
    Normal,
    /// Warning level - approaching limits
    Warning,
    /// Critical level - near memory limit
    Critical,
    /// Emergency level - at or over limit
    Emergency,
}

/// Memory usage breakdown by component
#[derive(Debug, Clone, Serialize)]
pub struct MemoryComponentUsage {
    /// Query cache memory usage in MB
    pub query_cache_mb: f64,
    /// Entity cache memory usage in MB
    pub entity_cache_mb: f64,
    /// Connection pool memory usage in MB
    pub connection_pool_mb: f64,
    /// Active queries memory usage in MB
    pub active_queries_mb: f64,
    /// Backpressure queues memory usage in MB
    pub backpressure_queues_mb: f64,
    /// Other components memory usage in MB
    pub other_mb: f64,
}

/// System memory information
#[derive(Debug, Clone, Serialize)]
pub struct SystemMemoryInfo {
    /// Total system memory in MB
    pub total_mb: f64,
    /// Available system memory in MB
    pub available_mb: f64,
    /// System memory usage percentage
    pub system_usage_percentage: f64,
    /// Process memory usage in MB
    pub process_usage_mb: f64,
}

/// Memory monitoring and management
pub struct MemoryMonitor {
    config: MemoryMonitorConfig,
    
    /// Current memory statistics
    stats: Arc<RwLock<MemoryStats>>,
    
    /// Memory pressure callbacks
    pressure_callbacks: Arc<RwLock<Vec<Box<dyn Fn(MemoryPressureLevel) + Send + Sync>>>>,
    
    /// Monitoring task handle
    monitoring_task: Option<tokio::task::JoinHandle<()>>,
}

impl MemoryMonitor {
    /// Create a new memory monitor
    pub async fn new(config: MemoryMonitorConfig) -> Result<Self> {
        let initial_stats = MemoryStats {
            current_usage_mb: 0.0,
            peak_usage_mb: 0.0,
            limit_mb: config.memory_limit_mb,
            usage_percentage: 0.0,
            pressure_level: MemoryPressureLevel::Normal,
            gc_hints_triggered: 0,
            oom_warnings: 0,
            last_check: Instant::now(),
            component_usage: MemoryComponentUsage {
                query_cache_mb: 0.0,
                entity_cache_mb: 0.0,
                connection_pool_mb: 0.0,
                active_queries_mb: 0.0,
                backpressure_queues_mb: 0.0,
                other_mb: 0.0,
            },
            system_memory: SystemMemoryInfo {
                total_mb: 0.0,
                available_mb: 0.0,
                system_usage_percentage: 0.0,
                process_usage_mb: 0.0,
            },
        };
        
        let monitor = Self {
            config,
            stats: Arc::new(RwLock::new(initial_stats)),
            pressure_callbacks: Arc::new(RwLock::new(Vec::new())),
            monitoring_task: None,
        };
        
        Ok(monitor)
    }
    
    /// Start memory monitoring
    pub async fn start(&mut self) -> Result<()> {
        if self.monitoring_task.is_some() {
            return Err(anyhow!("Memory monitor is already running"));
        }
        
        let stats = self.stats.clone();
        let config = self.config.clone();
        let pressure_callbacks = self.pressure_callbacks.clone();
        
        let task = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(config.monitoring_interval_seconds));
            
            loop {
                interval.tick().await;
                
                if let Err(e) = Self::update_memory_stats(&stats, &config, &pressure_callbacks).await {
                    error!("Failed to update memory stats: {}", e);
                }
            }
        });
        
        self.monitoring_task = Some(task);
        info!("Memory monitor started");
        Ok(())
    }
    
    /// Stop memory monitoring
    pub async fn stop(&mut self) {
        if let Some(task) = self.monitoring_task.take() {
            task.abort();
            info!("Memory monitor stopped");
        }
    }
    
    /// Get current memory statistics
    pub async fn get_stats(&self) -> MemoryStats {
        self.stats.read().await.clone()
    }
    
    /// Check if operation should proceed based on memory pressure
    pub async fn should_allow_operation(&self, estimated_memory_mb: f64) -> bool {
        let stats = self.stats.read().await;
        
        let projected_usage = stats.current_usage_mb + estimated_memory_mb;
        let projected_percentage = projected_usage / stats.limit_mb as f64;
        
        match stats.pressure_level {
            MemoryPressureLevel::Normal => projected_percentage < self.config.warning_threshold,
            MemoryPressureLevel::Warning => projected_percentage < self.config.critical_threshold,
            MemoryPressureLevel::Critical => projected_percentage < 1.0,
            MemoryPressureLevel::Emergency => false,
        }
    }
    
    /// Register callback for memory pressure notifications
    pub async fn register_pressure_callback<F>(&self, callback: F)
    where
        F: Fn(MemoryPressureLevel) + Send + Sync + 'static,
    {
        let mut callbacks = self.pressure_callbacks.write().await;
        callbacks.push(Box::new(callback));
    }
    
    /// Manually trigger garbage collection hint
    pub async fn trigger_gc_hint(&self) -> Result<()> {
        if !self.config.enable_gc_hints {
            return Ok(());
        }
        
        // In Rust, we don't have direct GC control like in Java or .NET
        // But we can encourage memory cleanup through other means
        
        // Update statistics
        {
            let mut stats = self.stats.write().await;
            stats.gc_hints_triggered += 1;
        }
        
        // Force a manual memory update
        self.force_memory_update().await?;
        
        info!("Garbage collection hint triggered");
        Ok(())
    }
    
    /// Force immediate memory statistics update
    pub async fn force_memory_update(&self) -> Result<()> {
        Self::update_memory_stats(&self.stats, &self.config, &self.pressure_callbacks).await
    }
    
    /// Update memory statistics
    async fn update_memory_stats(
        stats: Arc<RwLock<MemoryStats>>,
        config: &MemoryMonitorConfig,
        pressure_callbacks: &Arc<RwLock<Vec<Box<dyn Fn(MemoryPressureLevel) + Send + Sync>>>>,
    ) -> Result<()> {
        let system_memory = Self::get_system_memory_info()?;
        let process_memory = Self::get_process_memory_usage()?;
        let component_usage = Self::get_component_memory_usage().await;
        
        let current_usage_mb = process_memory;
        let usage_percentage = current_usage_mb / config.memory_limit_mb as f64;
        
        // Determine pressure level
        let pressure_level = if usage_percentage >= 1.0 {
            MemoryPressureLevel::Emergency
        } else if usage_percentage >= config.critical_threshold {
            MemoryPressureLevel::Critical
        } else if usage_percentage >= config.warning_threshold {
            MemoryPressureLevel::Warning
        } else {
            MemoryPressureLevel::Normal
        };
        
        let previous_pressure_level = {
            let stats_guard = stats.read().await;
            stats_guard.pressure_level
        };
        
        // Update statistics
        {
            let mut stats_guard = stats.write().await;
            
            stats_guard.current_usage_mb = current_usage_mb;
            stats_guard.peak_usage_mb = stats_guard.peak_usage_mb.max(current_usage_mb);
            stats_guard.usage_percentage = usage_percentage;
            stats_guard.pressure_level = pressure_level;
            stats_guard.last_check = Instant::now();
            stats_guard.component_usage = component_usage;
            stats_guard.system_memory = system_memory;
            
            // Update warning count if in emergency state
            if pressure_level == MemoryPressureLevel::Emergency {
                stats_guard.oom_warnings += 1;
            }
        }
        
        // Trigger callbacks if pressure level changed
        if pressure_level != previous_pressure_level {
            let callbacks = pressure_callbacks.read().await;
            for callback in callbacks.iter() {
                callback(pressure_level);
            }
            
            match pressure_level {
                MemoryPressureLevel::Warning => {
                    warn!("Memory usage approaching limit: {:.1}MB ({:.1}%)", 
                          current_usage_mb, usage_percentage * 100.0);
                }
                MemoryPressureLevel::Critical => {
                    error!("Memory usage critical: {:.1}MB ({:.1}%)", 
                           current_usage_mb, usage_percentage * 100.0);
                }
                MemoryPressureLevel::Emergency => {
                    error!("Memory usage exceeded limit: {:.1}MB ({:.1}%)", 
                           current_usage_mb, usage_percentage * 100.0);
                }
                MemoryPressureLevel::Normal => {
                    info!("Memory usage returned to normal: {:.1}MB ({:.1}%)", 
                          current_usage_mb, usage_percentage * 100.0);
                }
            }
        }
        
        debug!("Memory stats updated: {:.1}MB ({:.1}%)", current_usage_mb, usage_percentage * 100.0);
        Ok(())
    }
    
    /// Get system memory information
    fn get_system_memory_info() -> Result<SystemMemoryInfo> {
        // This is a simplified implementation
        // In a real implementation, you'd use system APIs or crates like `sysinfo`
        
        #[cfg(target_os = "linux")]
        {
            // On Linux, read from /proc/meminfo
            match std::fs::read_to_string("/proc/meminfo") {
                Ok(content) => {
                    let mut total_kb = 0;
                    let mut available_kb = 0;
                    
                    for line in content.lines() {
                        if line.starts_with("MemTotal:") {
                            if let Some(value) = line.split_whitespace().nth(1) {
                                total_kb = value.parse::<u64>().unwrap_or(0);
                            }
                        } else if line.starts_with("MemAvailable:") {
                            if let Some(value) = line.split_whitespace().nth(1) {
                                available_kb = value.parse::<u64>().unwrap_or(0);
                            }
                        }
                    }
                    
                    let total_mb = total_kb as f64 / 1024.0;
                    let available_mb = available_kb as f64 / 1024.0;
                    let used_mb = total_mb - available_mb;
                    let usage_percentage = if total_mb > 0.0 { used_mb / total_mb } else { 0.0 };
                    
                    Ok(SystemMemoryInfo {
                        total_mb,
                        available_mb,
                        system_usage_percentage: usage_percentage,
                        process_usage_mb: 0.0, // Will be filled separately
                    })
                }
                Err(_) => {
                    // Fallback to estimates
                    Ok(SystemMemoryInfo {
                        total_mb: 8192.0, // 8GB estimate
                        available_mb: 4096.0, // 4GB estimate
                        system_usage_percentage: 0.5,
                        process_usage_mb: 0.0,
                    })
                }
            }
        }
        
        #[cfg(not(target_os = "linux"))]
        {
            // For other platforms, return estimates
            Ok(SystemMemoryInfo {
                total_mb: 8192.0, // 8GB estimate
                available_mb: 4096.0, // 4GB estimate
                system_usage_percentage: 0.5,
                process_usage_mb: 0.0,
            })
        }
    }
    
    /// Get process memory usage
    fn get_process_memory_usage() -> Result<f64> {
        // This is a simplified implementation
        // In a real implementation, you'd use system APIs or crates like `sysinfo`
        
        #[cfg(target_os = "linux")]
        {
            // On Linux, read from /proc/self/status
            match std::fs::read_to_string("/proc/self/status") {
                Ok(content) => {
                    for line in content.lines() {
                        if line.starts_with("VmRSS:") {
                            if let Some(value) = line.split_whitespace().nth(1) {
                                let kb = value.parse::<u64>().unwrap_or(0);
                                return Ok(kb as f64 / 1024.0); // Convert to MB
                            }
                        }
                    }
                    Ok(128.0) // Fallback estimate
                }
                Err(_) => Ok(128.0), // Fallback estimate
            }
        }
        
        #[cfg(not(target_os = "linux"))]
        {
            // For other platforms, return estimate
            Ok(128.0) // 128MB estimate
        }
    }
    
    /// Get memory usage breakdown by component
    async fn get_component_memory_usage() -> MemoryComponentUsage {
        // This would be implemented to query actual component memory usage
        // For now, return estimates
        
        MemoryComponentUsage {
            query_cache_mb: 10.0,
            entity_cache_mb: 20.0,
            connection_pool_mb: 5.0,
            active_queries_mb: 15.0,
            backpressure_queues_mb: 8.0,
            other_mb: 70.0,
        }
    }
    
    /// Check if memory limit allows for new allocation
    pub async fn check_allocation(&self, size_mb: f64) -> Result<bool> {
        let stats = self.stats.read().await;
        
        let projected_usage = stats.current_usage_mb + size_mb;
        let projected_percentage = projected_usage / stats.limit_mb as f64;
        
        if projected_percentage > 1.0 {
            warn!("Memory allocation denied: would exceed limit ({:.1}MB + {:.1}MB > {:.1}MB)", 
                  stats.current_usage_mb, size_mb, stats.limit_mb as f64);
            return Ok(false);
        }
        
        Ok(true)
    }
    
    /// Get memory recommendations
    pub async fn get_recommendations(&self) -> Vec<String> {
        let stats = self.stats.read().await;
        let mut recommendations = Vec::new();
        
        match stats.pressure_level {
            MemoryPressureLevel::Warning => {
                recommendations.push("Consider clearing query cache".to_string());
                recommendations.push("Reduce batch sizes for operations".to_string());
            }
            MemoryPressureLevel::Critical => {
                recommendations.push("Clear all non-essential caches".to_string());
                recommendations.push("Reduce concurrent operations".to_string());
                recommendations.push("Consider increasing memory limit".to_string());
            }
            MemoryPressureLevel::Emergency => {
                recommendations.push("Emergency: Stop non-critical operations".to_string());
                recommendations.push("Force garbage collection".to_string());
                recommendations.push("Clear all caches immediately".to_string());
                recommendations.push("Restart service if memory continues to grow".to_string());
            }
            MemoryPressureLevel::Normal => {
                recommendations.push("Memory usage is healthy".to_string());
            }
        }
        
        recommendations
    }
}

impl Drop for MemoryMonitor {
    fn drop(&mut self) {
        if let Some(task) = self.monitoring_task.take() {
            task.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_memory_monitor_creation() {
        let config = MemoryMonitorConfig::default();
        let monitor = MemoryMonitor::new(config).await.unwrap();
        
        let stats = monitor.get_stats().await;
        assert_eq!(stats.limit_mb, 1024);
        assert_eq!(stats.pressure_level, MemoryPressureLevel::Normal);
    }
    
    #[tokio::test]
    async fn test_pressure_level_calculation() {
        let mut config = MemoryMonitorConfig::default();
        config.memory_limit_mb = 100;
        config.warning_threshold = 0.8;
        config.critical_threshold = 0.9;
        
        let monitor = MemoryMonitor::new(config).await.unwrap();
        
        // Test allocation check
        assert!(monitor.check_allocation(50.0).await.unwrap()); // Should be allowed
        assert!(!monitor.check_allocation(150.0).await.unwrap()); // Should be denied
    }
    
    #[tokio::test]
    async fn test_memory_recommendations() {
        let config = MemoryMonitorConfig::default();
        let monitor = MemoryMonitor::new(config).await.unwrap();
        
        let recommendations = monitor.get_recommendations().await;
        assert!(!recommendations.is_empty());
        assert!(recommendations.iter().any(|r| r.contains("healthy")));
    }
}
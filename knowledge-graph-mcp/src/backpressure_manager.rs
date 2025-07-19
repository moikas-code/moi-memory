use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, RwLock, Semaphore};
use tokio::time::{interval, sleep};
use tracing::{debug, warn, error, info};

/// Configuration for backpressure handling
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackpressureConfig {
    /// Maximum number of file changes to queue before dropping
    pub max_queue_size: usize,
    /// Maximum concurrent file processing operations
    pub max_concurrent_operations: usize,
    /// Batch size for processing file changes
    pub batch_size: usize,
    /// Maximum time to wait before processing a partial batch (milliseconds)
    pub batch_timeout_ms: u64,
    /// Rate limit: maximum files per second to process
    pub max_files_per_second: f64,
    /// Memory threshold percentage (0-100) to trigger aggressive mode
    pub memory_threshold_percent: f64,
    /// CPU threshold percentage (0-100) to trigger throttling
    pub cpu_threshold_percent: f64,
    /// Enable adaptive batching based on system load
    pub adaptive_batching: bool,
}

impl Default for BackpressureConfig {
    fn default() -> Self {
        Self {
            max_queue_size: 10000,
            max_concurrent_operations: 10,
            batch_size: 50,
            batch_timeout_ms: 1000,
            max_files_per_second: 100.0,
            memory_threshold_percent: 80.0,
            cpu_threshold_percent: 85.0,
            adaptive_batching: true,
        }
    }
}

/// File change event with metadata
#[derive(Debug, Clone)]
pub struct FileChangeEvent {
    pub path: PathBuf,
    pub timestamp: Instant,
    pub change_type: FileChangeType,
    pub priority: FilePriority,
    pub size_bytes: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileChangeType {
    Created,
    Modified,
    Deleted,
    Renamed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FilePriority {
    Low = 0,
    Normal = 1,
    High = 2,
    Critical = 3,
}

/// Backpressure state and statistics
#[derive(Debug, Clone, Serialize)]
pub struct BackpressureStats {
    pub queue_size: usize,
    pub active_operations: usize,
    pub processed_files: u64,
    pub dropped_files: u64,
    pub batches_processed: u64,
    pub current_throughput: f64,
    pub average_processing_time_ms: f64,
    pub mode: BackpressureMode,
    pub memory_usage_percent: f64,
    pub cpu_usage_percent: f64,
    pub last_batch_size: usize,
    pub last_batch_time: Option<Instant>,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub enum BackpressureMode {
    Normal,
    Throttled,
    Aggressive,
    Emergency,
}

/// Manages backpressure for file change processing
pub struct BackpressureManager {
    config: BackpressureConfig,
    
    /// Priority queues for different file priorities
    queues: Arc<RwLock<HashMap<FilePriority, VecDeque<FileChangeEvent>>>>,
    
    /// Semaphore for limiting concurrent operations
    operation_semaphore: Arc<Semaphore>,
    
    /// Statistics and state
    stats: Arc<RwLock<BackpressureStats>>,
    
    /// Rate limiting state
    rate_limiter: Arc<RwLock<RateLimiter>>,
    
    /// System monitoring
    system_monitor: Arc<RwLock<SystemMonitor>>,
    
    /// Shutdown signal
    shutdown_tx: Option<mpsc::Sender<()>>,
}

/// Rate limiter using token bucket algorithm
#[derive(Debug)]
struct RateLimiter {
    tokens: f64,
    max_tokens: f64,
    refill_rate: f64,
    last_refill: Instant,
}

impl RateLimiter {
    fn new(max_rate: f64) -> Self {
        Self {
            tokens: max_rate,
            max_tokens: max_rate,
            refill_rate: max_rate,
            last_refill: Instant::now(),
        }
    }
    
    /// Try to consume tokens, returns true if successful
    fn try_consume(&mut self, tokens: f64) -> bool {
        self.refill();
        if self.tokens >= tokens {
            self.tokens -= tokens;
            true
        } else {
            false
        }
    }
    
    /// Get time until next token is available
    fn time_until_token(&self) -> Duration {
        if self.tokens >= 1.0 {
            Duration::ZERO
        } else {
            let tokens_needed = 1.0 - self.tokens;
            let time_needed = tokens_needed / self.refill_rate;
            Duration::from_secs_f64(time_needed)
        }
    }
    
    fn refill(&mut self) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        let tokens_to_add = elapsed * self.refill_rate;
        
        self.tokens = (self.tokens + tokens_to_add).min(self.max_tokens);
        self.last_refill = now;
    }
}

/// System resource monitor
#[derive(Debug)]
struct SystemMonitor {
    memory_usage_percent: f64,
    cpu_usage_percent: f64,
    last_update: Instant,
    update_interval: Duration,
}

impl SystemMonitor {
    fn new() -> Self {
        Self {
            memory_usage_percent: 0.0,
            cpu_usage_percent: 0.0,
            last_update: Instant::now(),
            update_interval: Duration::from_secs(5),
        }
    }
    
    /// Update system metrics if enough time has passed
    async fn maybe_update(&mut self) {
        let now = Instant::now();
        if now.duration_since(self.last_update) >= self.update_interval {
            self.update_metrics().await;
            self.last_update = now;
        }
    }
    
    /// Update system metrics (simplified implementation)
    async fn update_metrics(&mut self) {
        // In a real implementation, this would use system APIs
        // For now, we'll simulate based on queue pressure
        
        // Simple heuristic: assume memory/CPU pressure correlates with processing load
        self.memory_usage_percent = (self.memory_usage_percent * 0.8 + 20.0).min(100.0);
        self.cpu_usage_percent = (self.cpu_usage_percent * 0.7 + 30.0).min(100.0);
    }
    
    fn get_memory_usage(&self) -> f64 {
        self.memory_usage_percent
    }
    
    fn get_cpu_usage(&self) -> f64 {
        self.cpu_usage_percent
    }
}

impl BackpressureManager {
    /// Create a new backpressure manager with default configuration
    pub async fn new() -> Result<Self> {
        Self::with_config(BackpressureConfig::default()).await
    }
    
    /// Create a new backpressure manager with custom configuration
    pub async fn with_config(config: BackpressureConfig) -> Result<Self> {
        let operation_semaphore = Arc::new(Semaphore::new(config.max_concurrent_operations));
        let queues = Arc::new(RwLock::new(HashMap::new()));
        let rate_limiter = Arc::new(RwLock::new(RateLimiter::new(config.max_files_per_second)));
        let system_monitor = Arc::new(RwLock::new(SystemMonitor::new()));
        
        // Initialize priority queues
        {
            let mut queues_guard = queues.write().await;
            queues_guard.insert(FilePriority::Critical, VecDeque::new());
            queues_guard.insert(FilePriority::High, VecDeque::new());
            queues_guard.insert(FilePriority::Normal, VecDeque::new());
            queues_guard.insert(FilePriority::Low, VecDeque::new());
        }
        
        let stats = Arc::new(RwLock::new(BackpressureStats {
            queue_size: 0,
            active_operations: 0,
            processed_files: 0,
            dropped_files: 0,
            batches_processed: 0,
            current_throughput: 0.0,
            average_processing_time_ms: 0.0,
            mode: BackpressureMode::Normal,
            memory_usage_percent: 0.0,
            cpu_usage_percent: 0.0,
            last_batch_size: 0,
            last_batch_time: None,
        }));
        
        Ok(Self {
            config,
            queues,
            operation_semaphore,
            stats,
            rate_limiter,
            system_monitor,
            shutdown_tx: None,
        })
    }
    
    /// Start the backpressure manager background tasks
    pub async fn start(&mut self) -> Result<mpsc::Receiver<Vec<FileChangeEvent>>> {
        let (batch_tx, batch_rx) = mpsc::channel(100);
        let (shutdown_tx, mut shutdown_rx) = mpsc::channel(1);
        
        self.shutdown_tx = Some(shutdown_tx);
        
        // Start batch processor
        let queues = self.queues.clone();
        let config = self.config.clone();
        let stats = self.stats.clone();
        let rate_limiter = self.rate_limiter.clone();
        let system_monitor = self.system_monitor.clone();
        
        tokio::spawn(async move {
            let mut batch_interval = interval(Duration::from_millis(config.batch_timeout_ms));
            
            loop {
                tokio::select! {
                    _ = batch_interval.tick() => {
                        if let Err(e) = Self::process_batches(
                            &queues,
                            &config,
                            &stats,
                            &rate_limiter,
                            &system_monitor,
                            &batch_tx,
                        ).await {
                            error!("Batch processing error: {}", e);
                        }
                    }
                    _ = shutdown_rx.recv() => {
                        info!("Backpressure manager shutting down");
                        break;
                    }
                }
            }
        });
        
        Ok(batch_rx)
    }
    
    /// Add a file change event to the processing queue
    pub async fn enqueue_file_change(&self, event: FileChangeEvent) -> Result<bool> {
        // Update system monitoring
        {
            let mut monitor = self.system_monitor.write().await;
            monitor.maybe_update().await;
        }
        
        // Determine backpressure mode
        let mode = self.determine_backpressure_mode().await;
        
        // Update stats with current mode
        {
            let mut stats = self.stats.write().await;
            stats.mode = mode;
            stats.memory_usage_percent = self.system_monitor.read().await.get_memory_usage();
            stats.cpu_usage_percent = self.system_monitor.read().await.get_cpu_usage();
        }
        
        // Apply backpressure based on mode
        match mode {
            BackpressureMode::Emergency => {
                // Drop all but critical files
                if event.priority != FilePriority::Critical {
                    let mut stats = self.stats.write().await;
                    stats.dropped_files += 1;
                    debug!("Dropped file change in emergency mode: {:?}", event.path);
                    return Ok(false);
                }
            }
            BackpressureMode::Aggressive => {
                // Drop low priority files, delay others
                if event.priority == FilePriority::Low {
                    let mut stats = self.stats.write().await;
                    stats.dropped_files += 1;
                    debug!("Dropped low priority file in aggressive mode: {:?}", event.path);
                    return Ok(false);
                }
                
                // Add delay for non-critical files
                if event.priority != FilePriority::Critical {
                    sleep(Duration::from_millis(100)).await;
                }
            }
            BackpressureMode::Throttled => {
                // Rate limit all operations
                let wait_time = {
                    let rate_limiter = self.rate_limiter.read().await;
                    rate_limiter.time_until_token()
                };
                
                if !wait_time.is_zero() {
                    sleep(wait_time).await;
                }
            }
            BackpressureMode::Normal => {
                // No additional restrictions
            }
        }
        
        // Check total queue size
        let total_queue_size = self.get_total_queue_size().await;
        if total_queue_size >= self.config.max_queue_size {
            // Drop lowest priority items first
            if !self.make_room_for_event(&event).await {
                let mut stats = self.stats.write().await;
                stats.dropped_files += 1;
                warn!("Queue full, dropped file change: {:?}", event.path);
                return Ok(false);
            }
        }
        
        // Add to appropriate priority queue
        {
            let mut queues = self.queues.write().await;
            if let Some(queue) = queues.get_mut(&event.priority) {
                queue.push_back(event);
                
                // Update queue size in stats
                let mut stats = self.stats.write().await;
                stats.queue_size = self.calculate_total_queue_size(&queues);
            }
        }
        
        Ok(true)
    }
    
    /// Get current backpressure statistics
    pub async fn get_stats(&self) -> BackpressureStats {
        self.stats.read().await.clone()
    }
    
    /// Shutdown the backpressure manager
    pub async fn shutdown(&mut self) -> Result<()> {
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(()).await;
        }
        Ok(())
    }
    
    /// Determine current backpressure mode based on system state
    async fn determine_backpressure_mode(&self) -> BackpressureMode {
        let monitor = self.system_monitor.read().await;
        let memory_usage = monitor.get_memory_usage();
        let cpu_usage = monitor.get_cpu_usage();
        let queue_size = self.get_total_queue_size().await;
        let queue_pressure = queue_size as f64 / self.config.max_queue_size as f64;
        
        // Emergency mode: extremely high resource usage or queue nearly full
        if memory_usage > 95.0 || cpu_usage > 95.0 || queue_pressure > 0.9 {
            return BackpressureMode::Emergency;
        }
        
        // Aggressive mode: high resource usage or queue mostly full
        if memory_usage > self.config.memory_threshold_percent 
            || cpu_usage > self.config.cpu_threshold_percent 
            || queue_pressure > 0.7 {
            return BackpressureMode::Aggressive;
        }
        
        // Throttled mode: moderate resource usage
        if memory_usage > 60.0 || cpu_usage > 70.0 || queue_pressure > 0.5 {
            return BackpressureMode::Throttled;
        }
        
        BackpressureMode::Normal
    }
    
    /// Process batches of file changes
    async fn process_batches(
        queues: &Arc<RwLock<HashMap<FilePriority, VecDeque<FileChangeEvent>>>>,
        config: &BackpressureConfig,
        stats: &Arc<RwLock<BackpressureStats>>,
        rate_limiter: &Arc<RwLock<RateLimiter>>,
        system_monitor: &Arc<RwLock<SystemMonitor>>,
        batch_tx: &mpsc::Sender<Vec<FileChangeEvent>>,
    ) -> Result<()> {
        let mut batch = Vec::new();
        let start_time = Instant::now();
        
        // Determine batch size based on system load if adaptive batching is enabled
        let effective_batch_size = if config.adaptive_batching {
            let monitor = system_monitor.read().await;
            let load_factor = (monitor.get_cpu_usage() + monitor.get_memory_usage()) / 200.0;
            let adaptive_size = (config.batch_size as f64 * (1.0 - load_factor * 0.5)) as usize;
            adaptive_size.max(1).min(config.batch_size)
        } else {
            config.batch_size
        };
        
        // Collect events from priority queues (highest priority first)
        {
            let mut queues_guard = queues.write().await;
            let priorities = [FilePriority::Critical, FilePriority::High, FilePriority::Normal, FilePriority::Low];
            
            for priority in priorities {
                if let Some(queue) = queues_guard.get_mut(&priority) {
                    while batch.len() < effective_batch_size && !queue.is_empty() {
                        // Check rate limit
                        let can_process = {
                            let mut limiter = rate_limiter.write().await;
                            limiter.try_consume(1.0)
                        };
                        
                        if !can_process {
                            break;
                        }
                        
                        if let Some(event) = queue.pop_front() {
                            batch.push(event);
                        }
                    }
                }
                
                if batch.len() >= effective_batch_size {
                    break;
                }
            }
        }
        
        if !batch.is_empty() {
            // Send batch for processing
            if let Err(e) = batch_tx.send(batch.clone()).await {
                error!("Failed to send batch: {}", e);
                return Err(anyhow!("Failed to send batch: {}", e));
            }
            
            // Update statistics
            {
                let mut stats_guard = stats.write().await;
                stats_guard.batches_processed += 1;
                stats_guard.processed_files += batch.len() as u64;
                stats_guard.last_batch_size = batch.len();
                stats_guard.last_batch_time = Some(start_time);
                
                // Update throughput calculation
                let processing_time = start_time.elapsed();
                stats_guard.average_processing_time_ms = processing_time.as_millis() as f64;
                stats_guard.current_throughput = batch.len() as f64 / processing_time.as_secs_f64();
                
                // Update queue size
                let queues_guard = queues.read().await;
                stats_guard.queue_size = Self::calculate_total_queue_size_static(&queues_guard);
            }
            
            debug!("Processed batch of {} files in {:?}", batch.len(), start_time.elapsed());
        }
        
        Ok(())
    }
    
    /// Get total number of queued events across all priorities
    async fn get_total_queue_size(&self) -> usize {
        let queues = self.queues.read().await;
        self.calculate_total_queue_size(&queues)
    }
    
    /// Calculate total queue size from queues map
    fn calculate_total_queue_size(&self, queues: &HashMap<FilePriority, VecDeque<FileChangeEvent>>) -> usize {
        Self::calculate_total_queue_size_static(queues)
    }
    
    /// Static version of calculate_total_queue_size
    fn calculate_total_queue_size_static(queues: &HashMap<FilePriority, VecDeque<FileChangeEvent>>) -> usize {
        queues.values().map(|q| q.len()).sum()
    }
    
    /// Make room for a new event by dropping lower priority events
    async fn make_room_for_event(&self, event: &FileChangeEvent) -> bool {
        let mut queues = self.queues.write().await;
        
        // Try to drop events with lower priority
        let priorities_to_check = match event.priority {
            FilePriority::Critical => vec![FilePriority::Low, FilePriority::Normal, FilePriority::High],
            FilePriority::High => vec![FilePriority::Low, FilePriority::Normal],
            FilePriority::Normal => vec![FilePriority::Low],
            FilePriority::Low => return false, // Can't drop anything for low priority
        };
        
        for priority in priorities_to_check {
            if let Some(queue) = queues.get_mut(&priority) {
                if !queue.is_empty() {
                    queue.pop_front(); // Drop oldest event of this priority
                    return true;
                }
            }
        }
        
        false
    }
}

/// Helper function to determine file priority based on path and change type
pub fn determine_file_priority(path: &PathBuf, change_type: FileChangeType) -> FilePriority {
    let path_str = path.to_string_lossy().to_lowercase();
    
    // Critical files
    if path_str.contains("cargo.toml") 
        || path_str.contains("package.json")
        || path_str.contains("dockerfile")
        || path_str.contains(".env") {
        return FilePriority::Critical;
    }
    
    // High priority files
    if path_str.ends_with(".rs") 
        || path_str.ends_with(".ts") 
        || path_str.ends_with(".js")
        || path_str.ends_with(".py") {
        return FilePriority::High;
    }
    
    // Normal priority for configuration files
    if path_str.contains("config") 
        || path_str.ends_with(".toml")
        || path_str.ends_with(".yaml")
        || path_str.ends_with(".json") {
        return FilePriority::Normal;
    }
    
    // Low priority for everything else
    FilePriority::Low
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_backpressure_manager_creation() {
        let manager = BackpressureManager::new().await.unwrap();
        let stats = manager.get_stats().await;
        
        assert_eq!(stats.queue_size, 0);
        assert_eq!(stats.processed_files, 0);
        assert_eq!(stats.dropped_files, 0);
    }
    
    #[tokio::test]
    async fn test_file_priority_determination() {
        let cargo_toml = PathBuf::from("Cargo.toml");
        assert_eq!(determine_file_priority(&cargo_toml, FileChangeType::Modified), FilePriority::Critical);
        
        let rust_file = PathBuf::from("src/main.rs");
        assert_eq!(determine_file_priority(&rust_file, FileChangeType::Modified), FilePriority::High);
        
        let config_file = PathBuf::from("config.yaml");
        assert_eq!(determine_file_priority(&config_file, FileChangeType::Modified), FilePriority::Normal);
        
        let text_file = PathBuf::from("readme.txt");
        assert_eq!(determine_file_priority(&text_file, FileChangeType::Modified), FilePriority::Low);
    }
    
    #[tokio::test]
    async fn test_rate_limiter() {
        let mut limiter = RateLimiter::new(2.0); // 2 tokens per second
        
        // Should be able to consume initially
        assert!(limiter.try_consume(1.0));
        assert!(limiter.try_consume(1.0));
        
        // Should not be able to consume more
        assert!(!limiter.try_consume(1.0));
        
        // Wait and tokens should refill
        tokio::time::sleep(Duration::from_millis(600)).await;
        limiter.refill();
        assert!(limiter.try_consume(1.0));
    }
}
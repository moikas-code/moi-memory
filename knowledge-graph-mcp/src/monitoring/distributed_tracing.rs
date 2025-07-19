use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;
use tracing::{info, warn, error, debug, Span};
use uuid::Uuid;

/// Trace context for distributed tracing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceContext {
    pub trace_id: String,
    pub span_id: String,
    pub parent_span_id: Option<String>,
    pub baggage: HashMap<String, String>,
    pub trace_flags: u8,
}

impl TraceContext {
    pub fn new() -> Self {
        Self {
            trace_id: Uuid::new_v4().to_string(),
            span_id: Uuid::new_v4().to_string(),
            parent_span_id: None,
            baggage: HashMap::new(),
            trace_flags: 0,
        }
    }
    
    pub fn child(&self) -> Self {
        Self {
            trace_id: self.trace_id.clone(),
            span_id: Uuid::new_v4().to_string(),
            parent_span_id: Some(self.span_id.clone()),
            baggage: self.baggage.clone(),
            trace_flags: self.trace_flags,
        }
    }
    
    pub fn add_baggage(&mut self, key: String, value: String) {
        self.baggage.insert(key, value);
    }
}

/// Span status for tracking operation outcomes
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum SpanStatus {
    Ok,
    Error,
    Timeout,
    Cancelled,
}

/// Trace span for capturing operation details
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceSpan {
    pub trace_id: String,
    pub span_id: String,
    pub parent_span_id: Option<String>,
    pub operation_name: String,
    pub service_name: String,
    pub start_time: SystemTime,
    pub end_time: Option<SystemTime>,
    pub duration_ms: Option<u64>,
    pub status: SpanStatus,
    pub tags: HashMap<String, String>,
    pub logs: Vec<SpanLog>,
    pub process_id: String,
}

/// Span log entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpanLog {
    pub timestamp: SystemTime,
    pub level: String,
    pub message: String,
    pub fields: HashMap<String, String>,
}

/// Distributed tracing configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TracingConfig {
    /// Enable distributed tracing
    pub enabled: bool,
    
    /// Service name for this instance
    pub service_name: String,
    
    /// Sampling rate (0.0 to 1.0)
    pub sampling_rate: f64,
    
    /// Maximum spans to keep in memory
    pub max_spans_in_memory: usize,
    
    /// Span retention duration (seconds)
    pub span_retention_seconds: u64,
    
    /// Enable automatic span creation for operations
    pub auto_instrumentation: bool,
    
    /// Export traces to external systems
    pub enable_export: bool,
    
    /// Export endpoint URL
    pub export_endpoint: Option<String>,
    
    /// Export batch size
    pub export_batch_size: usize,
    
    /// Export timeout (seconds)
    pub export_timeout_seconds: u64,
}

impl Default for TracingConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            service_name: "knowledge-graph-mcp".to_string(),
            sampling_rate: 1.0, // Sample all traces in development
            max_spans_in_memory: 10000,
            span_retention_seconds: 3600, // 1 hour
            auto_instrumentation: true,
            enable_export: false,
            export_endpoint: None,
            export_batch_size: 100,
            export_timeout_seconds: 30,
        }
    }
}

/// Tracing statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TracingStats {
    pub total_spans: u64,
    pub active_spans: u64,
    pub completed_spans: u64,
    pub error_spans: u64,
    pub exported_spans: u64,
    pub sampling_decisions: u64,
    pub sampled_spans: u64,
    pub average_span_duration_ms: f64,
}

/// Active span tracker
#[derive(Debug)]
struct ActiveSpan {
    span: TraceSpan,
    start_instant: Instant,
}

/// Distributed tracing manager
pub struct TracingManager {
    config: TracingConfig,
    active_spans: Arc<RwLock<HashMap<String, ActiveSpan>>>,
    completed_spans: Arc<RwLock<Vec<TraceSpan>>>,
    stats: Arc<RwLock<TracingStats>>,
    process_id: String,
    
    // Export handling
    export_handle: Arc<tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>>,
}

impl TracingManager {
    pub async fn new(config: TracingConfig) -> Self {
        let process_id = format!("{}:{}", 
            gethostname::gethostname().to_string_lossy(),
            std::process::id()
        );
        
        Self {
            config,
            active_spans: Arc::new(RwLock::new(HashMap::new())),
            completed_spans: Arc::new(RwLock::new(Vec::new())),
            stats: Arc::new(RwLock::new(TracingStats {
                total_spans: 0,
                active_spans: 0,
                completed_spans: 0,
                error_spans: 0,
                exported_spans: 0,
                sampling_decisions: 0,
                sampled_spans: 0,
                average_span_duration_ms: 0.0,
            })),
            process_id,
            export_handle: Arc::new(tokio::sync::Mutex::new(None)),
        }
    }
    
    /// Start the tracing manager
    pub async fn start(&self) -> Result<()> {
        if !self.config.enabled {
            info!("Distributed tracing is disabled");
            return Ok(());
        }
        
        info!("Starting distributed tracing manager for service: {}", self.config.service_name);
        
        // Start background cleanup task
        self.start_cleanup_task().await;
        
        // Start export task if enabled
        if self.config.enable_export {
            self.start_export_task().await;
        }
        
        info!("Distributed tracing manager started");
        Ok(())
    }
    
    /// Stop the tracing manager
    pub async fn stop(&self) {
        if let Some(handle) = self.export_handle.lock().await.take() {
            handle.abort();
        }
        info!("Distributed tracing manager stopped");
    }
    
    /// Start a new trace span
    pub async fn start_span(
        &self,
        operation_name: String,
        context: Option<TraceContext>,
    ) -> Result<TraceContext> {
        if !self.config.enabled {
            return Ok(TraceContext::new());
        }
        
        // Sampling decision
        let mut stats = self.stats.write().await;
        stats.sampling_decisions += 1;
        
        if !self.should_sample() {
            return Ok(TraceContext::new());
        }
        
        stats.sampled_spans += 1;
        drop(stats);
        
        let trace_context = match context {
            Some(parent_context) => parent_context.child(),
            None => TraceContext::new(),
        };
        
        let span = TraceSpan {
            trace_id: trace_context.trace_id.clone(),
            span_id: trace_context.span_id.clone(),
            parent_span_id: trace_context.parent_span_id.clone(),
            operation_name,
            service_name: self.config.service_name.clone(),
            start_time: SystemTime::now(),
            end_time: None,
            duration_ms: None,
            status: SpanStatus::Ok,
            tags: HashMap::new(),
            logs: Vec::new(),
            process_id: self.process_id.clone(),
        };
        
        let active_span = ActiveSpan {
            span,
            start_instant: Instant::now(),
        };
        
        // Store active span
        {
            let mut active = self.active_spans.write().await;
            active.insert(trace_context.span_id.clone(), active_span);
        }
        
        // Update stats
        {
            let mut stats = self.stats.write().await;
            stats.total_spans += 1;
            stats.active_spans += 1;
        }
        
        debug!("Started span: {} ({})", trace_context.span_id, span.operation_name);
        
        Ok(trace_context)
    }
    
    /// Finish a trace span
    pub async fn finish_span(
        &self,
        context: &TraceContext,
        status: SpanStatus,
        tags: Option<HashMap<String, String>>,
    ) -> Result<()> {
        if !self.config.enabled {
            return Ok(());
        }
        
        let mut active_spans = self.active_spans.write().await;
        
        if let Some(active_span) = active_spans.remove(&context.span_id) {
            let duration = active_span.start_instant.elapsed();
            let duration_ms = duration.as_millis() as u64;
            
            let mut span = active_span.span;
            span.end_time = Some(SystemTime::now());
            span.duration_ms = Some(duration_ms);
            span.status = status.clone();
            
            if let Some(tags) = tags {
                span.tags.extend(tags);
            }
            
            // Add to completed spans
            {
                let mut completed = self.completed_spans.write().await;
                completed.push(span);
                
                // Limit memory usage
                if completed.len() > self.config.max_spans_in_memory {
                    completed.drain(0..completed.len() - self.config.max_spans_in_memory);
                }
            }
            
            // Update stats
            {
                let mut stats = self.stats.write().await;
                stats.active_spans = stats.active_spans.saturating_sub(1);
                stats.completed_spans += 1;
                
                if status == SpanStatus::Error {
                    stats.error_spans += 1;
                }
                
                // Update average duration
                let total_completed = stats.completed_spans as f64;
                stats.average_span_duration_ms = 
                    (stats.average_span_duration_ms * (total_completed - 1.0) + duration_ms as f64) / total_completed;
            }
            
            debug!("Finished span: {} in {}ms ({})", context.span_id, duration_ms, status.to_string());
        }
        
        Ok(())
    }
    
    /// Add a log entry to an active span
    pub async fn log_to_span(
        &self,
        context: &TraceContext,
        level: String,
        message: String,
        fields: Option<HashMap<String, String>>,
    ) -> Result<()> {
        if !self.config.enabled {
            return Ok(());
        }
        
        let mut active_spans = self.active_spans.write().await;
        
        if let Some(active_span) = active_spans.get_mut(&context.span_id) {
            let log_entry = SpanLog {
                timestamp: SystemTime::now(),
                level,
                message,
                fields: fields.unwrap_or_default(),
            };
            
            active_span.span.logs.push(log_entry);
        }
        
        Ok(())
    }
    
    /// Add tags to an active span
    pub async fn add_span_tags(
        &self,
        context: &TraceContext,
        tags: HashMap<String, String>,
    ) -> Result<()> {
        if !self.config.enabled {
            return Ok(());
        }
        
        let mut active_spans = self.active_spans.write().await;
        
        if let Some(active_span) = active_spans.get_mut(&context.span_id) {
            active_span.span.tags.extend(tags);
        }
        
        Ok(())
    }
    
    /// Get spans for a specific trace
    pub async fn get_trace_spans(&self, trace_id: &str) -> Vec<TraceSpan> {
        let mut spans = Vec::new();
        
        // Check active spans
        {
            let active = self.active_spans.read().await;
            for active_span in active.values() {
                if active_span.span.trace_id == trace_id {
                    spans.push(active_span.span.clone());
                }
            }
        }
        
        // Check completed spans
        {
            let completed = self.completed_spans.read().await;
            for span in completed.iter() {
                if span.trace_id == trace_id {
                    spans.push(span.clone());
                }
            }
        }
        
        // Sort by start time
        spans.sort_by(|a, b| a.start_time.cmp(&b.start_time));
        spans
    }
    
    /// Get recent traces
    pub async fn get_recent_traces(&self, limit: usize) -> Vec<String> {
        let completed = self.completed_spans.read().await;
        let mut trace_ids: Vec<String> = completed
            .iter()
            .rev()
            .take(limit * 10) // Get more spans to find unique traces
            .map(|span| span.trace_id.clone())
            .collect();
        
        trace_ids.sort();
        trace_ids.dedup();
        trace_ids.truncate(limit);
        trace_ids
    }
    
    /// Get tracing statistics
    pub async fn get_stats(&self) -> TracingStats {
        self.stats.read().await.clone()
    }
    
    /// Create a span context from headers (for HTTP requests)
    pub fn extract_context_from_headers(&self, headers: &HashMap<String, String>) -> Option<TraceContext> {
        // Basic implementation - would follow OpenTelemetry or Jaeger format
        if let (Some(trace_id), Some(span_id)) = (
            headers.get("x-trace-id"),
            headers.get("x-span-id")
        ) {
            Some(TraceContext {
                trace_id: trace_id.clone(),
                span_id: span_id.clone(),
                parent_span_id: headers.get("x-parent-span-id").cloned(),
                baggage: HashMap::new(),
                trace_flags: 0,
            })
        } else {
            None
        }
    }
    
    /// Inject context into headers (for HTTP requests)
    pub fn inject_context_into_headers(&self, context: &TraceContext) -> HashMap<String, String> {
        let mut headers = HashMap::new();
        headers.insert("x-trace-id".to_string(), context.trace_id.clone());
        headers.insert("x-span-id".to_string(), context.span_id.clone());
        
        if let Some(parent_id) = &context.parent_span_id {
            headers.insert("x-parent-span-id".to_string(), parent_id.clone());
        }
        
        headers
    }
    
    // Private methods
    
    fn should_sample(&self) -> bool {
        use rand::Rng;
        let mut rng = rand::thread_rng();
        rng.gen::<f64>() < self.config.sampling_rate
    }
    
    async fn start_cleanup_task(&self) {
        let completed_spans = self.completed_spans.clone();
        let retention_seconds = self.config.span_retention_seconds;
        
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(300)); // 5 minutes
            
            loop {
                interval.tick().await;
                
                let cutoff = SystemTime::now() - Duration::from_secs(retention_seconds);
                let mut completed = completed_spans.write().await;
                
                let initial_count = completed.len();
                completed.retain(|span| span.start_time > cutoff);
                let removed_count = initial_count - completed.len();
                
                if removed_count > 0 {
                    debug!("Cleaned up {} expired spans", removed_count);
                }
            }
        });
    }
    
    async fn start_export_task(&self) {
        if self.config.export_endpoint.is_none() {
            return;
        }
        
        let completed_spans = self.completed_spans.clone();
        let stats = self.stats.clone();
        let config = self.config.clone();
        
        let handle = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(30)); // Export every 30 seconds
            
            loop {
                interval.tick().await;
                
                // Collect spans for export
                let spans_to_export: Vec<TraceSpan> = {
                    let mut completed = completed_spans.write().await;
                    if completed.len() >= config.export_batch_size {
                        completed.drain(0..config.export_batch_size).collect()
                    } else {
                        Vec::new()
                    }
                };
                
                if !spans_to_export.is_empty() {
                    // Export spans (placeholder implementation)
                    match Self::export_spans(&config, &spans_to_export).await {
                        Ok(exported_count) => {
                            let mut stats = stats.write().await;
                            stats.exported_spans += exported_count;
                            debug!("Exported {} spans", exported_count);
                        }
                        Err(e) => {
                            error!("Failed to export spans: {}", e);
                            // Put spans back for retry
                            let mut completed = completed_spans.write().await;
                            for span in spans_to_export.into_iter().rev() {
                                completed.insert(0, span);
                            }
                        }
                    }
                }
            }
        });
        
        *self.export_handle.lock().await = Some(handle);
    }
    
    async fn export_spans(config: &TracingConfig, spans: &[TraceSpan]) -> Result<u64> {
        // Placeholder implementation for span export
        // In a real implementation, this would send spans to Jaeger, Zipkin, or OpenTelemetry collector
        
        if let Some(_endpoint) = &config.export_endpoint {
            // Simulate export
            tokio::time::sleep(Duration::from_millis(100)).await;
            Ok(spans.len() as u64)
        } else {
            Err(anyhow!("No export endpoint configured"))
        }
    }
}

impl Drop for TracingManager {
    fn drop(&mut self) {
        if let Ok(mut handle) = self.export_handle.try_lock() {
            if let Some(task) = handle.take() {
                task.abort();
            }
        }
    }
}

impl std::fmt::Display for SpanStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpanStatus::Ok => write!(f, "ok"),
            SpanStatus::Error => write!(f, "error"),
            SpanStatus::Timeout => write!(f, "timeout"),
            SpanStatus::Cancelled => write!(f, "cancelled"),
        }
    }
}
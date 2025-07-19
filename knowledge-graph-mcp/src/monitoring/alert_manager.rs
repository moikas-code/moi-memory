use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{RwLock, mpsc};
use tokio::time::interval;
use tracing::{info, warn, error, debug};

use super::health_monitor::{HealthStatus, SystemHealth, ComponentHealth};

/// Alert severity levels
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum AlertSeverity {
    Info,
    Warning,
    Critical,
    Emergency,
}

impl std::fmt::Display for AlertSeverity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AlertSeverity::Info => write!(f, "info"),
            AlertSeverity::Warning => write!(f, "warning"),
            AlertSeverity::Critical => write!(f, "critical"),
            AlertSeverity::Emergency => write!(f, "emergency"),
        }
    }
}

/// Alert notification channels
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AlertChannel {
    Log,
    Webhook { url: String, headers: HashMap<String, String> },
    Slack { webhook_url: String, channel: String },
    Email { to: Vec<String>, smtp_config: EmailConfig },
    Custom { name: String, config: serde_json::Value },
}

/// Email configuration for alerts
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmailConfig {
    pub smtp_host: String,
    pub smtp_port: u16,
    pub username: String,
    pub password: String,
    pub from_address: String,
    pub use_tls: bool,
}

/// Alert condition for triggering rules
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AlertCondition {
    HealthStatusEquals { component: String, status: HealthStatus },
    HealthStatusNot { component: String, status: HealthStatus },
    ResponseTimeAbove { component: String, threshold_ms: u64 },
    MetricAbove { component: String, metric: String, threshold: f64 },
    MetricBelow { component: String, metric: String, threshold: f64 },
    ConsecutiveFailures { component: String, count: u32 },
    SystemUptimeBelow { threshold_seconds: u64 },
    PerformanceScoreBelow { threshold: f64 },
}

/// Alert rule configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertRule {
    pub id: String,
    pub name: String,
    pub description: String,
    pub condition: AlertCondition,
    pub severity: AlertSeverity,
    pub enabled: bool,
    pub cooldown_seconds: u64,
    pub channels: Vec<AlertChannel>,
    pub auto_resolve: bool,
    pub resolve_condition: Option<AlertCondition>,
}

/// Active alert instance
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Alert {
    pub id: String,
    pub rule_id: String,
    pub severity: AlertSeverity,
    pub title: String,
    pub description: String,
    pub triggered_at: Instant,
    pub resolved_at: Option<Instant>,
    pub component: Option<String>,
    pub metrics: HashMap<String, serde_json::Value>,
    pub notification_count: u32,
    pub last_notification: Option<Instant>,
}

/// Alert manager configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertManagerConfig {
    /// Enable alert processing
    pub enabled: bool,
    
    /// Alert evaluation interval (seconds)
    pub evaluation_interval_seconds: u64,
    
    /// Maximum active alerts to track
    pub max_active_alerts: usize,
    
    /// Alert history retention (hours)
    pub history_retention_hours: u64,
    
    /// Default notification cooldown (seconds)
    pub default_cooldown_seconds: u64,
    
    /// Maximum notifications per alert
    pub max_notifications_per_alert: u32,
    
    /// Enable alert grouping by component
    pub enable_grouping: bool,
    
    /// Group alerts within this time window (seconds)
    pub grouping_window_seconds: u64,
}

impl Default for AlertManagerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            evaluation_interval_seconds: 30,
            max_active_alerts: 1000,
            history_retention_hours: 168, // 7 days
            default_cooldown_seconds: 300, // 5 minutes
            max_notifications_per_alert: 10,
            enable_grouping: true,
            grouping_window_seconds: 300, // 5 minutes
        }
    }
}

/// Alert statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertStats {
    pub total_alerts: u64,
    pub active_alerts: u64,
    pub resolved_alerts: u64,
    pub notifications_sent: u64,
    pub failed_notifications: u64,
    pub rules_evaluated: u64,
    pub evaluation_errors: u64,
}

/// Comprehensive alert management system
pub struct AlertManager {
    config: AlertManagerConfig,
    rules: Arc<RwLock<HashMap<String, AlertRule>>>,
    active_alerts: Arc<RwLock<HashMap<String, Alert>>>,
    alert_history: Arc<RwLock<Vec<Alert>>>,
    rule_cooldowns: Arc<RwLock<HashMap<String, Instant>>>,
    stats: Arc<RwLock<AlertStats>>,
    
    // Communication channels
    alert_sender: mpsc::Sender<Alert>,
    alert_receiver: Arc<tokio::sync::Mutex<mpsc::Receiver<Alert>>>,
    
    // Background tasks
    manager_handle: Arc<tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>>,
    processor_handle: Arc<tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>>,
}

impl AlertManager {
    pub async fn new(config: AlertManagerConfig) -> Self {
        let (alert_sender, alert_receiver) = mpsc::channel(1000);
        
        Self {
            config,
            rules: Arc::new(RwLock::new(HashMap::new())),
            active_alerts: Arc::new(RwLock::new(HashMap::new())),
            alert_history: Arc::new(RwLock::new(Vec::new())),
            rule_cooldowns: Arc::new(RwLock::new(HashMap::new())),
            stats: Arc::new(RwLock::new(AlertStats {
                total_alerts: 0,
                active_alerts: 0,
                resolved_alerts: 0,
                notifications_sent: 0,
                failed_notifications: 0,
                rules_evaluated: 0,
                evaluation_errors: 0,
            })),
            alert_sender,
            alert_receiver: Arc::new(tokio::sync::Mutex::new(alert_receiver)),
            manager_handle: Arc::new(tokio::sync::Mutex::new(None)),
            processor_handle: Arc::new(tokio::sync::Mutex::new(None)),
        }
    }
    
    /// Start the alert management system
    pub async fn start(&self) -> Result<()> {
        if !self.config.enabled {
            info!("Alert manager is disabled");
            return Ok(());
        }
        
        info!("Starting alert management system...");
        
        // Start alert processor task
        let processor_handle = self.start_alert_processor().await;
        *self.processor_handle.lock().await = Some(processor_handle);
        
        info!("Alert management system started");
        Ok(())
    }
    
    /// Stop the alert management system
    pub async fn stop(&self) {
        if let Some(handle) = self.manager_handle.lock().await.take() {
            handle.abort();
        }
        if let Some(handle) = self.processor_handle.lock().await.take() {
            handle.abort();
        }
        info!("Alert management system stopped");
    }
    
    /// Add an alert rule
    pub async fn add_rule(&self, rule: AlertRule) {
        info!("Adding alert rule: {} ({})", rule.name, rule.id);
        self.rules.write().await.insert(rule.id.clone(), rule);
    }
    
    /// Remove an alert rule
    pub async fn remove_rule(&self, rule_id: &str) -> bool {
        info!("Removing alert rule: {}", rule_id);
        self.rules.write().await.remove(rule_id).is_some()
    }
    
    /// Update an alert rule
    pub async fn update_rule(&self, rule: AlertRule) -> bool {
        if self.rules.read().await.contains_key(&rule.id) {
            info!("Updating alert rule: {} ({})", rule.name, rule.id);
            self.rules.write().await.insert(rule.id.clone(), rule);
            true
        } else {
            false
        }
    }
    
    /// Get all alert rules
    pub async fn get_rules(&self) -> Vec<AlertRule> {
        self.rules.read().await.values().cloned().collect()
    }
    
    /// Get active alerts
    pub async fn get_active_alerts(&self) -> Vec<Alert> {
        self.active_alerts.read().await.values().cloned().collect()
    }
    
    /// Get alert history
    pub async fn get_alert_history(&self, limit: Option<usize>) -> Vec<Alert> {
        let history = self.alert_history.read().await;
        match limit {
            Some(n) => history.iter().rev().take(n).cloned().collect(),
            None => history.clone(),
        }
    }
    
    /// Get alert statistics
    pub async fn get_stats(&self) -> AlertStats {
        self.stats.read().await.clone()
    }
    
    /// Evaluate health status against alert rules
    pub async fn evaluate_health(&self, health: &SystemHealth) -> Result<()> {
        if !self.config.enabled {
            return Ok(());
        }
        
        let rules = self.rules.read().await.clone();
        let mut stats = self.stats.write().await;
        
        for (rule_id, rule) in &rules {
            if !rule.enabled {
                continue;
            }
            
            stats.rules_evaluated += 1;
            
            // Check rule cooldown
            if let Some(last_trigger) = self.rule_cooldowns.read().await.get(rule_id) {
                if last_trigger.elapsed().as_secs() < rule.cooldown_seconds {
                    continue;
                }
            }
            
            // Evaluate rule condition
            match self.evaluate_condition(&rule.condition, health).await {
                Ok(should_trigger) => {
                    if should_trigger {
                        self.trigger_alert(rule, health).await?;
                    } else if rule.auto_resolve {
                        // Check resolve condition
                        if let Some(resolve_condition) = &rule.resolve_condition {
                            if self.evaluate_condition(resolve_condition, health).await? {
                                self.resolve_alerts_for_rule(rule_id).await;
                            }
                        }
                    }
                }
                Err(e) => {
                    stats.evaluation_errors += 1;
                    error!("Failed to evaluate alert rule {}: {}", rule_id, e);
                }
            }
        }
        
        Ok(())
    }
    
    /// Manually trigger an alert
    pub async fn trigger_manual_alert(
        &self,
        title: String,
        description: String,
        severity: AlertSeverity,
        component: Option<String>,
    ) -> Result<()> {
        let alert = Alert {
            id: uuid::Uuid::new_v4().to_string(),
            rule_id: "manual".to_string(),
            severity,
            title,
            description,
            triggered_at: Instant::now(),
            resolved_at: None,
            component,
            metrics: HashMap::new(),
            notification_count: 0,
            last_notification: None,
        };
        
        self.alert_sender.send(alert).await.map_err(|e| anyhow!("Failed to send alert: {}", e))
    }
    
    /// Resolve an active alert
    pub async fn resolve_alert(&self, alert_id: &str) -> bool {
        let mut active_alerts = self.active_alerts.write().await;
        if let Some(mut alert) = active_alerts.remove(alert_id) {
            alert.resolved_at = Some(Instant::now());
            
            // Move to history
            self.alert_history.write().await.push(alert);
            
            // Update stats
            let mut stats = self.stats.write().await;
            stats.active_alerts = stats.active_alerts.saturating_sub(1);
            stats.resolved_alerts += 1;
            
            info!("Resolved alert: {}", alert_id);
            true
        } else {
            false
        }
    }
    
    // Private methods
    
    async fn evaluate_condition(&self, condition: &AlertCondition, health: &SystemHealth) -> Result<bool> {
        match condition {
            AlertCondition::HealthStatusEquals { component, status } => {
                if let Some(comp_health) = health.components.get(component) {
                    Ok(comp_health.status == *status)
                } else {
                    Ok(false)
                }
            }
            
            AlertCondition::HealthStatusNot { component, status } => {
                if let Some(comp_health) = health.components.get(component) {
                    Ok(comp_health.status != *status)
                } else {
                    Ok(true) // Component not found is considered "not healthy"
                }
            }
            
            AlertCondition::ResponseTimeAbove { component, threshold_ms } => {
                if let Some(comp_health) = health.components.get(component) {
                    Ok(comp_health.response_time_ms > *threshold_ms)
                } else {
                    Ok(false)
                }
            }
            
            AlertCondition::MetricAbove { component, metric, threshold } => {
                if let Some(comp_health) = health.components.get(component) {
                    if let Some(value) = comp_health.metrics.get(metric) {
                        if let Some(num_value) = value.as_f64() {
                            return Ok(num_value > *threshold);
                        }
                    }
                }
                Ok(false)
            }
            
            AlertCondition::MetricBelow { component, metric, threshold } => {
                if let Some(comp_health) = health.components.get(component) {
                    if let Some(value) = comp_health.metrics.get(metric) {
                        if let Some(num_value) = value.as_f64() {
                            return Ok(num_value < *threshold);
                        }
                    }
                }
                Ok(false)
            }
            
            AlertCondition::SystemUptimeBelow { threshold_seconds } => {
                Ok(health.uptime_seconds < *threshold_seconds)
            }
            
            AlertCondition::PerformanceScoreBelow { threshold } => {
                Ok(health.performance_score < *threshold)
            }
            
            AlertCondition::ConsecutiveFailures { component: _, count: _ } => {
                // This would require failure tracking from health monitor
                Ok(false) // Placeholder implementation
            }
        }
    }
    
    async fn trigger_alert(&self, rule: &AlertRule, health: &SystemHealth) -> Result<()> {
        let alert_id = uuid::Uuid::new_v4().to_string();
        
        // Extract relevant metrics based on condition
        let mut metrics = HashMap::new();
        let component_name = self.extract_component_from_condition(&rule.condition);
        
        if let Some(comp_name) = &component_name {
            if let Some(comp_health) = health.components.get(comp_name) {
                metrics.insert("response_time_ms".to_string(), 
                    serde_json::Value::Number(comp_health.response_time_ms.into()));
                metrics.insert("status".to_string(), 
                    serde_json::Value::String(comp_health.status.to_string()));
                
                // Include component-specific metrics
                for (key, value) in &comp_health.metrics {
                    metrics.insert(key.clone(), value.clone());
                }
            }
        }
        
        let alert = Alert {
            id: alert_id.clone(),
            rule_id: rule.id.clone(),
            severity: rule.severity.clone(),
            title: rule.name.clone(),
            description: rule.description.clone(),
            triggered_at: Instant::now(),
            resolved_at: None,
            component: component_name,
            metrics,
            notification_count: 0,
            last_notification: None,
        };
        
        // Send alert for processing
        self.alert_sender.send(alert).await.map_err(|e| anyhow!("Failed to send alert: {}", e))?;
        
        // Update rule cooldown
        self.rule_cooldowns.write().await.insert(rule.id.clone(), Instant::now());
        
        Ok(())
    }
    
    async fn resolve_alerts_for_rule(&self, rule_id: &str) {
        let mut active_alerts = self.active_alerts.write().await;
        let mut history = self.alert_history.write().await;
        let mut stats = self.stats.write().await;
        
        let mut resolved_count = 0;
        let alerts_to_resolve: Vec<String> = active_alerts
            .iter()
            .filter(|(_, alert)| alert.rule_id == rule_id)
            .map(|(id, _)| id.clone())
            .collect();
        
        for alert_id in alerts_to_resolve {
            if let Some(mut alert) = active_alerts.remove(&alert_id) {
                alert.resolved_at = Some(Instant::now());
                history.push(alert);
                resolved_count += 1;
            }
        }
        
        if resolved_count > 0 {
            stats.active_alerts = stats.active_alerts.saturating_sub(resolved_count);
            stats.resolved_alerts += resolved_count;
            info!("Auto-resolved {} alerts for rule: {}", resolved_count, rule_id);
        }
    }
    
    fn extract_component_from_condition(&self, condition: &AlertCondition) -> Option<String> {
        match condition {
            AlertCondition::HealthStatusEquals { component, .. } |
            AlertCondition::HealthStatusNot { component, .. } |
            AlertCondition::ResponseTimeAbove { component, .. } |
            AlertCondition::MetricAbove { component, .. } |
            AlertCondition::MetricBelow { component, .. } |
            AlertCondition::ConsecutiveFailures { component, .. } => Some(component.clone()),
            _ => None,
        }
    }
    
    async fn start_alert_processor(&self) -> tokio::task::JoinHandle<()> {
        let active_alerts = self.active_alerts.clone();
        let alert_history = self.alert_history.clone();
        let stats = self.stats.clone();
        let mut receiver = self.alert_receiver.lock().await;
        let config = self.config.clone();
        
        tokio::spawn(async move {
            while let Some(alert) = receiver.recv().await {
                // Process the alert
                let alert_id = alert.id.clone();
                
                // Add to active alerts
                {
                    let mut active = active_alerts.write().await;
                    active.insert(alert_id.clone(), alert.clone());
                }
                
                // Update stats
                {
                    let mut stats = stats.write().await;
                    stats.total_alerts += 1;
                    stats.active_alerts += 1;
                }
                
                // Send notifications (would implement actual notification logic here)
                info!("Alert triggered: {} - {} ({})", alert.title, alert.description, alert.severity);
                
                // Clean up old history entries
                {
                    let mut history = alert_history.write().await;
                    let cutoff = Instant::now() - Duration::from_secs(config.history_retention_hours * 3600);
                    history.retain(|a| a.triggered_at > cutoff);
                }
            }
        })
    }
}

impl Drop for AlertManager {
    fn drop(&mut self) {
        if let Ok(mut handle) = self.manager_handle.try_lock() {
            if let Some(task) = handle.take() {
                task.abort();
            }
        }
        if let Ok(mut handle) = self.processor_handle.try_lock() {
            if let Some(task) = handle.take() {
                task.abort();
            }
        }
    }
}
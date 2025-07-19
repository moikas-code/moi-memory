use crate::config::Neo4jConfig;
use crate::errors::Result;
use crate::retry::{RetryStrategy, RetryableError};
use neo4rs::{Graph, ConfigBuilder};
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::time::{Duration, Instant, interval};
use tracing::{info, warn, error, debug};

#[derive(Debug, Clone)]
pub struct ConnectionStats {
    pub total_connections: u32,
    pub active_connections: u32,
    pub failed_connections: u32,
    pub last_connection_at: Option<Instant>,
    pub last_failure_at: Option<Instant>,
    pub reconnection_attempts: u32,
}

impl Default for ConnectionStats {
    fn default() -> Self {
        Self {
            total_connections: 0,
            active_connections: 0,
            failed_connections: 0,
            last_connection_at: None,
            last_failure_at: None,
            reconnection_attempts: 0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PoolConfig {
    pub max_connections: u32,
    pub min_connections: u32,
    pub connection_timeout: Duration,
    pub health_check_interval: Duration,
    pub reconnect_delay: Duration,
    pub max_reconnect_attempts: u32,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            max_connections: 10,
            min_connections: 2,
            connection_timeout: Duration::from_secs(30),
            health_check_interval: Duration::from_secs(60),
            reconnect_delay: Duration::from_secs(5),
            max_reconnect_attempts: 10,
        }
    }
}

pub struct ConnectionPool {
    neo4j_config: Neo4jConfig,
    pool_config: PoolConfig,
    connections: Arc<RwLock<Vec<Arc<Graph>>>>,
    stats: Arc<RwLock<ConnectionStats>>,
    retry_strategy: RetryStrategy,
    is_healthy: Arc<RwLock<bool>>,
}

impl ConnectionPool {
    pub fn new(neo4j_config: Neo4jConfig, pool_config: PoolConfig) -> Self {
        Self {
            neo4j_config,
            pool_config,
            connections: Arc::new(RwLock::new(Vec::new())),
            stats: Arc::new(RwLock::new(ConnectionStats::default())),
            retry_strategy: RetryStrategy::standard(),
            is_healthy: Arc::new(RwLock::new(false)),
        }
    }

    pub async fn initialize(&self) -> Result<()> {
        info!("Initializing connection pool with {} min connections", self.pool_config.min_connections);
        
        for _ in 0..self.pool_config.min_connections {
            match self.create_connection().await {
                Ok(connection) => {
                    self.connections.write().await.push(Arc::new(connection));
                    self.update_stats(|stats| {
                        stats.total_connections += 1;
                        stats.active_connections += 1;
                        stats.last_connection_at = Some(Instant::now());
                    }).await;
                }
                Err(e) => {
                    warn!("Failed to create initial connection: {}", e);
                    self.update_stats(|stats| {
                        stats.failed_connections += 1;
                        stats.last_failure_at = Some(Instant::now());
                    }).await;
                }
            }
        }

        if self.connections.read().await.len() > 0 {
            *self.is_healthy.write().await = true;
        }

        // Start health check background task
        self.start_health_check().await;
        
        Ok(())
    }

    async fn create_connection(&self) -> Result<Graph> {
        let config = ConfigBuilder::default()
            .uri(&self.neo4j_config.uri)
            .user(&self.neo4j_config.username)
            .password(&self.neo4j_config.password)
            .db(&self.neo4j_config.database)
            .build()
            .map_err(|e| crate::errors::KnowledgeGraphError::Database(format!("Neo4j config error: {}", e)))?;

        let graph = Graph::connect(config).await
            .map_err(|e| crate::errors::KnowledgeGraphError::Database(format!("Neo4j connection error: {}", e)))?;

        // Test the connection
        let mut result = graph.execute(neo4rs::query("RETURN 1")).await
            .map_err(|e| crate::errors::KnowledgeGraphError::Database(format!("Neo4j test query error: {}", e)))?;

        if result.next().await.is_err() {
            return Err(crate::errors::KnowledgeGraphError::Database(
                "Failed to validate Neo4j connection".to_string()
            ));
        }

        debug!("Successfully created new Neo4j connection");
        Ok(graph)
    }

    pub async fn get_connection(&self) -> Result<Arc<Graph>> {
        let connections = self.connections.read().await;
        
        if connections.is_empty() {
            drop(connections);
            warn!("No available connections, attempting to create one");
            return self.create_and_add_connection().await;
        }

        // Return the first available connection
        // In a more sophisticated implementation, we might track connection usage
        let connection = connections[0].clone();
        Ok(connection)
    }

    async fn create_and_add_connection(&self) -> Result<Arc<Graph>> {
        let connection = self.create_connection().await?;
        let arc_connection = Arc::new(connection);
        
        self.connections.write().await.push(arc_connection.clone());
        self.update_stats(|stats| {
            stats.total_connections += 1;
            stats.active_connections += 1;
            stats.last_connection_at = Some(Instant::now());
        }).await;

        *self.is_healthy.write().await = true;
        info!("Successfully created and added new connection to pool");
        
        Ok(arc_connection)
    }

    pub async fn remove_failed_connection(&self, failed_connection: &Arc<Graph>) {
        let mut connections = self.connections.write().await;
        connections.retain(|conn| !Arc::ptr_eq(conn, failed_connection));
        
        self.update_stats(|stats| {
            stats.active_connections = stats.active_connections.saturating_sub(1);
            stats.failed_connections += 1;
            stats.last_failure_at = Some(Instant::now());
        }).await;

        if connections.is_empty() {
            *self.is_healthy.write().await = false;
            warn!("Connection pool is now empty - marking as unhealthy");
        }

        debug!("Removed failed connection from pool");
    }

    async fn start_health_check(&self) {
        let connections = self.connections.clone();
        let stats = self.stats.clone();
        let is_healthy = self.is_healthy.clone();
        let pool_config = self.pool_config.clone();
        let neo4j_config = self.neo4j_config.clone();

        tokio::spawn(async move {
            let mut interval = interval(pool_config.health_check_interval);
            
            loop {
                interval.tick().await;
                
                let mut reconnect_needed = false;
                {
                    let connections_guard = connections.read().await;
                    if connections_guard.len() < pool_config.min_connections as usize {
                        reconnect_needed = true;
                    }
                }

                if reconnect_needed {
                    debug!("Health check detected need for reconnection");
                    
                    // Attempt to create new connections up to min_connections
                    let current_count = connections.read().await.len();
                    let needed = pool_config.min_connections as usize - current_count;
                    
                    for _ in 0..needed {
                        match Self::create_connection_static(&neo4j_config).await {
                            Ok(connection) => {
                                connections.write().await.push(Arc::new(connection));
                                Self::update_stats_static(&stats, |s| {
                                    s.total_connections += 1;
                                    s.active_connections += 1;
                                    s.last_connection_at = Some(Instant::now());
                                    s.reconnection_attempts += 1;
                                }).await;
                                
                                *is_healthy.write().await = true;
                                info!("Successfully reconnected to Neo4j");
                            }
                            Err(e) => {
                                Self::update_stats_static(&stats, |s| {
                                    s.failed_connections += 1;
                                    s.last_failure_at = Some(Instant::now());
                                    s.reconnection_attempts += 1;
                                }).await;
                                
                                warn!("Failed to reconnect to Neo4j: {}", e);
                                
                                // Wait before next attempt
                                tokio::time::sleep(pool_config.reconnect_delay).await;
                            }
                        }
                    }
                }
            }
        });
    }

    async fn create_connection_static(neo4j_config: &Neo4jConfig) -> Result<Graph> {
        let config = ConfigBuilder::default()
            .uri(&neo4j_config.uri)
            .user(&neo4j_config.username)
            .password(&neo4j_config.password)
            .db(&neo4j_config.database)
            .build()
            .map_err(|e| crate::errors::KnowledgeGraphError::Database(format!("Neo4j config error: {}", e)))?;

        let graph = Graph::connect(config).await
            .map_err(|e| crate::errors::KnowledgeGraphError::Database(format!("Neo4j connection error: {}", e)))?;

        // Test the connection
        let mut result = graph.execute(neo4rs::query("RETURN 1")).await
            .map_err(|e| crate::errors::KnowledgeGraphError::Database(format!("Neo4j test query error: {}", e)))?;

        if result.next().await.is_err() {
            return Err(crate::errors::KnowledgeGraphError::Database(
                "Failed to validate Neo4j connection".to_string()
            ));
        }

        Ok(graph)
    }

    async fn update_stats<F>(&self, update_fn: F) 
    where
        F: FnOnce(&mut ConnectionStats),
    {
        let mut stats = self.stats.write().await;
        update_fn(&mut *stats);
    }

    async fn update_stats_static<F>(stats: &Arc<RwLock<ConnectionStats>>, update_fn: F) 
    where
        F: FnOnce(&mut ConnectionStats),
    {
        let mut stats_guard = stats.write().await;
        update_fn(&mut *stats_guard);
    }

    pub async fn get_stats(&self) -> ConnectionStats {
        self.stats.read().await.clone()
    }

    pub async fn is_healthy(&self) -> bool {
        *self.is_healthy.read().await
    }

    pub async fn with_retry<F, T>(&self, operation: F) -> Result<T>
    where
        F: Fn(Arc<Graph>) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<T>> + Send + '_>> + Send + Sync,
        T: Send,
    {
        self.retry_strategy.retry_async(|| async {
            let connection = self.get_connection().await?;
            
            match operation(connection.clone()).await {
                Ok(result) => Ok(result),
                Err(e) => {
                    // Check if this is a connection-related error
                    if Self::is_connection_error(&e) {
                        self.remove_failed_connection(&connection).await;
                    }
                    
                    // Determine if error is retryable
                    if Self::is_retryable_error(&e) {
                        return Err(RetryableError::Retryable(e));
                    } else {
                        return Err(RetryableError::NonRetryable(e));
                    }
                }
            }
        }).await
    }

    fn is_connection_error(error: &crate::errors::KnowledgeGraphError) -> bool {
        match error {
            crate::errors::KnowledgeGraphError::Database(msg) => {
                msg.contains("connection") || 
                msg.contains("timeout") || 
                msg.contains("disconnected") ||
                msg.contains("refused")
            }
            _ => false
        }
    }

    fn is_retryable_error(error: &crate::errors::KnowledgeGraphError) -> bool {
        match error {
            crate::errors::KnowledgeGraphError::Database(msg) => {
                msg.contains("timeout") || 
                msg.contains("temporary") ||
                msg.contains("connection") ||
                msg.contains("unavailable")
            }
            _ => false
        }
    }
}

impl Drop for ConnectionPool {
    fn drop(&mut self) {
        debug!("Connection pool is being dropped");
    }
}
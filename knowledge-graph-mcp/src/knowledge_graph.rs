use anyhow::{Result, anyhow};
use neo4rs::{Graph, Node, Relation, query};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn, error, debug};
use uuid::Uuid;

use crate::query_templates;
use crate::nlp_patterns::enhance_natural_language_query;
use crate::metrics::{MetricsCollector, QueryTimer};
use crate::circuit_breaker::{CircuitBreaker, CircuitBreakerConfig};
use crate::connection_pool::{ConnectionPool, PoolConfig};
use crate::timeout_manager::{TimeoutManager, TimeoutConfig, OperationType};
use crate::performance::{EntityLRUCache, EntityCacheConfig};
use crate::config::Neo4jConfig;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeEntity {
    pub id: String,
    pub name: String,
    pub entity_type: String, // Changed from enum to string for compatibility
    pub file_path: Option<String>,
    pub content: Option<String>,
    pub metadata: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityType {
    File,
    Module,
    Class,
    Function,
    Method,
    Variable,
    Import,
    Type,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RelationshipType {
    Contains,
    Calls,
    Imports,
    Inherits,
    Implements,
    DependsOn,
    References,
    Defines,
    Uses,
}

pub struct KnowledgeGraph {
    connection_pool: Arc<ConnectionPool>,
    initialized: Arc<RwLock<bool>>,
    metrics: Arc<MetricsCollector>,
    circuit_breaker: Arc<CircuitBreaker>,
    timeout_manager: Arc<TimeoutManager>,
    entity_cache: Arc<EntityLRUCache>,
}

impl KnowledgeGraph {
    pub async fn new(neo4j_config: Neo4jConfig, metrics: Arc<MetricsCollector>) -> Result<Self> {
        info!("Initializing Knowledge Graph with connection pool and timeout management...");
        
        // Create circuit breaker
        let circuit_breaker = Arc::new(CircuitBreaker::new(CircuitBreakerConfig {
            failure_threshold: 5,
            success_threshold: 3,
            timeout: std::time::Duration::from_secs(30),
            window: std::time::Duration::from_secs(60),
        }));
        
        // Create connection pool
        let pool_config = PoolConfig {
            max_connections: 10,
            min_connections: 2,
            connection_timeout: std::time::Duration::from_secs(30),
            health_check_interval: std::time::Duration::from_secs(60),
            reconnect_delay: std::time::Duration::from_secs(5),
            max_reconnect_attempts: 10,
        };
        
        let connection_pool = Arc::new(ConnectionPool::new(neo4j_config, pool_config));
        connection_pool.initialize().await?;
        
        // Create timeout manager with database-specific timeouts
        let timeout_manager = Arc::new(TimeoutManager::with_default_config());
        
        // Initialize entity cache
        let entity_cache_config = EntityCacheConfig::default();
        let entity_cache = Arc::new(EntityLRUCache::new(entity_cache_config).await);
        
        let kg = Self {
            connection_pool,
            initialized: Arc::new(RwLock::new(false)),
            metrics,
            circuit_breaker,
            timeout_manager,
            entity_cache,
        };
        
        // Initialize schema
        kg.initialize_schema().await?;
        
        Ok(kg)
    }
    
    async fn initialize_schema(&self) -> Result<()> {
        let mut initialized = self.initialized.write().await;
        if *initialized {
            return Ok(());
        }
        
        info!("Initializing Knowledge Graph schema...");
        
        // Create indexes for performance
        let indexes = vec![
            "CREATE INDEX IF NOT EXISTS FOR (n:File) ON (n.path)",
            "CREATE INDEX IF NOT EXISTS FOR (n:Function) ON (n.name)",
            "CREATE INDEX IF NOT EXISTS FOR (n:Class) ON (n.name)",
            "CREATE INDEX IF NOT EXISTS FOR (n:Module) ON (n.name)",
            "CREATE INDEX IF NOT EXISTS FOR (n:Variable) ON (n.name)",
            "CREATE INDEX IF NOT EXISTS FOR (n:Entity) ON (n.id)",
            "CREATE INDEX IF NOT EXISTS FOR (n:Entity) ON (n.entity_type)",
            "CREATE TEXT INDEX IF NOT EXISTS FOR (n:Entity) ON (n.content)",
        ];
        
        for index_query in indexes {
            let result = self.timeout_manager.execute_with_timeout(
                OperationType::DatabaseConnection,
                format!("Create index: {}", index_query),
                None,
                self.connection_pool.with_retry(|graph| {
                    let query_str = index_query.to_string();
                    Box::pin(async move {
                        graph.run(query(&query_str)).await
                            .map_err(|e| crate::errors::KnowledgeGraphError::Database(format!("Index creation error: {}", e)))?;
                        Ok(())
                    })
                })
            ).await;
            
            match result {
                Ok(_) => debug!("Created index: {}", index_query),
                Err(e) => warn!("Failed to create index (may already exist): {}", e),
            }
        }
        
        // Create constraints
        let constraints = vec![
            "CREATE CONSTRAINT IF NOT EXISTS FOR (n:Entity) REQUIRE n.id IS UNIQUE",
        ];
        
        for constraint_query in constraints {
            let result = self.timeout_manager.execute_with_timeout(
                OperationType::DatabaseConnection,
                format!("Create constraint: {}", constraint_query),
                None,
                self.connection_pool.with_retry(|graph| {
                    let query_str = constraint_query.to_string();
                    Box::pin(async move {
                        graph.run(query(&query_str)).await
                            .map_err(|e| crate::errors::KnowledgeGraphError::Database(format!("Constraint creation error: {}", e)))?;
                        Ok(())
                    })
                })
            ).await;
            
            match result {
                Ok(_) => debug!("Created constraint: {}", constraint_query),
                Err(e) => warn!("Failed to create constraint (may already exist): {}", e),
            }
        }
        
        *initialized = true;
        info!("Knowledge Graph schema initialized");
        Ok(())
    }
    
    pub async fn add_entity(&self, entity: CodeEntity) -> Result<String> {
        // Cache the entity first for fast retrieval
        let cache_key = format!("{}:{}", entity.entity_type, entity.name);
        self.entity_cache.put(&cache_key, entity.clone()).await?;
        
        let query_str = r#"
            MERGE (e:Entity {id: $id})
            SET e.entity_type = $entity_type,
                e.name = $name,
                e.file_path = $file_path,
                e.content = $content,
                e.metadata = $metadata,
                e.updated_at = datetime()
            WITH e
            CALL apoc.create.addLabels(e, [$label]) YIELD node
            RETURN node.id as id
        "#;
        
        let label = entity.entity_type.clone();
        let entity_metadata_str = serde_json::to_string(&entity.metadata)
            .map_err(|e| anyhow!("Failed to serialize entity metadata: {}", e))?;
        
        let result = self.timeout_manager.execute_with_timeout(
            OperationType::Query,
            format!("Add entity: {} ({})", entity.name, entity.entity_type),
            None,
            self.connection_pool.with_retry(|graph| {
                let query_str = query_str.to_string();
                let entity_id = entity.id.clone();
                let entity_type_str = entity.entity_type.clone();
                let entity_name = entity.name.clone();
                let entity_file_path = entity.file_path.clone().unwrap_or_default();
                let entity_content = entity.content.clone().unwrap_or_default();
                let entity_metadata_str = entity_metadata_str.clone();
                let label = label.clone();
                
                Box::pin(async move {
                    let mut result = graph.execute(
                        query(&query_str)
                            .param("id", entity_id.clone())
                            .param("entity_type", entity_type_str)
                            .param("name", entity_name)
                            .param("file_path", entity_file_path)
                            .param("content", entity_content)
                            .param("metadata", entity_metadata_str)
                            .param("label", label)
                    ).await
                    .map_err(|e| crate::errors::KnowledgeGraphError::Database(format!("Query execution error: {}", e)))?;
                    
                    if let Ok(Some(row)) = result.next().await {
                        let id: String = row.get("id")
                            .map_err(|e| crate::errors::KnowledgeGraphError::Database(format!("Result parsing error: {}", e)))?;
                        debug!("Added entity: {} ({})", id, label);
                        Ok(id)
                    } else {
                        Err(crate::errors::KnowledgeGraphError::Database("Failed to add entity".to_string()))
                    }
                })
            })
        ).await?;
        
        Ok(result)
    }
    
    pub async fn get_entity(&self, entity_id: &str) -> Result<Option<CodeEntity>> {
        // Try cache first
        if let Some(cached_entity) = self.entity_cache.get(entity_id).await {
            debug!("Retrieved entity from cache: {}", entity_id);
            return Ok(Some(cached_entity));
        }
        
        // Fall back to database
        let query_str = "MATCH (e:Entity {id: $id}) RETURN e";
        
        let result = self.timeout_manager.execute_with_timeout(
            OperationType::Query,
            format!("Get entity: {}", entity_id),
            None,
            self.connection_pool.with_retry(|graph| {
                let query_str = query_str.to_string();
                let entity_id_str = entity_id.to_string();
                
                Box::pin(async move {
                    let mut result = graph.execute(
                        query(&query_str).param("id", entity_id_str)
                    ).await
                    .map_err(|e| crate::errors::KnowledgeGraphError::Database(format!("Query execution error: {}", e)))?;
                    
                    if let Ok(Some(row)) = result.next().await {
                        let node: Node = row.get("e")
                            .map_err(|e| crate::errors::KnowledgeGraphError::Database(format!("Node parsing error: {}", e)))?;
                        
                        // Convert Neo4j node to CodeEntity
                        let entity = CodeEntity {
                            id: node.get("id").unwrap_or_default(),
                            name: node.get("name").unwrap_or_default(),
                            entity_type: node.get("entity_type").unwrap_or_default(),
                            file_path: node.get("file_path"),
                            content: node.get("content"),
                            metadata: serde_json::from_str(&node.get::<String>("metadata").unwrap_or_default()).unwrap_or_default(),
                        };
                        
                        Ok(Some(entity))
                    } else {
                        Ok(None)
                    }
                })
            })
        ).await?;
        
        // Cache the result if found
        if let Some(ref entity) = result {
            let cache_key = format!("{}:{}", entity.entity_type, entity.name);
            let _ = self.entity_cache.put(&cache_key, entity.clone()).await;
        }
        
        Ok(result)
    }
    
    pub async fn add_relationship(
        &self,
        from_id: &str,
        to_id: &str,
        relationship_type: RelationshipType,
        properties: Option<serde_json::Value>,
    ) -> Result<()> {
        let rel_type = match relationship_type {
            RelationshipType::Contains => "CONTAINS",
            RelationshipType::Calls => "CALLS",
            RelationshipType::Imports => "IMPORTS",
            RelationshipType::Inherits => "INHERITS",
            RelationshipType::Implements => "IMPLEMENTS",
            RelationshipType::DependsOn => "DEPENDS_ON",
            RelationshipType::References => "REFERENCES",
            RelationshipType::Defines => "DEFINES",
            RelationshipType::Uses => "USES",
        };
        
        let query_str = format!(
            r#"
            MATCH (from:Entity {{id: $from_id}})
            MATCH (to:Entity {{id: $to_id}})
            MERGE (from)-[r:{}]->(to)
            SET r.created_at = datetime()
            SET r += $properties
            "#,
            rel_type
        );
        
        let properties_str = properties.unwrap_or(serde_json::json!({})).to_string();
        
        self.timeout_manager.execute_with_timeout(
            OperationType::Query,
            format!("Add relationship: {} -[{}]-> {}", from_id, rel_type, to_id),
            None,
            self.connection_pool.with_retry(|graph| {
                let query_str = query_str.clone();
                let from_id_str = from_id.to_string();
                let to_id_str = to_id.to_string();
                let properties_str = properties_str.clone();
                let rel_type = rel_type.to_string();
                
                Box::pin(async move {
                    graph.run(
                        query(&query_str)
                            .param("from_id", from_id_str.clone())
                            .param("to_id", to_id_str.clone())
                            .param("properties", properties_str)
                    ).await
                    .map_err(|e| crate::errors::KnowledgeGraphError::Database(format!("Relationship creation error: {}", e)))?;
                    
                    debug!("Added relationship: {} -[{}]-> {}", from_id_str, rel_type, to_id_str);
                    Ok(())
                })
            })
        ).await
    }
    
    pub async fn query_natural_language(&self, question: &str) -> Result<Vec<serde_json::Value>> {
        let cypher = self.convert_nl_to_cypher(question)?;
        self.execute_cypher(&cypher).await
    }
    
    fn convert_nl_to_cypher(&self, question: &str) -> Result<String> {
        // First try enhanced NLP patterns with fuzzy matching
        if let Some(cypher) = enhance_natural_language_query(question) {
            info!("Matched enhanced NLP pattern for: {}", question);
            return Ok(cypher);
        }
        
        // Then try query templates
        if let Some(cypher) = query_templates::match_query_to_template(question) {
            info!("Matched query template for: {}", question);
            return Ok(cypher);
        }
        
        // Fall back to simple pattern matching for custom queries
        let question_lower = question.to_lowercase();
        
        if question_lower.contains("all functions") {
            Ok("MATCH (f:Function) RETURN f.name as name, f.file_path as file, f.line_start as line".to_string())
        } else if question_lower.contains("dependencies of") {
            if let Some(name) = Self::extract_quoted_string(question) {
                Ok(format!(
                    "MATCH (e:Entity {{name: '{}'}})-[:DEPENDS_ON|IMPORTS|CALLS]->(dep:Entity) RETURN dep.name as name, dep.entity_type as type, dep.file_path as file",
                    name
                ))
            } else {
                Err(anyhow!("Please specify the entity name in quotes"))
            }
        } else if question_lower.contains("calls") && question_lower.contains("function") {
            if let Some(name) = Self::extract_quoted_string(question) {
                Ok(format!(
                    "MATCH (caller:Entity)-[:CALLS]->(f:Function {{name: '{}'}}) RETURN caller.name as caller, caller.file_path as file",
                    name
                ))
            } else {
                Err(anyhow!("Please specify the function name in quotes"))
            }
        } else if question_lower.contains("structure") || question_lower.contains("architecture") {
            Ok("MATCH (f:File)-[:CONTAINS]->(e:Entity) RETURN f.path as file, collect(e.name) as entities ORDER BY f.path".to_string())
        } else {
            // Default to searching for entities by content
            Ok(format!(
                "MATCH (e:Entity) WHERE e.content CONTAINS '{}' RETURN e.name as name, e.entity_type as type, e.file_path as file LIMIT 10",
                question
            ))
        }
    }
    
    fn extract_quoted_string(text: &str) -> Option<String> {
        let start = text.find('"')?;
        let end = text.rfind('"')?;
        if start < end {
            Some(text[start + 1..end].to_string())
        } else {
            None
        }
    }
    
    pub async fn execute_cypher(&self, cypher: &str) -> Result<Vec<serde_json::Value>> {
        debug!("Executing Cypher: {}", cypher);
        
        // Start timer for metrics
        let timer = QueryTimer::new(self.metrics.clone(), cypher.to_string());
        
        let results = self.timeout_manager.execute_with_timeout(
            OperationType::Query,
            format!("Execute Cypher: {}", cypher.chars().take(50).collect::<String>()),
            None,
            self.connection_pool.with_retry(|graph| {
                let cypher_str = cypher.to_string();
                
                Box::pin(async move {
                    let mut result = graph.execute(query(&cypher_str)).await
                        .map_err(|e| crate::errors::KnowledgeGraphError::Database(format!("Query execution error: {}", e)))?;
                    let mut results = Vec::new();
                    
                    while let Ok(Some(row)) = result.next().await {
                        let mut json_row = serde_json::Map::new();
                        
                        for (key, value) in row.to::<neo4rs::BoltMap>()
                            .map_err(|e| crate::errors::KnowledgeGraphError::Database(format!("Row parsing error: {}", e)))?
                            .iter() {
                            json_row.insert(key.clone(), serde_json::Value::String(format!("{:?}", value)));
                        }
                        
                        results.push(serde_json::Value::Object(json_row));
                    }
                    
                    Ok(results)
                })
            })
        ).await?;
        
        // Record metrics
        timer.finish(results.len()).await;
        
        Ok(results)
    }
    
    pub async fn get_project_summary(&self) -> Result<serde_json::Value> {
        let stats_query = r#"
            MATCH (e:Entity)
            WITH e.entity_type as type, count(e) as count
            RETURN type, count
            ORDER BY count DESC
        "#;
        
        let relationship_query = r#"
            MATCH ()-[r]->()
            WITH type(r) as rel_type, count(r) as count
            RETURN rel_type, count
            ORDER BY count DESC
        "#;
        
        let entity_stats = self.execute_cypher(stats_query).await?;
        let relationship_stats = self.execute_cypher(relationship_query).await?;
        
        Ok(serde_json::json!({
            "entities": entity_stats,
            "relationships": relationship_stats,
            "cache_stats": self.entity_cache.get_stats().await
        }))
    }
    
    pub async fn find_similar_code(&self, entity_id: &str, limit: usize) -> Result<Vec<CodeEntity>> {
        // This is a simplified version - in production, you'd use vector embeddings
        let query_str = r#"
            MATCH (source:Entity {id: $id})
            MATCH (similar:Entity)
            WHERE source.id <> similar.id
                AND source.entity_type = similar.entity_type
                AND similar.content CONTAINS substring(source.content, 0, 50)
            RETURN similar
            LIMIT $limit
        "#;
        
        let results = self.timeout_manager.execute_with_timeout(
            OperationType::Query,
            format!("Find similar code for entity: {}", entity_id),
            None,
            self.connection_pool.with_retry(|graph| {
                let query_str = query_str.to_string();
                let entity_id_str = entity_id.to_string();
                let limit = limit as i64;
                
                Box::pin(async move {
                    let mut result = graph.execute(
                        query(&query_str)
                            .param("id", entity_id_str)
                            .param("limit", limit)
                    ).await
                    .map_err(|e| crate::errors::KnowledgeGraphError::Database(format!("Similar code query error: {}", e)))?;
                    
                    let mut entities = Vec::new();
                    
                    while let Ok(Some(row)) = result.next().await {
                        let node: Node = row.get("similar")
                            .map_err(|e| crate::errors::KnowledgeGraphError::Database(format!("Node parsing error: {}", e)))?;
                        // Convert node to CodeEntity
                        // This is simplified - you'd need proper deserialization
                        debug!("Found similar entity: {:?}", node);
                    }
                    
                    Ok(entities)
                })
            })
        ).await?;
        
        Ok(results)
    }
    
    pub async fn clear_graph(&self) -> Result<()> {
        warn!("Clearing entire knowledge graph!");
        
        // Clear cache first
        self.entity_cache.clear().await;
        
        self.timeout_manager.execute_with_timeout(
            OperationType::Query,
            "Clear entire knowledge graph".to_string(),
            None,
            self.connection_pool.with_retry(|graph| {
                Box::pin(async move {
                    graph.run(query("MATCH (n) DETACH DELETE n")).await
                        .map_err(|e| crate::errors::KnowledgeGraphError::Database(format!("Clear graph error: {}", e)))?;
                    info!("Knowledge graph cleared");
                    Ok(())
                })
            })
        ).await
    }
    
    pub async fn get_graph_stats(&self) -> Result<(u64, u64)> {
        let node_count_query = "MATCH (n) RETURN count(n) as count";
        let rel_count_query = "MATCH ()-[r]->() RETURN count(r) as count";
        
        let node_result = self.execute_cypher(node_count_query).await?;
        let node_count = node_result.first()
            .and_then(|v| v.get("count"))
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);
        
        let rel_result = self.execute_cypher(rel_count_query).await?;
        let rel_count = rel_result.first()
            .and_then(|v| v.get("count"))
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);
        
        Ok((node_count, rel_count))
    }
    
    pub async fn get_circuit_breaker_stats(&self) -> crate::circuit_breaker::CircuitBreakerStats {
        self.circuit_breaker.get_stats().await
    }
    
    pub async fn get_connection_stats(&self) -> crate::connection_pool::ConnectionStats {
        self.connection_pool.get_stats().await
    }
    
    pub async fn get_timeout_stats(&self) -> crate::timeout_manager::TimeoutStats {
        self.timeout_manager.get_stats().await
    }
    
    pub async fn get_active_operations(&self) -> Vec<crate::timeout_manager::OperationContext> {
        self.timeout_manager.get_active_operations().await
    }
    
    pub async fn is_healthy(&self) -> bool {
        self.connection_pool.is_healthy().await
    }
    
    pub async fn get_entity_cache_stats(&self) -> crate::performance::EntityCacheStats {
        self.entity_cache.get_stats().await
    }
    
    pub async fn warm_entity_cache(&self, entities: Vec<(String, CodeEntity)>) -> Result<()> {
        self.entity_cache.warm_cache(entities).await
    }
    
    pub async fn get_hot_entity_keys(&self, limit: usize) -> Vec<String> {
        self.entity_cache.get_hot_keys(limit).await
    }
}
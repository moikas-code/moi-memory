    
    async fn check_neo4j_health(&self) -> bool {
        match self.knowledge_graph.execute_cypher("RETURN 1 as health").await {
            Ok(_) => true,
            Err(e) => {
                warn!("Neo4j health check failed: {}", e);
                false
            }
        }
    }
    
    async fn check_redis_health(&self) -> bool {
        match self.graceful_cache.get_cache_stats().await {
            Ok(_) => true,
            Err(e) => {
                warn!("Redis health check failed: {}", e);
                false
            }
        }
    }
    
    async fn read_resource(&self, uri: &str) -> Result<Value> {
        match uri {
            "project://current" => {
                let summary = self.knowledge_graph.get_project_summary().await?;
                Ok(summary)
            }
            
            "graph://entities" => {
                let entities = self.knowledge_graph.execute_cypher(
                    "MATCH (e:Entity) RETURN e LIMIT 1000"
                ).await?;
                Ok(json!(entities))
            }
            
            "graph://relationships" => {
                let relationships = self.knowledge_graph.execute_cypher(
                    "MATCH ()-[r]->() RETURN type(r) as type, count(r) as count ORDER BY count DESC"
                ).await?;
                Ok(json!(relationships))
            }
            
            "monitoring://health" => {
                let system_health = self.health_monitor.get_system_health().await;
                Ok(json!(system_health))
            }
            
            "monitoring://alerts" => {
                let active_alerts = self.alert_manager.get_active_alerts().await;
                let alert_history = self.alert_manager.get_alert_history(Some(50)).await;
                Ok(json!({
                    "active_alerts": active_alerts,
                    "recent_history": alert_history
                }))
            }
            
            "monitoring://traces" => {
                let recent_traces = self.tracing_manager.get_recent_traces(20).await;
                let stats = self.tracing_manager.get_stats().await;
                Ok(json!({
                    "recent_traces": recent_traces,
                    "statistics": stats
                }))
            }
            
            _ => Err(anyhow!("Unknown resource URI: {}", uri))
        }
    }
    
    fn get_tool_definitions(&self) -> HashMap<String, ToolInfo> {
        let mut tools = HashMap::new();
        
        tools.insert("batch_analyze".to_string(), ToolInfo {
            description: "Analyze multiple files or directories in batch with security and performance optimization".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "paths": {
                        "type": "array",
                        "items": {
                            "type": "string"
                        },
                        "description": "Array of file or directory paths to analyze"
                    },
                    "recursive": {
                        "type": "boolean",
                        "description": "Whether to recursively analyze directories (default: false)"
                    }
                },
                "required": ["paths"]
            }),
        });
        
        tools.insert("query_knowledge".to_string(), ToolInfo {
            description: "Query the knowledge graph with Cypher sanitization, caching, and parallel execution".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Cypher query (will be sanitized for security)"
                    }
                },
                "required": ["query"]
            }),
        });
        
        tools.insert("system_monitoring".to_string(), ToolInfo {
            description: "Access comprehensive system monitoring including health, alerts, and distributed tracing".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "type": {
                        "type": "string",
                        "enum": ["health", "alerts", "traces", "comprehensive"],
                        "description": "Type of monitoring data to retrieve (default: health)"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Limit for traces (default: 10)"
                    }
                }
            }),
        });
        
        tools.insert("alert_management".to_string(), ToolInfo {
            description: "Manage system alerts including listing, triggering, and resolving alerts".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["list_rules", "list_alerts", "trigger_alert", "resolve_alert"],
                        "description": "Alert management action to perform"
                    },
                    "title": {
                        "type": "string",
                        "description": "Alert title (for trigger_alert)"
                    },
                    "description": {
                        "type": "string",
                        "description": "Alert description (for trigger_alert)"
                    },
                    "severity": {
                        "type": "string",
                        "enum": ["info", "warning", "critical", "emergency"],
                        "description": "Alert severity (for trigger_alert, default: warning)"
                    },
                    "component": {
                        "type": "string",
                        "description": "Component name (for trigger_alert)"
                    },
                    "alert_id": {
                        "type": "string",
                        "description": "Alert ID (for resolve_alert)"
                    },
                    "history_limit": {
                        "type": "integer",
                        "description": "Limit for alert history (for list_alerts)"
                    }
                },
                "required": ["action"]
            }),
        });
        
        tools.insert("export_graph".to_string(), ToolInfo {
            description: "Export knowledge graph data in various formats (JSON, CSV, GEXF) with security validation".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "format": {
                        "type": "string",
                        "enum": ["json", "csv", "gexf"],
                        "description": "Export format (default: json)"
                    },
                    "include_metadata": {
                        "type": "boolean",
                        "description": "Include metadata in export (default: true)"
                    }
                }
            }),
        });
        
        tools.insert("analyze_code_structure".to_string(), ToolInfo {
            description: "Analyze code structure of files or projects with caching and dependency analysis".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "File or directory path to analyze"
                    },
                    "include_dependencies": {
                        "type": "boolean",
                        "description": "Include dependency analysis (default: true)"
                    }
                },
                "required": ["path"]
            }),
        });
        
        tools.insert("find_relationships".to_string(), ToolInfo {
            description: "Find relationships for an entity with security validation and performance optimization".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "entity_name": {
                        "type": "string",
                        "description": "Name of the entity to find relationships for"
                    },
                    "relationship_types": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Types of relationships to find (default: all common types)"
                    },
                    "direction": {
                        "type": "string",
                        "enum": ["incoming", "outgoing", "both"],
                        "description": "Direction of relationships (default: both)"
                    }
                },
                "required": ["entity_name"]
            }),
        });
        
        tools.insert("search_similar_code".to_string(), ToolInfo {
            description: "Search for similar code patterns with security validation".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "search_text": {
                        "type": "string",
                        "description": "Text to search for"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of results (default: 10)"
                    },
                    "similarity_threshold": {
                        "type": "number",
                        "description": "Similarity threshold 0-1 (default: 0.7)"
                    }
                },
                "required": ["search_text"]
            }),
        });
        
        tools.insert("get_project_context".to_string(), ToolInfo {
            description: "Get project context around an entity or general overview with security validation".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "focus_entity": {
                        "type": "string",
                        "description": "Entity to focus on (optional)"
                    },
                    "depth": {
                        "type": "integer",
                        "description": "Relationship traversal depth (default: 2)"
                    }
                }
            }),
        });
        
        tools.insert("track_decision".to_string(), ToolInfo {
            description: "Track architectural decisions and their impact on entities with authentication required".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "decision_id": {
                        "type": "string",
                        "description": "Unique decision ID (auto-generated if not provided)"
                    },
                    "decision_type": {
                        "type": "string",
                        "description": "Type of decision (architectural, refactoring, etc.)"
                    },
                    "description": {
                        "type": "string",
                        "description": "Description of the decision"
                    },
                    "affected_entities": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "List of entity names affected by this decision"
                    }
                },
                "required": ["decision_type", "description"]
            }),
        });
        
        tools.insert("get_metrics".to_string(), ToolInfo {
            description: "Get comprehensive system metrics including security, performance, health, and monitoring statistics".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {}
            }),
        });
        
        tools.insert("health_check".to_string(), ToolInfo {
            description: "Comprehensive health check of all server components with detailed monitoring status".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {}
            }),
        });
        
        tools
    }
    
    fn get_resource_definitions(&self) -> HashMap<String, ResourceInfo> {
        let mut resources = HashMap::new();
        
        resources.insert("project".to_string(), ResourceInfo {
            description: "Current project information".to_string(),
            mime_types: vec!["application/json".to_string()],
        });
        
        resources.insert("graph".to_string(), ResourceInfo {
            description: "Knowledge graph data".to_string(),
            mime_types: vec!["application/json".to_string()],
        });
        
        resources.insert("monitoring".to_string(), ResourceInfo {
            description: "System monitoring and health data".to_string(),
            mime_types: vec!["application/json".to_string()],
        });
        
        resources
    }
    
    fn error_response(&self, code: i32, message: &str, id: Option<Value>) -> JsonRpcResponse {
        JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.to_string(),
                data: None,
            }),
            id,
        }
    }
    
    /// Start file change processor with backpressure management
    fn start_file_change_processor_with_backpressure(
        kg: Arc<KnowledgeGraph>,
        analyzer: Arc<Mutex<CodeAnalyzer>>,
        cache: Arc<GracefulCacheManager>,
        mut batch_rx: mpsc::Receiver<Vec<FileChangeEvent>>,
    ) {
        tokio::spawn(async move {
            while let Some(batch) = batch_rx.recv().await {
                debug!("Processing file change batch of {} events", batch.len());
                
                for event in batch {
                    let path = &event.path;
                    debug!("Processing file change: {:?} ({:?})", path, event.change_type);
                    
                    if matches!(event.change_type, FileChangeType::Deleted) {
                        let _ = cache.invalidate_cache_pattern(&path.to_string_lossy()).await;
                        continue;
                    }
                    
                    if let Ok(entities) = analyzer.lock().await.analyze_file(path).await {
                        for entity in entities {
                            if let Err(e) = kg.add_entity(entity).await {
                                warn!("Failed to add entity from {}: {}", path.display(), e);
                            }
                        }
                        debug!("Successfully processed file change: {:?}", path);
                    } else {
                        debug!("Failed to analyze file: {:?}", path);
                    }
                    
                    let _ = cache.invalidate_cache_pattern(&path.to_string_lossy()).await;
                }
            }
        });
    }
}
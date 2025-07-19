use anyhow::Result;
use knowledge_graph_mcp::knowledge_graph::{KnowledgeGraph, CodeEntity, EntityType};
use knowledge_graph_mcp::code_analyzer::CodeAnalyzer;
use std::path::Path;

#[tokio::test]
async fn test_knowledge_graph_basic_operations() -> Result<()> {
    // This test requires Docker to be running
    let kg = KnowledgeGraph::new().await?;
    
    // Create a test entity
    let entity = CodeEntity {
        id: "test-entity-1".to_string(),
        entity_type: EntityType::Function,
        name: "test_function".to_string(),
        file_path: "test.rs".to_string(),
        line_start: 1,
        line_end: 5,
        content: "fn test_function() { }".to_string(),
        language: "rust".to_string(),
        metadata: serde_json::json!({}),
    };
    
    // Add entity
    let id = kg.add_entity(entity).await?;
    assert_eq!(id, "test-entity-1");
    
    // Query for the entity
    let results = kg.execute_cypher("MATCH (e:Entity {id: 'test-entity-1'}) RETURN e.name as name").await?;
    assert!(!results.is_empty());
    
    // Clean up
    kg.clear_graph().await?;
    
    Ok(())
}

#[tokio::test]
async fn test_code_analyzer_rust_parsing() -> Result<()> {
    let analyzer = CodeAnalyzer::new();
    
    // Create a test file
    let test_content = r#"
use std::collections::HashMap;

struct TestStruct {
    field: String,
}

impl TestStruct {
    fn new() -> Self {
        Self {
            field: String::new(),
        }
    }
    
    fn method(&self) -> &str {
        &self.field
    }
}

fn main() {
    let test = TestStruct::new();
    println!("{}", test.method());
}
"#;
    
    // Write test file
    let test_dir = std::env::temp_dir().join("kg-test");
    std::fs::create_dir_all(&test_dir)?;
    let test_file = test_dir.join("test.rs");
    std::fs::write(&test_file, test_content)?;
    
    // Analyze the file
    let entities = analyzer.analyze_file(&test_file).await?;
    
    // Verify entities were extracted
    assert!(entities.len() > 1); // Should have file, struct, impl, functions
    
    // Check for specific entities
    let has_struct = entities.iter().any(|e| e.name == "TestStruct" && matches!(e.entity_type, EntityType::Class));
    let has_main = entities.iter().any(|e| e.name == "main" && matches!(e.entity_type, EntityType::Function));
    let has_method = entities.iter().any(|e| e.name == "method" && matches!(e.entity_type, EntityType::Method));
    
    assert!(has_struct, "Should find TestStruct");
    assert!(has_main, "Should find main function");
    assert!(has_method, "Should find method");
    
    // Clean up
    std::fs::remove_dir_all(&test_dir)?;
    
    Ok(())
}

#[tokio::test]
async fn test_natural_language_query_conversion() -> Result<()> {
    let kg = KnowledgeGraph::new().await?;
    
    // Add some test data
    let function_entity = CodeEntity {
        id: "func-1".to_string(),
        entity_type: EntityType::Function,
        name: "calculate_total".to_string(),
        file_path: "math.rs".to_string(),
        line_start: 10,
        line_end: 20,
        content: "fn calculate_total() { }".to_string(),
        language: "rust".to_string(),
        metadata: serde_json::json!({}),
    };
    
    kg.add_entity(function_entity).await?;
    
    // Test natural language query
    let results = kg.query_natural_language("show me all functions").await?;
    assert!(!results.is_empty());
    
    // Clean up
    kg.clear_graph().await?;
    
    Ok(())
}

#[cfg(test)]
mod cache_tests {
    use super::*;
    use knowledge_graph_mcp::cache_manager::CacheManager;
    
    #[tokio::test]
    async fn test_cache_operations() -> Result<()> {
        let mut cache = CacheManager::new().await?;
        
        // Test query caching
        let test_query = "SELECT * FROM test";
        let test_result = serde_json::json!({
            "data": [1, 2, 3],
            "count": 3
        });
        
        // Cache a result
        cache.cache_query_result(test_query, &test_result, None).await?;
        
        // Retrieve from cache
        let cached = cache.get_cached_query_result(test_query).await?;
        assert!(cached.is_some());
        assert_eq!(cached.unwrap(), test_result);
        
        // Test cache miss
        let miss = cache.get_cached_query_result("non-existent-query").await?;
        assert!(miss.is_none());
        
        // Test cache stats
        let stats = cache.get_cache_stats().await?;
        assert!(stats.hits > 0);
        assert!(stats.misses > 0);
        
        Ok(())
    }
}
# Comprehensive Testing Strategy

## Overview
This document outlines the testing strategy for ensuring the Knowledge Graph MCP Server is production-ready.

## Test Categories

### 1. Unit Tests (Target: 80% Coverage)

#### Docker Manager Tests
```rust
// tests/unit/docker_manager_test.rs
#[cfg(test)]
mod tests {
    use super::*;
    use mockall::*;
    
    #[tokio::test]
    async fn test_container_health_check() {
        let mock_docker = MockDocker::new();
        mock_docker.expect_inspect_container()
            .returning(|_| Ok(container_running()));
        
        let manager = DockerManager::with_client(mock_docker);
        assert!(manager.is_container_healthy("test").await.unwrap());
    }
    
    #[tokio::test]
    async fn test_auto_start_missing_container() {
        // Test container creation when not found
    }
    
    #[tokio::test] 
    async fn test_retry_on_connection_failure() {
        // Test exponential backoff
    }
}
```

#### Knowledge Graph Tests
```rust
// tests/unit/knowledge_graph_test.rs
#[tokio::test]
async fn test_entity_creation() {
    let kg = KnowledgeGraph::new_test().await.unwrap();
    
    let entity = CodeEntity {
        id: "test-1".to_string(),
        entity_type: EntityType::Function,
        name: "test_func".to_string(),
        // ...
    };
    
    let id = kg.add_entity(entity).await.unwrap();
    assert_eq!(id, "test-1");
    
    // Verify entity exists
    let result = kg.execute_cypher(
        "MATCH (e:Entity {id: 'test-1'}) RETURN e"
    ).await.unwrap();
    assert!(!result.is_empty());
}

#[tokio::test]
async fn test_natural_language_conversion() {
    let kg = KnowledgeGraph::new_test().await.unwrap();
    
    let test_cases = vec![
        ("show me all functions", "MATCH (f:Function) RETURN f"),
        ("what calls main", "MATCH (caller)-[:CALLS]->(f:Function {name: 'main'}) RETURN caller"),
    ];
    
    for (nl_query, expected_cypher) in test_cases {
        let cypher = kg.nl_to_cypher(nl_query).unwrap();
        assert_eq!(cypher, expected_cypher);
    }
}
```

### 2. Integration Tests

#### End-to-End MCP Communication
```rust
// tests/integration/mcp_protocol_test.rs
#[tokio::test]
async fn test_full_mcp_lifecycle() {
    let server = TestServer::start().await;
    let client = TestMCPClient::connect(&server).await;
    
    // Test initialization
    let init_response = client.send_request(json!({
        "jsonrpc": "2.0",
        "method": "initialize",
        "id": 1
    })).await;
    
    assert_eq!(init_response["result"]["protocolVersion"], "0.1.0");
    
    // Test tool listing
    let tools_response = client.send_request(json!({
        "jsonrpc": "2.0",
        "method": "tools/list",
        "id": 2
    })).await;
    
    assert!(tools_response["result"]["tools"].as_object().unwrap().contains_key("query_knowledge"));
    
    // Test tool execution
    let query_response = client.send_request(json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "query_knowledge",
            "arguments": {
                "query": "show me all functions"
            }
        },
        "id": 3
    })).await;
    
    assert!(query_response["result"].is_object());
}
```

#### Multi-Language Code Analysis
```rust
// tests/integration/code_analysis_test.rs
#[tokio::test]
async fn test_analyze_mixed_language_project() {
    let test_project = TestProject::create_mixed()
        .add_rust_file("src/main.rs", RUST_SAMPLE)
        .add_js_file("index.js", JS_SAMPLE)
        .add_python_file("app.py", PYTHON_SAMPLE)
        .build();
    
    let analyzer = CodeAnalyzer::new();
    let entities = analyzer.analyze_project(&test_project.path()).await.unwrap();
    
    // Verify entities from all languages
    assert!(entities.iter().any(|e| e.language == "rust"));
    assert!(entities.iter().any(|e| e.language == "javascript"));
    assert!(entities.iter().any(|e| e.language == "python"));
    
    // Verify relationships
    let imports = entities.iter()
        .filter(|e| matches!(e.entity_type, EntityType::Import))
        .count();
    assert!(imports > 0);
}
```

### 3. Performance Tests

#### Benchmark Suite
```rust
// benches/performance.rs
use criterion::{black_box, criterion_group, criterion_main, Criterion};

fn benchmark_natural_language_query(c: &mut Criterion) {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let kg = runtime.block_on(KnowledgeGraph::new()).unwrap();
    
    c.bench_function("nl_query_simple", |b| {
        b.iter(|| {
            runtime.block_on(
                kg.query_natural_language(black_box("show me all functions"))
            )
        })
    });
    
    c.bench_function("nl_query_complex", |b| {
        b.iter(|| {
            runtime.block_on(
                kg.query_natural_language(black_box("find all functions that call parse_config and are in the auth module"))
            )
        })
    });
}

fn benchmark_code_analysis(c: &mut Criterion) {
    let analyzer = CodeAnalyzer::new();
    
    c.bench_function("analyze_small_file", |b| {
        b.iter(|| {
            runtime.block_on(
                analyzer.analyze_file(black_box(Path::new("small.rs")))
            )
        })
    });
    
    c.bench_function("analyze_large_file", |b| {
        b.iter(|| {
            runtime.block_on(
                analyzer.analyze_file(black_box(Path::new("large.rs")))
            )
        })
    });
}

criterion_group!(benches, benchmark_natural_language_query, benchmark_code_analysis);
criterion_main!(benches);
```

### 4. Load Tests

#### Stress Testing
```rust
// tests/load/stress_test.rs
#[tokio::test]
async fn test_concurrent_queries() {
    let server = TestServer::start().await;
    let num_clients = 100;
    let queries_per_client = 50;
    
    let handles: Vec<_> = (0..num_clients)
        .map(|i| {
            tokio::spawn(async move {
                let client = TestMCPClient::connect(&server).await;
                for j in 0..queries_per_client {
                    let result = client.query(&format!("query {} from client {}", j, i)).await;
                    assert!(result.is_ok());
                }
            })
        })
        .collect();
    
    // Wait for all clients
    for handle in handles {
        handle.await.unwrap();
    }
    
    // Verify server is still responsive
    let client = TestMCPClient::connect(&server).await;
    assert!(client.health_check().await.is_ok());
}

#[tokio::test]
async fn test_large_codebase_analysis() {
    // Create synthetic large project
    let project = TestProject::create_large(
        1000, // files
        500,  // lines per file
    );
    
    let start = Instant::now();
    let result = analyze_project(&project.path()).await;
    let duration = start.elapsed();
    
    assert!(result.is_ok());
    assert!(duration.as_secs() < 60); // Should complete within 1 minute
    
    let entities = result.unwrap();
    assert!(entities.len() > 10000); // Should find many entities
}
```

### 5. Property-Based Tests

#### Using PropTest
```rust
// tests/property/parser_properties.rs
use proptest::prelude::*;

proptest! {
    #[test]
    fn test_entity_roundtrip(entity in arb_code_entity()) {
        let kg = runtime.block_on(KnowledgeGraph::new()).unwrap();
        
        let id = runtime.block_on(kg.add_entity(entity.clone())).unwrap();
        let retrieved = runtime.block_on(kg.get_entity(&id)).unwrap();
        
        prop_assert_eq!(entity.name, retrieved.name);
        prop_assert_eq!(entity.entity_type, retrieved.entity_type);
    }
    
    #[test]
    fn test_cypher_injection_safety(input in ".*") {
        let kg = runtime.block_on(KnowledgeGraph::new()).unwrap();
        
        // Should not panic or allow injection
        let result = runtime.block_on(
            kg.query_natural_language(&input)
        );
        
        prop_assert!(result.is_ok() || result.is_err());
    }
}

fn arb_code_entity() -> impl Strategy<Value = CodeEntity> {
    (
        ".*",  // id
        prop::sample::select(vec![EntityType::Function, EntityType::Class]),
        ".*",  // name
        ".*",  // file_path
        1u32..1000,  // line_start
        1u32..1000,  // line_end
    ).prop_map(|(id, entity_type, name, file_path, line_start, line_end)| {
        CodeEntity {
            id,
            entity_type,
            name,
            file_path,
            line_start,
            line_end: line_end.max(line_start),
            content: String::new(),
            language: "rust".to_string(),
            metadata: json!({}),
        }
    })
}
```

### 6. Fuzz Testing

#### AFL Fuzzing
```rust
// fuzz/targets/query_parser.rs
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        let _ = parse_natural_language_query(s);
    }
});
```

## Test Data Management

### Test Fixtures
```
tests/fixtures/
├── small_project/
│   ├── src/
│   │   ├── main.rs
│   │   └── lib.rs
│   └── Cargo.toml
├── large_project/
│   └── ... (generated)
└── edge_cases/
    ├── unicode_names.rs
    ├── deeply_nested.js
    └── circular_deps.py
```

### Test Database
- Use testcontainers-rs for ephemeral Neo4j/Redis
- Snapshot testing for graph states
- Deterministic test data generation

## CI/CD Integration

### GitHub Actions Workflow
```yaml
name: Test Suite

on: [push, pull_request]

jobs:
  unit-tests:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v2
      - uses: actions-rs/toolchain@v1
      - run: cargo test --lib

  integration-tests:
    runs-on: ubuntu-latest
    services:
      neo4j:
        image: neo4j:5.15-community
      redis:
        image: redis:7-alpine
    steps:
      - uses: actions/checkout@v2
      - run: cargo test --test '*'

  benchmarks:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v2
      - run: cargo bench --no-run
      - run: cargo bench

  coverage:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v2
      - uses: actions-rs/tarpaulin@v0.1
      - run: cargo tarpaulin --out Xml
      - uses: codecov/codecov-action@v1
```

## Test Metrics Goals

- **Unit Test Coverage**: >80%
- **Integration Test Coverage**: >70%
- **Performance Regression**: <5%
- **Load Test Success**: 100 concurrent users
- **Fuzz Test Hours**: 24 hours without crashes

## Test Execution Strategy

1. **Local Development**: `cargo test` for quick feedback
2. **Pre-commit**: Fast unit tests only
3. **PR Validation**: Full test suite
4. **Nightly**: Extended fuzz and load tests
5. **Release**: Complete test suite + manual testing
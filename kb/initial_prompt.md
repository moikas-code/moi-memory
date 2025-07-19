# Knowledge Graph MCP Server Implementation Prompt

## Project Overview

I want you to implement a Model Context Protocol (MCP) server that provides intelligent persistent memory as a knowledge graph for development projects. This server will automatically capture, store, and make queryable all aspects of a software project including code structure, decisions, documentation, and relationships.

## Key Requirements

**Core Functionality:**
- MCP server that integrates with Claude Code
- Knowledge graph storage using Neo4j for code relationships
- Auto-starting Docker containers for dependencies (Neo4j + Redis)
- Real-time code analysis and relationship tracking
- Natural language querying of project knowledge
- Persistent memory across development sessions

**Performance Goals:**
- <100ms query response time
- Support for 100k+ lines of code
- Real-time updates as code changes
- Efficient caching with Redis

## Technology Stack Decision

Implement in **Rust** for maximum performance, using:
- `bollard` for Docker API integration
- `neo4rs` for Neo4j graph database
- `redis` for caching layer
- `tokio` for async runtime
- `tracing` for logging
- MCP protocol implementation

## Initial Project Structure

Create this file structure:

```
knowledge-graph-mcp/
├── Cargo.toml
├── src/
│   ├── main.rs
│   ├── mcp_server.rs
│   ├── docker_manager.rs
│   ├── knowledge_graph.rs
│   ├── code_analyzer.rs
│   ├── cache_manager.rs
│   └── lib.rs
├── config/
│   └── server_config.toml
├── tests/
│   ├── integration_tests.rs
│   └── test_fixtures/
└── README.md
```

## Starter Code Snippets

### Cargo.toml
```toml
[package]
name = "knowledge-graph-mcp"
version = "0.1.0"
edition = "2021"

[dependencies]
tokio = { version = "1.0", features = ["full"] }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
neo4rs = "0.7"
redis = "0.24"
bollard = "0.15"
uuid = { version = "1.0", features = ["v4"] }
tracing = "0.1"
tracing-subscriber = "0.3"
anyhow = "1.0"
futures-util = "0.3"
clap = { version = "4.0", features = ["derive"] }
tree-sitter = "0.20"
tree-sitter-rust = "0.20"
tree-sitter-javascript = "0.20"
tree-sitter-python = "0.20"
walkdir = "2.3"
notify = "6.0"

[profile.release]
lto = true
codegen-units = 1
panic = "abort"
```

### src/main.rs (Entry Point)
```rust
use anyhow::Result;
use tracing::{info, error};
use tracing_subscriber;

mod mcp_server;
mod docker_manager;
mod knowledge_graph;
mod code_analyzer;
mod cache_manager;

use mcp_server::KnowledgeGraphMCPServer;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::init();
    
    info!("🚀 Starting Knowledge Graph MCP Server...");
    
    // Initialize and start the MCP server
    let server = KnowledgeGraphMCPServer::new().await?;
    server.run().await?;
    
    Ok(())
}
```

### src/mcp_server.rs (Skeleton)
```rust
use anyhow::Result;
use serde_json::{json, Value};
use tracing::{info, warn, error};
use std::sync::Arc;

use crate::docker_manager::DockerManager;
use crate::knowledge_graph::KnowledgeGraph;
use crate::code_analyzer::CodeAnalyzer;
use crate::cache_manager::CacheManager;

pub struct KnowledgeGraphMCPServer {
    docker_manager: DockerManager,
    knowledge_graph: Arc<KnowledgeGraph>,
    code_analyzer: Arc<CodeAnalyzer>,
    cache_manager: Arc<CacheManager>,
}

impl KnowledgeGraphMCPServer {
    pub async fn new() -> Result<Self> {
        info!("Initializing Knowledge Graph MCP Server...");
        
        // TODO: Initialize Docker manager and auto-start containers
        let docker_manager = DockerManager::new().await?;
        
        // TODO: Initialize knowledge graph connection
        let knowledge_graph = Arc::new(KnowledgeGraph::new().await?);
        
        // TODO: Initialize code analyzer
        let code_analyzer = Arc::new(CodeAnalyzer::new());
        
        // TODO: Initialize cache manager
        let cache_manager = Arc::new(CacheManager::new().await?);
        
        Ok(Self {
            docker_manager,
            knowledge_graph,
            code_analyzer,
            cache_manager,
        })
    }
    
    pub async fn run(&self) -> Result<()> {
        info!("MCP Server listening for requests...");
        
        // TODO: Implement MCP protocol handler
        // TODO: Set up tool handlers for knowledge queries
        // TODO: Set up resource handlers for project data
        // TODO: Implement file system watching for real-time updates
        
        Ok(())
    }
}
```

## Comprehensive TODO List

### Phase 1: Infrastructure Setup (Priority 1)
- [ ] **Docker Manager Implementation**
  - [ ] Check if Neo4j container exists and is running
  - [ ] Auto-pull Neo4j 5.15-community image if missing
  - [ ] Create and start Neo4j container with proper config
  - [ ] Check if Redis container exists and is running  
  - [ ] Auto-pull Redis 7-alpine image if missing
  - [ ] Create and start Redis container
  - [ ] Implement health checking for both services
  - [ ] Add retry logic with exponential backoff
  - [ ] Handle container restart scenarios

- [ ] **Knowledge Graph Setup**
  - [ ] Establish Neo4j connection with retry logic
  - [ ] Create graph schema for code entities (File, Function, Class, Variable, Module)
  - [ ] Create relationship types (CALLS, IMPORTS, INHERITS, DEPENDS_ON, CONTAINS)
  - [ ] Add indexes for performance (entity names, file paths, content)
  - [ ] Implement graph query interface
  - [ ] Add transaction support for atomic updates

- [ ] **Cache Manager Setup**
  - [ ] Establish Redis connection
  - [ ] Implement query result caching
  - [ ] Add cache invalidation strategies
  - [ ] Implement session management for user contexts
  - [ ] Add metrics tracking for cache hit rates

### Phase 2: MCP Protocol Implementation (Priority 1)
- [ ] **Core MCP Server**
  - [ ] Implement JSON-RPC 2.0 message handling
  - [ ] Add server capabilities declaration (tools, resources, prompts)
  - [ ] Implement stdio transport for Claude Code integration
  - [ ] Add proper error handling and response formatting
  - [ ] Implement server initialization and handshake

- [ ] **Tool Handlers**
  - [ ] `query_knowledge` - Natural language project queries
  - [ ] `analyze_code_structure` - Get project architecture overview
  - [ ] `find_relationships` - Discover code dependencies
  - [ ] `search_similar_code` - Find similar code patterns
  - [ ] `get_project_context` - Current project state summary
  - [ ] `track_decision` - Record architectural decisions

- [ ] **Resource Handlers**
  - [ ] Project files and directories
  - [ ] Code relationship graphs
  - [ ] Decision records and documentation
  - [ ] Performance metrics and project stats

### Phase 3: Code Analysis Engine (Priority 2)
- [ ] **Parser Integration**
  - [ ] Set up Tree-sitter parsers for Rust, JavaScript, Python, TypeScript
  - [ ] Implement AST traversal for code structure extraction
  - [ ] Extract functions, classes, variables, imports
  - [ ] Build call graphs and dependency relationships
  - [ ] Handle multiple language projects

- [ ] **File System Monitoring**
  - [ ] Implement file watcher using `notify` crate
  - [ ] Filter relevant file changes (ignore .git, node_modules, target)
  - [ ] Queue file changes for batch processing
  - [ ] Implement incremental analysis for changed files
  - [ ] Update knowledge graph in real-time

- [ ] **Semantic Analysis**
  - [ ] Extract code comments and documentation
  - [ ] Identify architectural patterns
  - [ ] Track code complexity metrics
  - [ ] Detect code smells and potential issues
  - [ ] Build semantic relationships between entities

### Phase 4: Query Processing (Priority 2)
- [ ] **Natural Language Processing**
  - [ ] Parse user queries for intent detection
  - [ ] Map natural language to Cypher queries
  - [ ] Handle different query types (search, analyze, explain)
  - [ ] Implement query result ranking by relevance
  - [ ] Add query suggestions and auto-completion

- [ ] **Graph Query Optimization**
  - [ ] Implement efficient Cypher query generation
  - [ ] Add query result caching strategies
  - [ ] Optimize graph traversals for common patterns
  - [ ] Implement pagination for large result sets
  - [ ] Add query performance monitoring

### Phase 5: Advanced Features (Priority 3)
- [ ] **Vector Search Integration**
  - [ ] Set up vector embeddings for code similarity
  - [ ] Implement semantic code search
  - [ ] Add code recommendation engine
  - [ ] Integrate with knowledge graph for hybrid search

- [ ] **Project Intelligence**
  - [ ] Detect architectural patterns automatically
  - [ ] Track technical debt accumulation
  - [ ] Suggest refactoring opportunities
  - [ ] Monitor code quality trends over time
  - [ ] Generate project health reports

- [ ] **Team Collaboration Features**
  - [ ] Track multi-developer contributions
  - [ ] Maintain decision history with rationale
  - [ ] Support project handoff scenarios
  - [ ] Generate onboarding documentation

### Phase 6: Testing & Quality (Priority 2)
- [ ] **Unit Tests**
  - [ ] Docker manager container lifecycle tests
  - [ ] Knowledge graph CRUD operation tests
  - [ ] Code analyzer parsing accuracy tests
  - [ ] Cache manager functionality tests
  - [ ] MCP protocol compliance tests

- [ ] **Integration Tests**
  - [ ] End-to-end MCP communication tests
  - [ ] Real project analysis accuracy tests
  - [ ] Performance benchmarking under load
  - [ ] Container auto-recovery tests
  - [ ] Multi-language project tests

- [ ] **Performance Testing**
  - [ ] Query response time benchmarks
  - [ ] Memory usage profiling
  - [ ] Concurrent user simulation
  - [ ] Large codebase stress testing
  - [ ] Container startup time optimization

## Implementation Guidelines

### Error Handling Strategy
- Use `anyhow::Result` for error propagation
- Implement graceful degradation when services are unavailable
- Add comprehensive logging with structured context
- Return helpful error messages to Claude Code users

### Performance Considerations
- Implement connection pooling for Neo4j and Redis
- Use batch operations for bulk graph updates
- Cache frequently accessed data in Redis
- Optimize Cypher queries with appropriate indexes
- Use async/await throughout for non-blocking operations

### Security Requirements
- Validate all user inputs and file paths
- Implement rate limiting for expensive operations
- Secure database connections with authentication
- Sanitize code content before storing in graph
- Add audit logging for all graph modifications

## Testing Strategy

Create test fixtures for:
- Sample multi-language codebases
- Known code relationship patterns
- Complex dependency scenarios
- Real-world project structures

Implement benchmarks for:
- Query response times
- Graph update performance
- Memory usage patterns
- Container startup times

## Success Criteria

The implementation is successful when:
1. **Single Command Startup**: `cargo run` starts everything automatically
2. **Real-time Updates**: Code changes appear in knowledge graph within 1 second
3. **Fast Queries**: Natural language queries return results in <100ms
4. **Robust Recovery**: Handles container crashes and network issues gracefully
5. **Accurate Analysis**: Correctly identifies code relationships and patterns
6. **Claude Code Integration**: Works seamlessly as an MCP server

## Additional Notes

- Prioritize getting basic functionality working before optimizing
- Use structured logging to debug integration issues
- Test with real codebases early and often
- Focus on developer experience - it should "just work"
- Document any Docker or system requirements clearly

Begin with Phase 1 (Infrastructure Setup) and work through the TODOs systematically. Each phase builds on the previous one, so maintain working functionality as you add features.

The goal is a production-ready MCP server that transforms how developers interact with their codebases by making project knowledge instantly accessible and actionable.
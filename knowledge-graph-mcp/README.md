# Knowledge Graph MCP Server

A Model Context Protocol (MCP) server that provides intelligent persistent memory as a knowledge graph for development projects. This server automatically captures, stores, and makes queryable all aspects of a software project including code structure, decisions, documentation, and relationships.

## Features

- **Automatic Container Management**: Auto-starts and manages Neo4j and Redis containers
- **Real-time Code Analysis**: Monitors file changes and updates the knowledge graph in real-time
- **Natural Language Queries**: Query your codebase using natural language
- **Multi-language Support**: Analyzes Rust, JavaScript, TypeScript, and Python code
- **Intelligent Caching**: Redis-based caching for fast query responses
- **Code Relationship Tracking**: Tracks calls, imports, inheritance, and dependencies
- **Decision Tracking**: Record and query architectural decisions
- **Performance Optimized**: Written in Rust for maximum performance

## Prerequisites

- Docker installed and running
- Rust 1.70+ (for building from source)
- Available ports: 7474, 7687 (Neo4j), 6379 (Redis)

## Installation

### From Source

```bash
git clone https://github.com/yourusername/knowledge-graph-mcp.git
cd knowledge-graph-mcp
cargo build --release
```

## Usage

### Starting the Server

```bash
cargo run
```

The server will:
1. Automatically start Neo4j and Redis containers if not running
2. Initialize the knowledge graph schema
3. Listen for MCP commands on stdin/stdout

### Integration with Claude Code

Add to your Claude Code MCP configuration:

```json
{
  "mcpServers": {
    "knowledge-graph": {
      "command": "/path/to/knowledge-graph-mcp/target/release/knowledge-graph-mcp",
      "args": []
    }
  }
}
```

## Available Tools

### `query_knowledge`
Query the knowledge graph using natural language.

```typescript
{
  "query": "What functions call the parse_config function?"
}
```

### `analyze_code_structure`
Analyze a project's code structure and store it in the knowledge graph.

```typescript
{
  "path": "./src"  // Optional, defaults to current directory
}
```

### `find_relationships`
Find all relationships for a specific code entity.

```typescript
{
  "entity_name": "MyClass"
}
```

### `search_similar_code`
Find code similar to a given entity.

```typescript
{
  "entity_id": "uuid-of-entity",
  "limit": 10  // Optional, default 10
}
```

### `get_project_context`
Get current project context and statistics.

```typescript
{}  // No parameters required
```

### `track_decision`
Track an architectural or development decision.

```typescript
{
  "decision": "Use Neo4j for graph storage",
  "rationale": "Better performance for relationship queries"  // Optional
}
```

## Example Queries

### Natural Language Queries
- "Show me all functions in the authentication module"
- "What are the dependencies of the UserService class?"
- "Find all TODO comments in the codebase"
- "Which files import the config module?"
- "Show me the call graph for the main function"

### Direct Cypher Queries
The server also supports direct Cypher queries through the natural language interface:
- "MATCH (f:Function)-[:CALLS]->(g:Function) RETURN f.name, g.name"
- "MATCH (c:Class)-[:INHERITS]->(p:Class) RETURN c.name, p.name"

## Architecture

### Components

1. **Docker Manager**: Handles automatic container lifecycle management
2. **Knowledge Graph**: Neo4j-based graph database for storing code relationships
3. **Code Analyzer**: Tree-sitter based parser for multiple languages
4. **Cache Manager**: Redis-based caching layer for performance
5. **MCP Server**: JSON-RPC 2.0 protocol implementation

### Data Model

#### Entities
- File
- Module
- Class
- Function/Method
- Variable
- Import
- Type

#### Relationships
- CONTAINS
- CALLS
- IMPORTS
- INHERITS
- IMPLEMENTS
- DEPENDS_ON
- REFERENCES
- DEFINES
- USES

## Performance

- Query response time: <100ms (cached), <500ms (uncached)
- Supports codebases with 100k+ lines of code
- Real-time updates within 1 second of file changes
- Efficient batch processing for large projects

## Configuration

Edit `config/server_config.toml` to customize:
- Database connections
- Cache TTL settings
- Supported languages
- Ignored directories
- Performance tuning

## Development

### Running Tests

```bash
cargo test
```

### Building Documentation

```bash
cargo doc --open
```

### Contributing

1. Fork the repository
2. Create a feature branch
3. Make your changes
4. Add tests
5. Submit a pull request

## Troubleshooting

### Container Issues
- Ensure Docker is running: `docker ps`
- Check container logs: `docker logs knowledge-graph-neo4j`
- Verify ports are available: `lsof -i :7687,6379`

### Connection Issues
- Check Neo4j browser: http://localhost:7474
- Default credentials: neo4j/knowledge-graph-2024
- Redis CLI: `redis-cli ping`

### Performance Issues
- Increase Neo4j memory in container settings
- Adjust cache TTL in configuration
- Enable query result pagination for large datasets

## License

MIT License - see LICENSE file for details

## Acknowledgments

- Built with the Model Context Protocol (MCP) specification
- Uses Tree-sitter for code parsing
- Powered by Neo4j graph database and Redis cache
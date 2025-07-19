#!/bin/bash

# Knowledge Graph MCP Server Quick Start Script

echo "🚀 Knowledge Graph MCP Server Quick Start"
echo "========================================"

# Check prerequisites
echo "Checking prerequisites..."

# Check Docker
if ! command -v docker &> /dev/null; then
    echo "❌ Docker is not installed. Please install Docker first."
    echo "   Visit: https://docs.docker.com/get-docker/"
    exit 1
fi

# Check if Docker is running
if ! docker info &> /dev/null; then
    echo "❌ Docker is not running. Please start Docker."
    exit 1
fi

echo "✅ Docker is running"

# Check Rust
if ! command -v cargo &> /dev/null; then
    echo "❌ Rust is not installed. Installing via rustup..."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
    source $HOME/.cargo/env
fi

echo "✅ Rust is installed"

# Build the project
echo ""
echo "Building the project..."
cargo build --release

if [ $? -ne 0 ]; then
    echo "❌ Build failed. Please check the error messages above."
    exit 1
fi

echo "✅ Build successful"

# Check if containers are already running
echo ""
echo "Checking Docker containers..."

if docker ps | grep -q knowledge-graph-neo4j; then
    echo "ℹ️  Neo4j container is already running"
else
    echo "ℹ️  Neo4j container will be auto-started when the server runs"
fi

if docker ps | grep -q knowledge-graph-redis; then
    echo "ℹ️  Redis container is already running"
else
    echo "ℹ️  Redis container will be auto-started when the server runs"
fi

# Create example MCP config
echo ""
echo "Creating example MCP configuration..."

MCP_CONFIG_DIR="$HOME/.config/claude"
mkdir -p "$MCP_CONFIG_DIR"

cat > "$MCP_CONFIG_DIR/mcp-config-example.json" << EOF
{
  "mcpServers": {
    "knowledge-graph": {
      "command": "$(pwd)/target/release/knowledge-graph-mcp",
      "args": []
    }
  }
}
EOF

echo "✅ Example configuration created at: $MCP_CONFIG_DIR/mcp-config-example.json"

# Display next steps
echo ""
echo "🎉 Setup complete!"
echo ""
echo "Next steps:"
echo "1. Run the server: cargo run"
echo "2. The server will auto-start Neo4j and Redis containers"
echo "3. Configure Claude Code with the MCP server path"
echo "4. Start using natural language queries in Claude Code!"
echo ""
echo "Example queries:"
echo "- 'Show me all functions in this project'"
echo "- 'What calls the main function?'"
echo "- 'Find all TODO comments'"
echo ""
echo "For more information, see README.md"
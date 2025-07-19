#!/bin/bash
set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Installation directories
INSTALL_DIR="/opt/knowledge-graph-mcp"
CONFIG_DIR="/etc/knowledge-graph-mcp"
DATA_DIR="/var/lib/knowledge-graph-mcp"
LOG_DIR="/var/log/knowledge-graph-mcp"
SYSTEMD_DIR="/etc/systemd/system"

echo -e "${GREEN}Knowledge Graph MCP Server Installer${NC}"
echo "======================================"

# Check if running as root
if [ "$EUID" -ne 0 ]; then 
   echo -e "${RED}Please run as root (use sudo)${NC}"
   exit 1
fi

# Check dependencies
echo -e "${YELLOW}Checking dependencies...${NC}"

if ! command -v docker &> /dev/null; then
    echo -e "${RED}Docker is not installed. Please install Docker first.${NC}"
    exit 1
fi

if ! command -v docker compose &> /dev/null; then
    echo -e "${RED}Docker Compose is not installed. Please install Docker Compose v2.${NC}"
    exit 1
fi

# Create user if not exists
if ! id -u mcp &>/dev/null; then
    echo -e "${YELLOW}Creating mcp user...${NC}"
    useradd -r -s /bin/false -d /var/lib/knowledge-graph-mcp mcp
fi

# Create directories
echo -e "${YELLOW}Creating directories...${NC}"
mkdir -p "$INSTALL_DIR" "$CONFIG_DIR" "$DATA_DIR" "$LOG_DIR"
chown mcp:mcp "$DATA_DIR" "$LOG_DIR"

# Copy files
echo -e "${YELLOW}Copying files...${NC}"
cp -r . "$INSTALL_DIR/"
cp config/server_config.toml "$CONFIG_DIR/"
cp env.example "$CONFIG_DIR/env"

# Build the binary
echo -e "${YELLOW}Building the application...${NC}"
cd "$INSTALL_DIR"
cargo build --release

# Install binary
echo -e "${YELLOW}Installing binary...${NC}"
cp target/release/knowledge-graph-mcp /usr/local/bin/
chmod +x /usr/local/bin/knowledge-graph-mcp

# Install systemd service
echo -e "${YELLOW}Installing systemd service...${NC}"
cp scripts/knowledge-graph-mcp.service "$SYSTEMD_DIR/"
systemctl daemon-reload

# Set permissions
chown -R mcp:mcp "$INSTALL_DIR" "$CONFIG_DIR"
chmod 600 "$CONFIG_DIR/env"

# Add user to docker group
usermod -aG docker mcp

echo -e "${GREEN}Installation complete!${NC}"
echo ""
echo "Next steps:"
echo "1. Edit configuration: sudo nano $CONFIG_DIR/env"
echo "2. Start the service: sudo systemctl start knowledge-graph-mcp"
echo "3. Enable auto-start: sudo systemctl enable knowledge-graph-mcp"
echo "4. View logs: sudo journalctl -u knowledge-graph-mcp -f"
echo ""
echo "Neo4j Browser will be available at: http://localhost:7474"
echo "Default Neo4j credentials: neo4j / knowledge-graph-2024"
use anyhow::Result;
use tracing::{info, error};
use tracing_subscriber;

mod config;
mod errors;
mod retry;
mod mcp_server;
mod docker_manager;
mod knowledge_graph;
mod code_analyzer;
mod cache_manager;

use mcp_server::KnowledgeGraphMCPServer;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging with environment variable support
    let log_filter = std::env::var("RUST_LOG")
        .unwrap_or_else(|_| "knowledge_graph_mcp=info".to_string());
    
    tracing_subscriber::fmt()
        .with_env_filter(log_filter)
        .init();
    
    info!("🚀 Starting Knowledge Graph MCP Server...");
    
    // Initialize and start the MCP server
    match KnowledgeGraphMCPServer::new().await {
        Ok(server) => {
            info!("Server initialized successfully");
            if let Err(e) = server.run().await {
                error!("Server error: {}", errors::format_user_error(&e));
                std::process::exit(1);
            }
        }
        Err(e) => {
            error!("Failed to initialize server: {}", errors::format_user_error(&e));
            std::process::exit(1);
        }
    }
    
    Ok(())
}
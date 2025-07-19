use anyhow::{Result, anyhow};
use bollard::{Docker, API_DEFAULT_VERSION};
use bollard::container::{Config, CreateContainerOptions, StartContainerOptions};
use bollard::image::CreateImageOptions;
use bollard::models::{ContainerInspectResponse, ContainerState};
use futures_util::StreamExt;
use std::collections::HashMap;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{info, warn, error, debug};

const NEO4J_IMAGE: &str = "neo4j:5.15-community";
const REDIS_IMAGE: &str = "redis:7-alpine";
const NEO4J_CONTAINER_NAME: &str = "knowledge-graph-neo4j";
const REDIS_CONTAINER_NAME: &str = "knowledge-graph-redis";

pub struct DockerManager {
    docker: Docker,
}

impl DockerManager {
    pub async fn new() -> Result<Self> {
        info!("Initializing Docker Manager...");
        
        let docker = Docker::connect_with_local_defaults()
            .map_err(|e| anyhow!("Failed to connect to Docker: {}", e))?;
        
        // Verify Docker is accessible
        docker.ping().await
            .map_err(|e| anyhow!("Docker daemon is not accessible: {}", e))?;
        
        info!("Docker connection established");
        
        let manager = Self { docker };
        
        // Auto-start required containers
        manager.ensure_neo4j_running().await?;
        manager.ensure_redis_running().await?;
        
        Ok(manager)
    }
    
    async fn ensure_neo4j_running(&self) -> Result<()> {
        info!("Ensuring Neo4j container is running...");
        
        match self.get_container_state(NEO4J_CONTAINER_NAME).await {
            Ok(Some(state)) if state.running.unwrap_or(false) => {
                info!("Neo4j container is already running");
                Ok(())
            }
            Ok(Some(_)) => {
                info!("Starting existing Neo4j container...");
                self.start_container(NEO4J_CONTAINER_NAME).await
            }
            Ok(None) => {
                info!("Neo4j container not found, creating...");
                self.create_and_start_neo4j().await
            }
            Err(e) => {
                warn!("Error checking Neo4j container: {}", e);
                self.create_and_start_neo4j().await
            }
        }
    }
    
    async fn ensure_redis_running(&self) -> Result<()> {
        info!("Ensuring Redis container is running...");
        
        match self.get_container_state(REDIS_CONTAINER_NAME).await {
            Ok(Some(state)) if state.running.unwrap_or(false) => {
                info!("Redis container is already running");
                Ok(())
            }
            Ok(Some(_)) => {
                info!("Starting existing Redis container...");
                self.start_container(REDIS_CONTAINER_NAME).await
            }
            Ok(None) => {
                info!("Redis container not found, creating...");
                self.create_and_start_redis().await
            }
            Err(e) => {
                warn!("Error checking Redis container: {}", e);
                self.create_and_start_redis().await
            }
        }
    }
    
    async fn get_container_state(&self, name: &str) -> Result<Option<ContainerState>> {
        match self.docker.inspect_container(name, None).await {
            Ok(container) => Ok(container.state),
            Err(bollard::errors::Error::DockerResponseServerError { status_code: 404, .. }) => Ok(None),
            Err(e) => Err(anyhow!("Failed to inspect container: {}", e))
        }
    }
    
    async fn create_and_start_neo4j(&self) -> Result<()> {
        // Pull image if needed
        self.pull_image(NEO4J_IMAGE).await?;
        
        // Create container configuration
        let mut env = vec![
            "NEO4J_AUTH=neo4j/knowledge-graph-2024",
            "NEO4J_PLUGINS=[\"apoc\"]",
            "NEO4J_dbms_memory_pagecache_size=512M",
            "NEO4J_dbms_memory_heap_initial__size=512M",
            "NEO4J_dbms_memory_heap_max__size=1G",
        ];
        
        let mut exposed_ports = HashMap::new();
        exposed_ports.insert("7474/tcp", HashMap::new());
        exposed_ports.insert("7687/tcp", HashMap::new());
        
        let mut port_bindings = HashMap::new();
        port_bindings.insert("7474/tcp", Some(vec![HashMap::from([("HostPort", "7474")])]));
        port_bindings.insert("7687/tcp", Some(vec![HashMap::from([("HostPort", "7687")])]));
        
        let config = Config {
            image: Some(NEO4J_IMAGE),
            env: Some(env.iter().map(|s| s.to_string()).collect()),
            exposed_ports: Some(exposed_ports),
            host_config: Some(bollard::models::HostConfig {
                port_bindings: Some(port_bindings),
                restart_policy: Some(bollard::models::RestartPolicy {
                    name: Some("unless-stopped".to_string()),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        };
        
        let options = CreateContainerOptions {
            name: NEO4J_CONTAINER_NAME,
            ..Default::default()
        };
        
        self.docker.create_container(Some(options), config).await
            .map_err(|e| anyhow!("Failed to create Neo4j container: {}", e))?;
        
        info!("Neo4j container created");
        
        self.start_container(NEO4J_CONTAINER_NAME).await?;
        self.wait_for_neo4j_health().await
    }
    
    async fn create_and_start_redis(&self) -> Result<()> {
        // Pull image if needed
        self.pull_image(REDIS_IMAGE).await?;
        
        // Create container configuration
        let mut exposed_ports = HashMap::new();
        exposed_ports.insert("6379/tcp", HashMap::new());
        
        let mut port_bindings = HashMap::new();
        port_bindings.insert("6379/tcp", Some(vec![HashMap::from([("HostPort", "6379")])]));
        
        let config = Config {
            image: Some(REDIS_IMAGE),
            exposed_ports: Some(exposed_ports),
            host_config: Some(bollard::models::HostConfig {
                port_bindings: Some(port_bindings),
                restart_policy: Some(bollard::models::RestartPolicy {
                    name: Some("unless-stopped".to_string()),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        };
        
        let options = CreateContainerOptions {
            name: REDIS_CONTAINER_NAME,
            ..Default::default()
        };
        
        self.docker.create_container(Some(options), config).await
            .map_err(|e| anyhow!("Failed to create Redis container: {}", e))?;
        
        info!("Redis container created");
        
        self.start_container(REDIS_CONTAINER_NAME).await?;
        self.wait_for_redis_health().await
    }
    
    async fn pull_image(&self, image: &str) -> Result<()> {
        info!("Pulling Docker image: {}", image);
        
        let options = CreateImageOptions {
            from_image: image,
            ..Default::default()
        };
        
        let mut stream = self.docker.create_image(Some(options), None, None);
        
        while let Some(result) = stream.next().await {
            match result {
                Ok(info) => debug!("Pull progress: {:?}", info),
                Err(e) => return Err(anyhow!("Failed to pull image: {}", e))
            }
        }
        
        info!("Successfully pulled image: {}", image);
        Ok(())
    }
    
    async fn start_container(&self, name: &str) -> Result<()> {
        self.docker.start_container(name, None::<StartContainerOptions<String>>).await
            .map_err(|e| anyhow!("Failed to start container {}: {}", name, e))?;
        
        info!("Started container: {}", name);
        Ok(())
    }
    
    async fn wait_for_neo4j_health(&self) -> Result<()> {
        info!("Waiting for Neo4j to be ready...");
        
        let mut retries = 30;
        while retries > 0 {
            // Try to connect to Neo4j
            match neo4rs::Graph::new("neo4j://localhost:7687", "neo4j", "knowledge-graph-2024").await {
                Ok(graph) => {
                    // Try a simple query
                    match graph.run(neo4rs::query("RETURN 1")).await {
                        Ok(_) => {
                            info!("Neo4j is ready!");
                            return Ok(());
                        }
                        Err(e) => debug!("Neo4j not ready yet: {}", e)
                    }
                }
                Err(e) => debug!("Cannot connect to Neo4j yet: {}", e)
            }
            
            sleep(Duration::from_secs(2)).await;
            retries -= 1;
        }
        
        Err(anyhow!("Neo4j failed to become ready in time"))
    }
    
    async fn wait_for_redis_health(&self) -> Result<()> {
        info!("Waiting for Redis to be ready...");
        
        let mut retries = 15;
        while retries > 0 {
            // Try to connect to Redis
            match redis::Client::open("redis://127.0.0.1:6379") {
                Ok(client) => {
                    match client.get_connection() {
                        Ok(mut conn) => {
                            use redis::Commands;
                            match conn.ping::<String>() {
                                Ok(_) => {
                                    info!("Redis is ready!");
                                    return Ok(());
                                }
                                Err(e) => debug!("Redis not ready yet: {}", e)
                            }
                        }
                        Err(e) => debug!("Cannot connect to Redis yet: {}", e)
                    }
                }
                Err(e) => debug!("Redis client error: {}", e)
            }
            
            sleep(Duration::from_secs(1)).await;
            retries -= 1;
        }
        
        Err(anyhow!("Redis failed to become ready in time"))
    }
    
    pub async fn stop_containers(&self) -> Result<()> {
        info!("Stopping managed containers...");
        
        for container_name in &[NEO4J_CONTAINER_NAME, REDIS_CONTAINER_NAME] {
            match self.docker.stop_container(container_name, None).await {
                Ok(_) => info!("Stopped container: {}", container_name),
                Err(e) => warn!("Failed to stop container {}: {}", container_name, e)
            }
        }
        
        Ok(())
    }
}
use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn, error, debug};

use crate::knowledge_graph::CodeEntity;

/// Supported embedding models
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EmbeddingModel {
    /// CodeBERT-based model for code understanding
    CodeBERT,
    /// GraphCodeBERT for graph-aware code embeddings
    GraphCodeBERT,
    /// Universal Sentence Encoder for general text
    UniversalSentenceEncoder,
    /// Custom local model
    LocalModel { model_path: String },
}

/// Code embedding representation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeEmbedding {
    pub entity_id: String,
    pub embedding_vector: Vec<f32>,
    pub model_version: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub metadata: HashMap<String, String>,
    pub embedding_type: EmbeddingType,
}

/// Types of embeddings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EmbeddingType {
    /// Code function or method embedding
    Function,
    /// Code class embedding
    Class,
    /// Code file/module embedding
    Module,
    /// Comment or documentation embedding
    Documentation,
    /// Variable or identifier embedding
    Identifier,
    /// Generic code snippet embedding
    Snippet,
}

/// Embedding engine configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingEngineConfig {
    /// Model to use for embeddings
    pub model: EmbeddingModel,
    
    /// Embedding vector dimensions
    pub embedding_dimensions: usize,
    
    /// Maximum text length for embeddings
    pub max_text_length: usize,
    
    /// Enable caching of embeddings
    pub enable_caching: bool,
    
    /// Cache size limit
    pub cache_size_limit: usize,
    
    /// Enable batch processing
    pub enable_batch_processing: bool,
    
    /// Batch size for processing
    pub batch_size: usize,
    
    /// Enable preprocessing optimizations
    pub enable_preprocessing: bool,
    
    /// Model inference timeout (seconds)
    pub inference_timeout_seconds: u64,
}

impl Default for EmbeddingEngineConfig {
    fn default() -> Self {
        Self {
            model: EmbeddingModel::CodeBERT,
            embedding_dimensions: 768,
            max_text_length: 512,
            enable_caching: true,
            cache_size_limit: 10000,
            enable_batch_processing: true,
            batch_size: 32,
            enable_preprocessing: true,
            inference_timeout_seconds: 30,
        }
    }
}

/// Text preprocessing for embeddings
#[derive(Debug, Clone)]
struct TextPreprocessor {
    enable_code_normalization: bool,
    enable_comment_extraction: bool,
    enable_identifier_expansion: bool,
}

impl TextPreprocessor {
    fn new() -> Self {
        Self {
            enable_code_normalization: true,
            enable_comment_extraction: true,
            enable_identifier_expansion: true,
        }
    }
    
    fn preprocess(&self, text: &str, entity_type: &str) -> String {
        let mut processed = text.to_string();
        
        if self.enable_code_normalization {
            processed = self.normalize_code(&processed, entity_type);
        }
        
        if self.enable_comment_extraction {
            processed = self.extract_meaningful_content(&processed, entity_type);
        }
        
        if self.enable_identifier_expansion {
            processed = self.expand_identifiers(&processed);
        }
        
        processed
    }
    
    fn normalize_code(&self, text: &str, entity_type: &str) -> String {
        let mut normalized = text.to_string();
        
        match entity_type {
            "function" | "method" => {
                // Remove function bodies for signature-focused embeddings
                if let Some(body_start) = normalized.find('{') {
                    if let Some(signature_end) = normalized[..body_start].rfind(')') {
                        normalized = normalized[..signature_end + 1].to_string();
                    }
                }
            }
            "class" => {
                // Extract class declaration and key methods
                normalized = self.extract_class_structure(&normalized);
            }
            _ => {}
        }
        
        // Remove excessive whitespace
        normalized = regex::Regex::new(r"\s+")
            .unwrap()
            .replace_all(&normalized, " ")
            .to_string();
        
        normalized
    }
    
    fn extract_meaningful_content(&self, text: &str, entity_type: &str) -> String {
        match entity_type {
            "function" | "method" => {
                // Extract function name, parameters, and docstring
                let mut content = Vec::new();
                
                // Function signature
                if let Some(sig) = self.extract_function_signature(text) {
                    content.push(sig);
                }
                
                // Docstring or comments
                if let Some(doc) = self.extract_docstring(text) {
                    content.push(doc);
                }
                
                content.join(" ")
            }
            "class" => {
                // Extract class name, inheritance, and key methods
                self.extract_class_overview(text)
            }
            _ => text.to_string()
        }
    }
    
    fn expand_identifiers(&self, text: &str) -> String {
        // Expand camelCase and snake_case identifiers
        let camel_case_regex = regex::Regex::new(r"([a-z])([A-Z])").unwrap();
        let snake_case_regex = regex::Regex::new(r"_").unwrap();
        
        let expanded = camel_case_regex.replace_all(text, "$1 $2");
        snake_case_regex.replace_all(&expanded, " ").to_string()
    }
    
    fn extract_class_structure(&self, text: &str) -> String {
        // Simplified class structure extraction
        text.lines()
            .filter(|line| {
                line.trim().starts_with("class ") ||
                line.trim().starts_with("def ") ||
                line.trim().starts_with("fn ") ||
                line.trim().starts_with("function ")
            })
            .collect::<Vec<_>>()
            .join(" ")
    }
    
    fn extract_function_signature(&self, text: &str) -> Option<String> {
        // Extract function signature (simplified)
        text.lines()
            .find(|line| {
                line.trim().starts_with("def ") ||
                line.trim().starts_with("fn ") ||
                line.trim().starts_with("function ")
            })
            .map(|line| line.trim().to_string())
    }
    
    fn extract_docstring(&self, text: &str) -> Option<String> {
        // Extract docstring or leading comments (simplified)
        let lines: Vec<&str> = text.lines().collect();
        let mut docstring_lines = Vec::new();
        
        for line in lines {
            let trimmed = line.trim();
            if trimmed.starts_with("\"\"\"") || trimmed.starts_with("///") || trimmed.starts_with("//") {
                docstring_lines.push(trimmed);
            } else if !docstring_lines.is_empty() && trimmed.is_empty() {
                break;
            }
        }
        
        if docstring_lines.is_empty() {
            None
        } else {
            Some(docstring_lines.join(" "))
        }
    }
    
    fn extract_class_overview(&self, text: &str) -> String {
        // Extract class declaration and method signatures
        let lines: Vec<&str> = text.lines().collect();
        let mut overview = Vec::new();
        
        for line in lines {
            let trimmed = line.trim();
            if trimmed.starts_with("class ") || 
               trimmed.starts_with("def ") ||
               trimmed.starts_with("fn ") {
                overview.push(trimmed);
            }
        }
        
        overview.join(" ")
    }
}

/// Statistics for embedding operations
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EmbeddingStats {
    pub total_embeddings_created: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub average_inference_time_ms: f64,
    pub batch_operations: u64,
    pub preprocessing_time_ms: f64,
    pub total_tokens_processed: u64,
}

/// High-performance embedding engine for code
pub struct EmbeddingEngine {
    config: EmbeddingEngineConfig,
    preprocessor: TextPreprocessor,
    embedding_cache: Arc<RwLock<HashMap<String, CodeEmbedding>>>,
    stats: Arc<RwLock<EmbeddingStats>>,
    
    // Model state (in a real implementation, this would contain the actual model)
    model_loaded: bool,
    model_version: String,
}

impl EmbeddingEngine {
    pub async fn new(config: EmbeddingEngineConfig) -> Result<Self> {
        info!("Initializing embedding engine with model: {:?}", config.model);
        
        // In a real implementation, you would load the actual model here
        let model_version = match &config.model {
            EmbeddingModel::CodeBERT => "microsoft/codebert-base".to_string(),
            EmbeddingModel::GraphCodeBERT => "microsoft/graphcodebert-base".to_string(),
            EmbeddingModel::UniversalSentenceEncoder => "google/universal-sentence-encoder".to_string(),
            EmbeddingModel::LocalModel { model_path } => format!("local:{}", model_path),
        };
        
        Ok(Self {
            config,
            preprocessor: TextPreprocessor::new(),
            embedding_cache: Arc::new(RwLock::new(HashMap::new())),
            stats: Arc::new(RwLock::new(EmbeddingStats::default())),
            model_loaded: true, // Simulated model loading
            model_version,
        })
    }
    
    /// Generate embedding for a code entity
    pub async fn create_embedding(&self, entity: &CodeEntity) -> Result<CodeEmbedding> {
        if !self.model_loaded {
            return Err(anyhow!("Embedding model not loaded"));
        }
        
        let start_time = std::time::Instant::now();
        
        // Check cache first
        if self.config.enable_caching {
            let cache_key = self.generate_cache_key(entity);
            let cache = self.embedding_cache.read().await;
            if let Some(cached_embedding) = cache.get(&cache_key) {
                let mut stats = self.stats.write().await;
                stats.cache_hits += 1;
                debug!("Cache hit for entity: {}", entity.id);
                return Ok(cached_embedding.clone());
            }
        }
        
        // Preprocess the content
        let content = entity.content.as_deref().unwrap_or(&entity.name);
        let processed_text = if self.config.enable_preprocessing {
            self.preprocessor.preprocess(content, &entity.entity_type)
        } else {
            content.to_string()
        };
        
        // Truncate if necessary
        let final_text = if processed_text.len() > self.config.max_text_length {
            processed_text[..self.config.max_text_length].to_string()
        } else {
            processed_text
        };
        
        // Generate embedding vector (simulated)
        let embedding_vector = self.generate_embedding_vector(&final_text, &entity.entity_type).await?;
        
        let embedding_type = self.determine_embedding_type(&entity.entity_type);
        
        let embedding = CodeEmbedding {
            entity_id: entity.id.clone(),
            embedding_vector,
            model_version: self.model_version.clone(),
            created_at: chrono::Utc::now(),
            metadata: self.create_embedding_metadata(entity, &final_text),
            embedding_type,
        };
        
        // Cache the embedding
        if self.config.enable_caching {
            let cache_key = self.generate_cache_key(entity);
            let mut cache = self.embedding_cache.write().await;
            
            // Check cache size limit
            if cache.len() >= self.config.cache_size_limit {
                // Remove oldest entries (simplified LRU)
                let keys_to_remove: Vec<String> = cache.keys().take(cache.len() / 10).cloned().collect();
                for key in keys_to_remove {
                    cache.remove(&key);
                }
            }
            
            cache.insert(cache_key, embedding.clone());
        }
        
        // Update statistics
        let inference_time = start_time.elapsed().as_millis() as f64;
        let mut stats = self.stats.write().await;
        stats.total_embeddings_created += 1;
        stats.cache_misses += 1;
        stats.average_inference_time_ms = 
            (stats.average_inference_time_ms * (stats.total_embeddings_created - 1) as f64 + inference_time) / 
            stats.total_embeddings_created as f64;
        stats.total_tokens_processed += final_text.split_whitespace().count() as u64;
        
        debug!("Created embedding for entity: {} in {:.2}ms", entity.id, inference_time);
        Ok(embedding)
    }
    
    /// Generate embeddings for multiple entities in batch
    pub async fn create_batch_embeddings(&self, entities: &[CodeEntity]) -> Result<Vec<CodeEmbedding>> {
        if !self.config.enable_batch_processing {
            // Process individually
            let mut embeddings = Vec::new();
            for entity in entities {
                embeddings.push(self.create_embedding(entity).await?);
            }
            return Ok(embeddings);
        }
        
        let mut embeddings = Vec::new();
        let chunks: Vec<&[CodeEntity]> = entities.chunks(self.config.batch_size).collect();
        
        for chunk in chunks {
            let batch_embeddings = self.process_batch(chunk).await?;
            embeddings.extend(batch_embeddings);
        }
        
        let mut stats = self.stats.write().await;
        stats.batch_operations += 1;
        
        Ok(embeddings)
    }
    
    /// Get embedding statistics
    pub async fn get_stats(&self) -> EmbeddingStats {
        self.stats.read().await.clone()
    }
    
    /// Clear the embedding cache
    pub async fn clear_cache(&self) {
        let mut cache = self.embedding_cache.write().await;
        cache.clear();
        info!("Embedding cache cleared");
    }
    
    /// Get cache statistics
    pub async fn get_cache_stats(&self) -> (usize, usize) {
        let cache = self.embedding_cache.read().await;
        (cache.len(), self.config.cache_size_limit)
    }
    
    // Private methods
    
    async fn process_batch(&self, entities: &[CodeEntity]) -> Result<Vec<CodeEmbedding>> {
        let mut embeddings = Vec::new();
        
        // In a real implementation, this would use the model's batch processing capabilities
        for entity in entities {
            embeddings.push(self.create_embedding(entity).await?);
        }
        
        Ok(embeddings)
    }
    
    async fn generate_embedding_vector(&self, text: &str, entity_type: &str) -> Result<Vec<f32>> {
        // Simulated embedding generation
        // In a real implementation, this would call the actual model
        
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await; // Simulate inference time
        
        let mut vector = vec![0.0f32; self.config.embedding_dimensions];
        
        // Generate deterministic but varied embeddings based on text content
        let text_hash = self.hash_text(text);
        let type_hash = self.hash_text(entity_type);
        
        for (i, value) in vector.iter_mut().enumerate() {
            let seed = (text_hash.wrapping_add(type_hash).wrapping_add(i as u64)) as f32;
            *value = (seed.sin() * 1000.0) % 2.0 - 1.0; // Value between -1 and 1
        }
        
        // Normalize the vector
        let magnitude = vector.iter().map(|x| x * x).sum::<f32>().sqrt();
        if magnitude > 0.0 {
            for value in &mut vector {
                *value /= magnitude;
            }
        }
        
        Ok(vector)
    }
    
    fn hash_text(&self, text: &str) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut hasher);
        hasher.finish()
    }
    
    fn generate_cache_key(&self, entity: &CodeEntity) -> String {
        format!("{}:{}:{}", entity.id, entity.entity_type, 
                entity.content.as_ref().map(|c| c.len()).unwrap_or(0))
    }
    
    fn determine_embedding_type(&self, entity_type: &str) -> EmbeddingType {
        match entity_type {
            "function" | "method" => EmbeddingType::Function,
            "class" => EmbeddingType::Class,
            "module" | "file" => EmbeddingType::Module,
            "variable" => EmbeddingType::Identifier,
            _ => EmbeddingType::Snippet,
        }
    }
    
    fn create_embedding_metadata(&self, entity: &CodeEntity, processed_text: &str) -> HashMap<String, String> {
        let mut metadata = HashMap::new();
        
        metadata.insert("original_length".to_string(), 
                       entity.content.as_ref().map(|c| c.len()).unwrap_or(0).to_string());
        metadata.insert("processed_length".to_string(), processed_text.len().to_string());
        metadata.insert("entity_type".to_string(), entity.entity_type.clone());
        
        if let Some(file_path) = &entity.file_path {
            metadata.insert("file_path".to_string(), file_path.clone());
        }
        
        // Add language detection
        if let Some(file_path) = &entity.file_path {
            if let Some(extension) = std::path::Path::new(file_path).extension() {
                metadata.insert("language".to_string(), extension.to_string_lossy().to_string());
            }
        }
        
        metadata
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    
    #[tokio::test]
    async fn test_embedding_creation() {
        let config = EmbeddingEngineConfig::default();
        let engine = EmbeddingEngine::new(config).await.unwrap();
        
        let entity = CodeEntity {
            id: "test-function".to_string(),
            name: "test_function".to_string(),
            entity_type: "function".to_string(),
            file_path: Some("test.rs".to_string()),
            content: Some("fn test_function(x: i32) -> i32 { x * 2 }".to_string()),
            metadata: HashMap::new(),
        };
        
        let embedding = engine.create_embedding(&entity).await.unwrap();
        
        assert_eq!(embedding.entity_id, "test-function");
        assert_eq!(embedding.embedding_vector.len(), 768);
        assert!(embedding.embedding_vector.iter().any(|&x| x != 0.0));
    }
    
    #[tokio::test]
    async fn test_embedding_caching() {
        let config = EmbeddingEngineConfig::default();
        let engine = EmbeddingEngine::new(config).await.unwrap();
        
        let entity = CodeEntity {
            id: "test-cached".to_string(),
            name: "cached_function".to_string(),
            entity_type: "function".to_string(),
            file_path: Some("test.rs".to_string()),
            content: Some("fn cached_function() {}".to_string()),
            metadata: HashMap::new(),
        };
        
        // First call - should miss cache
        let embedding1 = engine.create_embedding(&entity).await.unwrap();
        let stats1 = engine.get_stats().await;
        assert_eq!(stats1.cache_misses, 1);
        
        // Second call - should hit cache
        let embedding2 = engine.create_embedding(&entity).await.unwrap();
        let stats2 = engine.get_stats().await;
        assert_eq!(stats2.cache_hits, 1);
        
        assert_eq!(embedding1.embedding_vector, embedding2.embedding_vector);
    }
    
    #[tokio::test]
    async fn test_batch_embedding_creation() {
        let config = EmbeddingEngineConfig::default();
        let engine = EmbeddingEngine::new(config).await.unwrap();
        
        let entities = vec![
            CodeEntity {
                id: "batch-1".to_string(),
                name: "function1".to_string(),
                entity_type: "function".to_string(),
                file_path: Some("test.rs".to_string()),
                content: Some("fn function1() {}".to_string()),
                metadata: HashMap::new(),
            },
            CodeEntity {
                id: "batch-2".to_string(),
                name: "function2".to_string(),
                entity_type: "function".to_string(),
                file_path: Some("test.rs".to_string()),
                content: Some("fn function2() {}".to_string()),
                metadata: HashMap::new(),
            },
        ];
        
        let embeddings = engine.create_batch_embeddings(&entities).await.unwrap();
        
        assert_eq!(embeddings.len(), 2);
        assert_eq!(embeddings[0].entity_id, "batch-1");
        assert_eq!(embeddings[1].entity_id, "batch-2");
        
        let stats = engine.get_stats().await;
        assert_eq!(stats.batch_operations, 1);
    }
}
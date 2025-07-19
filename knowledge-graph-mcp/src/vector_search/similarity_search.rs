use crate::vector_search::embedding_engine::{CodeEmbedding, EmbeddingEngine};
use crate::vector_search::vector_store::{VectorStore, SearchResult};
use crate::error::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimilaritySearchConfig {
    pub default_similarity_threshold: f32,
    pub max_results: usize,
    pub enable_fuzzy_matching: bool,
    pub enable_semantic_boost: bool,
    pub weight_code_structure: f32,
    pub weight_semantic_meaning: f32,
    pub weight_identifier_similarity: f32,
}

impl Default for SimilaritySearchConfig {
    fn default() -> Self {
        Self {
            default_similarity_threshold: 0.7,
            max_results: 50,
            enable_fuzzy_matching: true,
            enable_semantic_boost: true,
            weight_code_structure: 0.4,
            weight_semantic_meaning: 0.4,
            weight_identifier_similarity: 0.2,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SimilarityMetric {
    Cosine,
    Euclidean,
    Manhattan,
    Jaccard,
    Weighted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimilarityResult {
    pub entity_id: String,
    pub file_path: String,
    pub similarity_score: f32,
    pub match_type: MatchType,
    pub context: SimilarityContext,
    pub explanation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MatchType {
    Exact,
    Semantic,
    Structural,
    Fuzzy,
    Hybrid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimilarityContext {
    pub matched_tokens: Vec<String>,
    pub confidence_score: f32,
    pub pattern_matches: Vec<String>,
    pub semantic_category: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchQuery {
    pub text: String,
    pub file_path: Option<String>,
    pub language: Option<String>,
    pub similarity_threshold: Option<f32>,
    pub max_results: Option<usize>,
    pub metric: Option<SimilarityMetric>,
    pub include_context: bool,
    pub boost_recent: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SimilarityStats {
    pub total_searches: u64,
    pub cache_hits: u64,
    pub average_search_time_ms: f64,
    pub most_common_metrics: HashMap<SimilarityMetric, u64>,
    pub accuracy_scores: Vec<f32>,
}

pub struct SimilaritySearchEngine {
    config: SimilaritySearchConfig,
    embedding_engine: Arc<EmbeddingEngine>,
    vector_store: Arc<VectorStore>,
    search_cache: Arc<RwLock<HashMap<String, Vec<SimilarityResult>>>>,
    stats: Arc<RwLock<SimilarityStats>>,
}

impl SimilaritySearchEngine {
    pub fn new(
        config: SimilaritySearchConfig,
        embedding_engine: Arc<EmbeddingEngine>,
        vector_store: Arc<VectorStore>,
    ) -> Self {
        Self {
            config,
            embedding_engine,
            vector_store,
            search_cache: Arc::new(RwLock::new(HashMap::new())),
            stats: Arc::new(RwLock::new(SimilarityStats {
                total_searches: 0,
                cache_hits: 0,
                average_search_time_ms: 0.0,
                most_common_metrics: HashMap::new(),
                accuracy_scores: Vec::new(),
            })),
        }
    }

    pub async fn search(&self, query: SearchQuery) -> Result<Vec<SimilarityResult>> {
        let start_time = std::time::Instant::now();
        
        // Check cache first
        let cache_key = self.generate_cache_key(&query);
        if let Some(cached_results) = self.check_cache(&cache_key).await {
            self.update_cache_hit_stats().await;
            return Ok(cached_results);
        }

        // Generate embedding for query
        let query_embedding = self.embedding_engine
            .generate_embedding(&query.text, query.language.as_deref())
            .await?;

        // Perform similarity search
        let results = match query.metric.unwrap_or(SimilarityMetric::Weighted) {
            SimilarityMetric::Cosine => self.cosine_similarity_search(&query, &query_embedding).await?,
            SimilarityMetric::Euclidean => self.euclidean_similarity_search(&query, &query_embedding).await?,
            SimilarityMetric::Manhattan => self.manhattan_similarity_search(&query, &query_embedding).await?,
            SimilarityMetric::Jaccard => self.jaccard_similarity_search(&query, &query_embedding).await?,
            SimilarityMetric::Weighted => self.weighted_similarity_search(&query, &query_embedding).await?,
        };

        // Cache results
        self.cache_results(&cache_key, &results).await;

        // Update stats
        let search_time = start_time.elapsed().as_millis() as f64;
        self.update_search_stats(&query.metric.unwrap_or(SimilarityMetric::Weighted), search_time).await;

        Ok(results)
    }

    async fn cosine_similarity_search(
        &self,
        query: &SearchQuery,
        query_embedding: &CodeEmbedding,
    ) -> Result<Vec<SimilarityResult>> {
        let search_results = self.vector_store
            .cosine_search(
                &query_embedding.vector,
                query.similarity_threshold.unwrap_or(self.config.default_similarity_threshold),
                query.max_results.unwrap_or(self.config.max_results),
            )
            .await?;

        self.convert_to_similarity_results(search_results, MatchType::Semantic, query).await
    }

    async fn euclidean_similarity_search(
        &self,
        query: &SearchQuery,
        query_embedding: &CodeEmbedding,
    ) -> Result<Vec<SimilarityResult>> {
        let search_results = self.vector_store
            .euclidean_search(
                &query_embedding.vector,
                query.similarity_threshold.unwrap_or(self.config.default_similarity_threshold),
                query.max_results.unwrap_or(self.config.max_results),
            )
            .await?;

        self.convert_to_similarity_results(search_results, MatchType::Structural, query).await
    }

    async fn manhattan_similarity_search(
        &self,
        query: &SearchQuery,
        query_embedding: &CodeEmbedding,
    ) -> Result<Vec<SimilarityResult>> {
        let search_results = self.vector_store
            .manhattan_search(
                &query_embedding.vector,
                query.similarity_threshold.unwrap_or(self.config.default_similarity_threshold),
                query.max_results.unwrap_or(self.config.max_results),
            )
            .await?;

        self.convert_to_similarity_results(search_results, MatchType::Structural, query).await
    }

    async fn jaccard_similarity_search(
        &self,
        query: &SearchQuery,
        query_embedding: &CodeEmbedding,
    ) -> Result<Vec<SimilarityResult>> {
        // For Jaccard, we need to work with token sets
        let query_tokens = self.extract_tokens(&query.text);
        let search_results = self.vector_store
            .jaccard_search(
                &query_tokens,
                query.similarity_threshold.unwrap_or(self.config.default_similarity_threshold),
                query.max_results.unwrap_or(self.config.max_results),
            )
            .await?;

        self.convert_to_similarity_results(search_results, MatchType::Fuzzy, query).await
    }

    async fn weighted_similarity_search(
        &self,
        query: &SearchQuery,
        query_embedding: &CodeEmbedding,
    ) -> Result<Vec<SimilarityResult>> {
        // Combine multiple similarity metrics with weights
        let cosine_results = self.vector_store
            .cosine_search(
                &query_embedding.vector,
                0.5, // Lower threshold for weighted search
                query.max_results.unwrap_or(self.config.max_results) * 2,
            )
            .await?;

        let query_tokens = self.extract_tokens(&query.text);
        let jaccard_results = self.vector_store
            .jaccard_search(
                &query_tokens,
                0.3, // Lower threshold for weighted search
                query.max_results.unwrap_or(self.config.max_results) * 2,
            )
            .await?;

        // Combine and weight results
        let combined_results = self.combine_weighted_results(
            cosine_results,
            jaccard_results,
            query,
        ).await?;

        self.convert_to_similarity_results(combined_results, MatchType::Hybrid, query).await
    }

    async fn combine_weighted_results(
        &self,
        cosine_results: Vec<SearchResult>,
        jaccard_results: Vec<SearchResult>,
        query: &SearchQuery,
    ) -> Result<Vec<SearchResult>> {
        let mut combined_scores: HashMap<String, f32> = HashMap::new();
        let mut all_results: HashMap<String, SearchResult> = HashMap::new();

        // Process cosine results
        for result in cosine_results {
            let weighted_score = result.score * self.config.weight_semantic_meaning;
            combined_scores.insert(result.entity_id.clone(), weighted_score);
            all_results.insert(result.entity_id.clone(), result);
        }

        // Process Jaccard results
        for result in jaccard_results {
            let weighted_score = result.score * self.config.weight_identifier_similarity;
            let entity_id = result.entity_id.clone();
            
            if let Some(existing_score) = combined_scores.get(&entity_id) {
                combined_scores.insert(entity_id.clone(), existing_score + weighted_score);
            } else {
                combined_scores.insert(entity_id.clone(), weighted_score);
                all_results.insert(entity_id, result);
            }
        }

        // Convert back to SearchResult with combined scores
        let mut final_results: Vec<SearchResult> = combined_scores
            .into_iter()
            .filter_map(|(entity_id, score)| {
                if score >= query.similarity_threshold.unwrap_or(self.config.default_similarity_threshold) {
                    all_results.get(&entity_id).map(|result| SearchResult {
                        entity_id: entity_id.clone(),
                        score,
                        metadata: result.metadata.clone(),
                    })
                } else {
                    None
                }
            })
            .collect();

        // Sort by combined score
        final_results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
        final_results.truncate(query.max_results.unwrap_or(self.config.max_results));

        Ok(final_results)
    }

    async fn convert_to_similarity_results(
        &self,
        search_results: Vec<SearchResult>,
        match_type: MatchType,
        query: &SearchQuery,
    ) -> Result<Vec<SimilarityResult>> {
        let mut similarity_results = Vec::new();

        for result in search_results {
            let context = if query.include_context {
                self.generate_similarity_context(&result, &query.text).await?
            } else {
                SimilarityContext {
                    matched_tokens: Vec::new(),
                    confidence_score: result.score,
                    pattern_matches: Vec::new(),
                    semantic_category: None,
                }
            };

            let explanation = self.generate_explanation(&result, &match_type, &context);

            similarity_results.push(SimilarityResult {
                entity_id: result.entity_id,
                file_path: result.metadata.get("file_path")
                    .unwrap_or(&"unknown".to_string())
                    .clone(),
                similarity_score: result.score,
                match_type: match_type.clone(),
                context,
                explanation,
            });
        }

        Ok(similarity_results)
    }

    async fn generate_similarity_context(
        &self,
        result: &SearchResult,
        query_text: &str,
    ) -> Result<SimilarityContext> {
        let query_tokens = self.extract_tokens(query_text);
        let result_text = result.metadata.get("text").unwrap_or(&String::new());
        let result_tokens = self.extract_tokens(result_text);

        let matched_tokens = query_tokens
            .iter()
            .filter(|token| result_tokens.contains(token))
            .cloned()
            .collect();

        let confidence_score = if !query_tokens.is_empty() {
            matched_tokens.len() as f32 / query_tokens.len() as f32
        } else {
            0.0
        };

        let pattern_matches = self.find_pattern_matches(query_text, result_text);
        let semantic_category = self.determine_semantic_category(result_text);

        Ok(SimilarityContext {
            matched_tokens,
            confidence_score,
            pattern_matches,
            semantic_category,
        })
    }

    fn extract_tokens(&self, text: &str) -> Vec<String> {
        text.split_whitespace()
            .filter(|token| token.len() > 2)
            .map(|token| token.to_lowercase())
            .collect()
    }

    fn find_pattern_matches(&self, query: &str, text: &str) -> Vec<String> {
        let mut patterns = Vec::new();
        
        // Simple pattern matching for common code constructs
        if query.contains("function") && text.contains("fn ") {
            patterns.push("function_definition".to_string());
        }
        if query.contains("class") && text.contains("struct ") {
            patterns.push("struct_definition".to_string());
        }
        if query.contains("import") && text.contains("use ") {
            patterns.push("import_statement".to_string());
        }

        patterns
    }

    fn determine_semantic_category(&self, text: &str) -> Option<String> {
        if text.contains("fn ") || text.contains("function") {
            Some("function".to_string())
        } else if text.contains("struct ") || text.contains("class") {
            Some("type_definition".to_string())
        } else if text.contains("use ") || text.contains("import") {
            Some("import".to_string())
        } else if text.contains("test") || text.contains("#[test]") {
            Some("test".to_string())
        } else {
            None
        }
    }

    fn generate_explanation(
        &self,
        result: &SearchResult,
        match_type: &MatchType,
        context: &SimilarityContext,
    ) -> String {
        match match_type {
            MatchType::Exact => "Exact text match found".to_string(),
            MatchType::Semantic => format!(
                "Semantic similarity based on code meaning (confidence: {:.2})",
                context.confidence_score
            ),
            MatchType::Structural => format!(
                "Structural similarity based on code patterns (score: {:.2})",
                result.score
            ),
            MatchType::Fuzzy => format!(
                "Fuzzy match with {} shared tokens",
                context.matched_tokens.len()
            ),
            MatchType::Hybrid => format!(
                "Hybrid match combining semantic and structural analysis (score: {:.2})",
                result.score
            ),
        }
    }

    fn generate_cache_key(&self, query: &SearchQuery) -> String {
        format!(
            "{}:{}:{}:{}:{}",
            query.text,
            query.file_path.as_deref().unwrap_or(""),
            query.language.as_deref().unwrap_or(""),
            query.similarity_threshold.unwrap_or(self.config.default_similarity_threshold),
            query.metric.as_ref().map(|m| format!("{:?}", m)).unwrap_or_default()
        )
    }

    async fn check_cache(&self, cache_key: &str) -> Option<Vec<SimilarityResult>> {
        let cache = self.search_cache.read().await;
        cache.get(cache_key).cloned()
    }

    async fn cache_results(&self, cache_key: &str, results: &[SimilarityResult]) {
        let mut cache = self.search_cache.write().await;
        
        // Simple LRU eviction if cache is too large
        if cache.len() > 1000 {
            let keys_to_remove: Vec<String> = cache.keys().take(100).cloned().collect();
            for key in keys_to_remove {
                cache.remove(&key);
            }
        }
        
        cache.insert(cache_key.to_string(), results.to_vec());
    }

    async fn update_cache_hit_stats(&self) {
        let mut stats = self.stats.write().await;
        stats.cache_hits += 1;
    }

    async fn update_search_stats(&self, metric: &SimilarityMetric, search_time_ms: f64) {
        let mut stats = self.stats.write().await;
        stats.total_searches += 1;
        
        // Update average search time
        let total_time = stats.average_search_time_ms * (stats.total_searches - 1) as f64;
        stats.average_search_time_ms = (total_time + search_time_ms) / stats.total_searches as f64;
        
        // Update metric usage
        *stats.most_common_metrics.entry(metric.clone()).or_insert(0) += 1;
    }

    pub async fn get_stats(&self) -> SimilarityStats {
        self.stats.read().await.clone()
    }

    pub async fn clear_cache(&self) {
        let mut cache = self.search_cache.write().await;
        cache.clear();
    }

    pub async fn find_similar_code(&self, code_snippet: &str, language: Option<&str>) -> Result<Vec<SimilarityResult>> {
        let query = SearchQuery {
            text: code_snippet.to_string(),
            file_path: None,
            language: language.map(|s| s.to_string()),
            similarity_threshold: Some(0.8), // Higher threshold for code similarity
            max_results: Some(10),
            metric: Some(SimilarityMetric::Weighted),
            include_context: true,
            boost_recent: false,
        };

        self.search(query).await
    }

    pub async fn find_related_functions(&self, function_name: &str) -> Result<Vec<SimilarityResult>> {
        let query = SearchQuery {
            text: format!("function {}", function_name),
            file_path: None,
            language: None,
            similarity_threshold: Some(0.6),
            max_results: Some(20),
            metric: Some(SimilarityMetric::Semantic),
            include_context: true,
            boost_recent: false,
        };

        self.search(query).await
    }

    pub async fn search_by_pattern(&self, pattern: &str, file_extension: Option<&str>) -> Result<Vec<SimilarityResult>> {
        let query = SearchQuery {
            text: pattern.to_string(),
            file_path: file_extension.map(|ext| format!("*.{}", ext)),
            language: file_extension.map(|s| s.to_string()),
            similarity_threshold: Some(0.5),
            max_results: Some(50),
            metric: Some(SimilarityMetric::Jaccard),
            include_context: true,
            boost_recent: false,
        };

        self.search(query).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_similarity_search_config() {
        let config = SimilaritySearchConfig::default();
        assert_eq!(config.default_similarity_threshold, 0.7);
        assert_eq!(config.max_results, 50);
    }

    #[tokio::test]
    async fn test_token_extraction() {
        let engine = create_test_engine().await;
        let tokens = engine.extract_tokens("fn test_function() -> Result<()>");
        assert!(tokens.contains(&"test_function()".to_string()));
        assert!(tokens.contains(&"result<()>".to_string()));
    }

    async fn create_test_engine() -> SimilaritySearchEngine {
        // This would be implemented with proper test setup
        todo!("Implement test engine creation")
    }
}
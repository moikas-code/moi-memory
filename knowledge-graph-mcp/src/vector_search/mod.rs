pub mod embedding_engine;
pub mod similarity_search;
pub mod vector_store;
pub mod semantic_analyzer;

// Re-export key types for convenience
pub use embedding_engine::{
    EmbeddingEngine, EmbeddingEngineConfig, CodeEmbedding, EmbeddingModel
};
pub use similarity_search::{
    SimilaritySearchEngine, SimilaritySearchConfig, SearchResult, 
    SimilarityMetric, SearchFilter
};
pub use vector_store::{
    VectorStore, VectorStoreConfig, VectorIndex, IndexedVector
};
pub use semantic_analyzer::{
    SemanticAnalyzer, SemanticAnalyzerConfig, SemanticPattern,
    CodePattern, AnalysisResult
};
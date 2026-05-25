use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use settings_macros::{MergeFrom, with_fallible_options};

/// Configuration for an external AI service endpoint (embedding, reranking, or HyDE).
#[with_fallible_options]
#[derive(Default, Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, MergeFrom)]
pub struct ContextIndexProviderContent {
    /// The base URL for the service (e.g. "http://192.168.1.50:7997").
    pub api_url: Option<String>,
    /// The model identifier to use (e.g. "Qwen/Qwen3-Embedding-4B").
    pub model: Option<String>,
}

/// Configuration for the project-wide context indexer.
///
/// When enabled, the context indexer maintains content hashes and chunks for
/// supported project files so semantic indexing can attach embeddings.
#[with_fallible_options]
#[derive(Default, Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, MergeFrom)]
pub struct ContextIndexSettingsContent {
    /// Whether the context indexer is enabled.
    ///
    /// Default: false
    pub enabled: Option<bool>,

    /// Configuration for the embedding service.
    pub embed: Option<ContextIndexProviderContent>,

    /// Configuration for the reranking service.
    pub rerank: Option<ContextIndexProviderContent>,

    /// Configuration for the HyDE (Hypothetical Document Embeddings) service.
    pub hyde: Option<ContextIndexProviderContent>,

    /// Target chunk size in tokens (~4 chars/token). Adjacent sub-budget
    /// windows are merged up to this limit.
    ///
    /// Default: 1024
    pub chunk_target_tokens: Option<u32>,

    /// Maximum chunk size in tokens. A window is force-split above this.
    ///
    /// Default: 3000
    pub chunk_max_tokens: Option<u32>,

    /// Minimum chunk size in tokens. Chunks below this are dropped
    /// (unless they are the only chunk for a file).
    ///
    /// Default: 64
    pub chunk_min_tokens: Option<u32>,

    /// Minimum doc-comment size in tokens before emitting a separate doc-view
    /// row for a code definition.
    ///
    /// Default: 20
    pub chunk_doc_view_min_tokens: Option<u32>,

    /// Dimensionality of the embedding vectors. Locked at LanceDB table
    /// creation time; changing this after indexing requires a reset.
    ///
    /// Default: 2560
    pub embedding_dim: Option<u32>,

    /// Dense vector candidates to retrieve before fusion.
    ///
    /// Default: 100
    pub ann_top_k: Option<u32>,

    /// BM25/FTS candidates to retrieve before fusion.
    ///
    /// Default: 100
    pub bm25_top_k: Option<u32>,

    /// Reciprocal-rank-fusion constant.
    ///
    /// Default: 60
    pub rrf_k: Option<u32>,

    /// Number of fused candidates requested from the reranker.
    ///
    /// Default: 10
    pub rerank_top_k: Option<u32>,

    /// Number of final results rendered in the UI.
    ///
    /// Default: 5
    pub result_top_k: Option<u32>,

    /// Instruction prepended to user queries for Qwen3 embedding.
    ///
    /// Default: "Given a code search query, retrieve relevant source code that satisfies the query."
    pub query_instruction: Option<String>,

    /// Low-confidence threshold for rerank scores.
    ///
    /// Default: 0.3
    pub confidence_threshold: Option<f32>,
}

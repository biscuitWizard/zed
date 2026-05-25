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
/// When enabled, the context indexer maintains content hashes for all text files
/// in the project and (in a future phase) generates embeddings for semantic search.
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

    /// Dimensionality of the embedding vectors. Locked at LanceDB table
    /// creation time; changing this after indexing requires a reset.
    ///
    /// Default: 2560
    pub embedding_dim: Option<u32>,
}

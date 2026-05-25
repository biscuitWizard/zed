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
}

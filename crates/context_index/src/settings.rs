use settings::RegisterSetting;

pub const DEFAULT_EMBED_MODEL: &str = "Qwen/Qwen3-Embedding-4B";
pub const DEFAULT_RERANK_MODEL: &str = "tomaarsen/Qwen3-Reranker-4B-seq-cls";
pub const DEFAULT_QUERY_INSTRUCTION: &str =
    "Given a code search query, retrieve relevant source code that satisfies the query.";

#[derive(Clone, Debug, RegisterSetting)]
pub struct ContextIndexSettings {
    pub enabled: bool,
    pub embed_api_url: String,
    pub embed_model: String,
    pub rerank_api_url: String,
    pub rerank_model: String,
    pub hyde_api_url: String,
    pub hyde_model: String,
    pub chunk_target_tokens: u32,
    pub chunk_max_tokens: u32,
    pub chunk_min_tokens: u32,
    pub chunk_doc_view_min_tokens: u32,
    pub embedding_dim: u32,
    pub ann_top_k: u32,
    pub bm25_top_k: u32,
    pub rrf_k: u32,
    pub rerank_top_k: u32,
    pub result_top_k: u32,
    pub query_instruction: String,
    pub confidence_threshold: f32,
}

impl settings::Settings for ContextIndexSettings {
    fn from_settings(content: &settings::SettingsContent) -> Self {
        let ci = content.context_index.clone().unwrap_or_default();
        let embed = ci.embed.unwrap_or_default();
        let rerank = ci.rerank.unwrap_or_default();
        let hyde = ci.hyde.unwrap_or_default();

        Self {
            enabled: ci.enabled.unwrap_or(false),
            embed_api_url: embed.api_url.unwrap_or_default(),
            embed_model: non_empty_or_default(embed.model, DEFAULT_EMBED_MODEL),
            rerank_api_url: rerank.api_url.unwrap_or_default(),
            rerank_model: non_empty_or_default(rerank.model, DEFAULT_RERANK_MODEL),
            hyde_api_url: hyde.api_url.unwrap_or_default(),
            hyde_model: hyde.model.unwrap_or_default(),
            chunk_target_tokens: ci.chunk_target_tokens.unwrap_or(1024),
            chunk_max_tokens: ci.chunk_max_tokens.unwrap_or(3000),
            chunk_min_tokens: ci.chunk_min_tokens.unwrap_or(64),
            chunk_doc_view_min_tokens: ci.chunk_doc_view_min_tokens.unwrap_or(20),
            embedding_dim: ci.embedding_dim.unwrap_or(2560),
            ann_top_k: ci.ann_top_k.unwrap_or(100),
            bm25_top_k: ci.bm25_top_k.unwrap_or(100),
            rrf_k: ci.rrf_k.unwrap_or(60),
            rerank_top_k: ci.rerank_top_k.unwrap_or(10),
            result_top_k: ci.result_top_k.unwrap_or(5),
            query_instruction: non_empty_or_default(
                ci.query_instruction,
                DEFAULT_QUERY_INSTRUCTION,
            ),
            confidence_threshold: ci.confidence_threshold.unwrap_or(0.3),
        }
    }
}

fn non_empty_or_default(value: Option<String>, default: &str) -> String {
    value
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| default.to_string())
}

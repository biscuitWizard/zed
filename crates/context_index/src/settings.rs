use settings::RegisterSetting;

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
    pub embedding_dim: u32,
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
            embed_model: embed.model.unwrap_or_default(),
            rerank_api_url: rerank.api_url.unwrap_or_default(),
            rerank_model: rerank.model.unwrap_or_default(),
            hyde_api_url: hyde.api_url.unwrap_or_default(),
            hyde_model: hyde.model.unwrap_or_default(),
            chunk_target_tokens: ci.chunk_target_tokens.unwrap_or(1024),
            chunk_max_tokens: ci.chunk_max_tokens.unwrap_or(3000),
            chunk_min_tokens: ci.chunk_min_tokens.unwrap_or(64),
            embedding_dim: ci.embedding_dim.unwrap_or(2560),
        }
    }
}

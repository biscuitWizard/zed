/// Token-budget knobs expressed in non-whitespace characters.
///
/// The cAST paper uses NWS characters as the budget unit, so we follow that.
/// Multiply token counts by `CHARS_PER_TOKEN` (default 4) to get NWS chars.
#[derive(Clone, Copy, Debug)]
pub struct ChunkBudget {
    /// Greedy-merge target: adjacent windows merge while combined NWS size
    /// stays at or below this.
    pub target: usize,
    /// Hard ceiling per window before a split is forced.
    pub maximum: usize,
    /// Windows below this are dropped (unless it is the only window).
    pub minimum: usize,
    /// Minimum doc-comment size before a separate Doc-view chunk is emitted.
    pub doc_view_min: usize,
}

const CHARS_PER_TOKEN: usize = 4;

impl ChunkBudget {
    pub fn from_token_counts(target_tokens: u32, max_tokens: u32, min_tokens: u32) -> Self {
        Self::from_token_counts_with_doc_view_min(target_tokens, max_tokens, min_tokens, 20)
    }

    pub fn from_token_counts_with_doc_view_min(
        target_tokens: u32,
        max_tokens: u32,
        min_tokens: u32,
        doc_view_min_tokens: u32,
    ) -> Self {
        Self {
            target: target_tokens as usize * CHARS_PER_TOKEN,
            maximum: max_tokens as usize * CHARS_PER_TOKEN,
            minimum: min_tokens as usize * CHARS_PER_TOKEN,
            doc_view_min: doc_view_min_tokens as usize * CHARS_PER_TOKEN,
        }
    }
}

impl Default for ChunkBudget {
    fn default() -> Self {
        Self::from_token_counts(1024, 3000, 64)
    }
}

/// Non-whitespace character count — tokenizer-free size proxy.
pub fn nws_size(text: &str) -> usize {
    text.chars().filter(|c| !c.is_whitespace()).count()
}

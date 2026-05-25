use std::time::Instant;

use anyhow::Result;
use collections::{HashMap, HashSet};
use futures::channel::mpsc::UnboundedSender;

use crate::{
    embed::{EmbedClient, RerankClient},
    expansion::{ExpandedContext, expand_context},
    settings::ContextIndexSettings,
    store::{ChunkRow, LanceStore},
};

#[derive(Clone, Debug)]
pub struct RetrievalToggles {
    pub result_top_k: u32,
    pub ann_top_k: u32,
    pub bm25_top_k: u32,
    pub rrf_k: u32,
    pub rerank_top_k: u32,
    pub rerank: bool,
    pub bm25: bool,
    pub doc_views: bool,
    pub confidence_threshold: f32,
}

impl RetrievalToggles {
    pub fn from_settings(settings: &ContextIndexSettings) -> Self {
        Self {
            result_top_k: settings.result_top_k,
            ann_top_k: settings.ann_top_k,
            bm25_top_k: settings.bm25_top_k,
            rrf_k: settings.rrf_k,
            rerank_top_k: settings.rerank_top_k,
            rerank: true,
            bm25: true,
            doc_views: true,
            confidence_threshold: settings.confidence_threshold,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub rank: u32,
    pub score_vector: Option<f32>,
    pub score_bm25_rank: Option<u32>,
    pub score_rerank: Option<f32>,
    pub doc_view_hit: bool,
    pub low_confidence: bool,
    pub chunk: ChunkRow,
    pub expanded: ExpandedContext,
}

#[derive(Debug, Clone)]
pub enum RenderEvent {
    Header {
        query: String,
    },
    Stage {
        label: &'static str,
        detail: String,
    },
    Result {
        block_markdown: String,
    },
    Footer {
        count: usize,
        low_confidence: bool,
        elapsed_ms: u128,
    },
}

#[derive(Clone)]
struct Candidate {
    row: ChunkRow,
    vec_distance: Option<f32>,
    bm25_rank: Option<u32>,
    rrf_score: f32,
    views_seen: HashSet<String>,
}

pub async fn search(
    store: &LanceStore,
    embed: &EmbedClient,
    rerank: &RerankClient,
    query: &str,
    toggles: &RetrievalToggles,
) -> Result<Vec<SearchResult>> {
    search_inner(store, embed, rerank, query, toggles, None).await
}

pub async fn search_streaming(
    store: &LanceStore,
    embed: &EmbedClient,
    rerank: &RerankClient,
    query: &str,
    toggles: &RetrievalToggles,
    events: UnboundedSender<RenderEvent>,
) -> Result<Vec<SearchResult>> {
    let _ = events.unbounded_send(RenderEvent::Header {
        query: query.to_string(),
    });
    let started = Instant::now();
    let results = search_inner(store, embed, rerank, query, toggles, Some(&events)).await?;
    let low_confidence = results.iter().any(|result| result.low_confidence);
    let _ = events.unbounded_send(RenderEvent::Footer {
        count: results.len(),
        low_confidence,
        elapsed_ms: started.elapsed().as_millis(),
    });
    Ok(results)
}

async fn search_inner(
    store: &LanceStore,
    embed: &EmbedClient,
    rerank: &RerankClient,
    query: &str,
    toggles: &RetrievalToggles,
    events: Option<&UnboundedSender<RenderEvent>>,
) -> Result<Vec<SearchResult>> {
    let started = Instant::now();
    emit_stage(events, "Embedding query", "started".to_string());
    let query_vec = embed.embed_query(query).await?;
    emit_stage(
        events,
        "Embedding query",
        format!(
            "done ({}-d, {} ms)",
            query_vec.len(),
            started.elapsed().as_millis()
        ),
    );

    let view_filter = (!toggles.doc_views).then_some("code");
    let ann_started = Instant::now();
    let ann = store
        .vector_search(&query_vec, toggles.ann_top_k as usize, view_filter)
        .await?;
    emit_stage(
        events,
        "ANN search",
        format!(
            "{} results ({} ms)",
            ann.len(),
            ann_started.elapsed().as_millis()
        ),
    );

    let bm25_started = Instant::now();
    let bm25 = if toggles.bm25 && toggles.bm25_top_k > 0 {
        match store
            .fts_search(query, toggles.bm25_top_k as usize, view_filter)
            .await
        {
            Ok(rows) => rows,
            Err(err) => {
                log::warn!("[context_index] BM25 search failed: {err}");
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };
    emit_stage(
        events,
        "BM25 search",
        format!(
            "{} results ({} ms)",
            bm25.len(),
            bm25_started.elapsed().as_millis()
        ),
    );

    let fused = fuse(ann, bm25, toggles.rrf_k);
    let mut candidates = ensure_canonical(store, fused).await?;
    candidates.truncate((toggles.rerank_top_k.max(toggles.result_top_k * 4)) as usize);
    emit_stage(events, "Fusion", format!("{} candidates", candidates.len()));

    if candidates.is_empty() {
        return Ok(Vec::new());
    }

    let mut rerank_scores = HashMap::default();
    if toggles.rerank && candidates.len() > 1 {
        let rerank_started = Instant::now();
        let documents = candidates
            .iter()
            .map(|candidate| candidate.row.code_text.clone())
            .collect::<Vec<_>>();
        match rerank
            .rerank(query, &documents, toggles.rerank_top_k as usize)
            .await
        {
            Ok(results) => {
                for result in results {
                    if let Some(candidate) = candidates.get(result.index) {
                        rerank_scores.insert(candidate.row.id.clone(), result.score);
                    }
                }
                emit_stage(
                    events,
                    "Reranking",
                    format!(
                        "done ({} candidates, {} ms)",
                        documents.len(),
                        rerank_started.elapsed().as_millis()
                    ),
                );
            }
            Err(err) => {
                log::warn!("[context_index] rerank failed, using RRF order: {err}");
                emit_stage(events, "Reranking", "failed; using RRF order".to_string());
            }
        }
    }

    if !rerank_scores.is_empty() {
        candidates.sort_by(|a, b| {
            let a_score = rerank_scores
                .get(&a.row.id)
                .copied()
                .unwrap_or(f32::NEG_INFINITY);
            let b_score = rerank_scores
                .get(&b.row.id)
                .copied()
                .unwrap_or(f32::NEG_INFINITY);
            b_score
                .total_cmp(&a_score)
                .then_with(|| b.rrf_score.total_cmp(&a.rrf_score))
        });
    }

    let max_rerank = rerank_scores.values().copied().reduce(f32::max);
    let low_confidence = max_rerank
        .map(|score| score < toggles.confidence_threshold)
        .unwrap_or(false);

    let mut results = Vec::new();
    for (idx, candidate) in candidates
        .into_iter()
        .take(toggles.result_top_k as usize)
        .enumerate()
    {
        let expanded = expand_context(store, candidate.row.clone()).await?;
        let result = SearchResult {
            rank: idx as u32 + 1,
            score_vector: candidate.vec_distance.map(|distance| 1.0 - distance),
            score_bm25_rank: candidate.bm25_rank,
            score_rerank: rerank_scores.get(&candidate.row.id).copied(),
            doc_view_hit: candidate.views_seen.contains("doc"),
            low_confidence,
            chunk: candidate.row,
            expanded,
        };
        if let Some(events) = events {
            let _ = events.unbounded_send(RenderEvent::Result {
                block_markdown: format_result_markdown(&result),
            });
        }
        results.push(result);
    }

    Ok(results)
}

fn fuse(ann: Vec<ChunkRow>, bm25: Vec<ChunkRow>, rrf_k: u32) -> HashMap<String, Candidate> {
    let mut fused = HashMap::default();

    for (rank, row) in ann.into_iter().enumerate() {
        let rank = rank as u32 + 1;
        let candidate = fused
            .entry(row.node_id.clone())
            .or_insert_with(|| Candidate {
                row: row.clone(),
                vec_distance: None,
                bm25_rank: None,
                rrf_score: 0.0,
                views_seen: HashSet::default(),
            });
        candidate.views_seen.insert(row.view.clone());
        if row.view == "code" {
            candidate.row = row.clone();
        }
        candidate.vec_distance = Some(
            candidate
                .vec_distance
                .map(|distance| distance.min(row.score))
                .unwrap_or(row.score),
        );
        candidate.rrf_score += rrf_score(rank, rrf_k);
    }

    for (rank, row) in bm25.into_iter().enumerate() {
        let rank = rank as u32 + 1;
        let candidate = fused
            .entry(row.node_id.clone())
            .or_insert_with(|| Candidate {
                row: row.clone(),
                vec_distance: None,
                bm25_rank: None,
                rrf_score: 0.0,
                views_seen: HashSet::default(),
            });
        candidate.views_seen.insert(row.view.clone());
        if row.view == "code" {
            candidate.row = row;
        }
        candidate.bm25_rank = Some(
            candidate
                .bm25_rank
                .map(|prev| prev.min(rank))
                .unwrap_or(rank),
        );
        candidate.rrf_score += rrf_score(rank, rrf_k);
    }

    fused
}

async fn ensure_canonical(
    store: &LanceStore,
    mut fused: HashMap<String, Candidate>,
) -> Result<Vec<Candidate>> {
    let node_ids = fused
        .iter()
        .filter_map(|(node_id, candidate)| {
            (candidate.row.view != "code").then_some(node_id.clone())
        })
        .collect::<Vec<_>>();
    let canonical = store.get_canonical_by_node_ids(&node_ids).await?;
    for (node_id, row) in canonical {
        if let Some(candidate) = fused.get_mut(&node_id) {
            candidate.row = row;
        }
    }

    let mut candidates = fused.into_values().collect::<Vec<_>>();
    candidates.sort_by(|a, b| b.rrf_score.total_cmp(&a.rrf_score));
    Ok(candidates)
}

fn rrf_score(rank: u32, k: u32) -> f32 {
    1.0 / (k + rank) as f32
}

fn emit_stage(events: Option<&UnboundedSender<RenderEvent>>, label: &'static str, detail: String) {
    if let Some(events) = events {
        let _ = events.unbounded_send(RenderEvent::Stage { label, detail });
    }
}

pub fn format_result_markdown(result: &SearchResult) -> String {
    let rerank = result
        .score_rerank
        .map(|score| format!("rerank={score:.3}"))
        .unwrap_or_else(|| "rerank=-".to_string());
    let vector = result
        .score_vector
        .map(|score| format!("vec={score:.3}"))
        .unwrap_or_else(|| "vec=-".to_string());
    let bm25 = result
        .score_bm25_rank
        .map(|rank| format!("bm25=#{rank}"))
        .unwrap_or_else(|| "bm25=-".to_string());
    let doc = if result.doc_view_hit { " doc-view" } else { "" };
    let language = markdown_language(&result.chunk.language_id);

    format!(
        "\n---\n\n## #{}  {rerank}  {vector}  {bm25}{doc}\n\n[`{}:{}-{}`]({})  ·  `{}`\n\n```{}\n{}```\n",
        result.rank,
        result.chunk.file_path,
        result.chunk.line_start + 1,
        result.chunk.line_end + 1,
        result.chunk.file_path,
        result.chunk.breadcrumb,
        language,
        result.expanded.skeleton,
    )
}

fn markdown_language(language_id: &str) -> &'static str {
    match language_id {
        "csharp" => "csharp",
        "go" => "go",
        "javascript" | "javascriptreact" => "javascript",
        "markdown" => "markdown",
        "python" => "python",
        "rust" => "rust",
        "typescript" => "typescript",
        _ => "text",
    }
}

use std::{sync::Arc, time::Duration};

use anyhow::{Context as _, Result, anyhow, bail};
use futures::AsyncReadExt as _;
use http_client::{AsyncBody, HttpClient, Method, Request};
use serde::{Deserialize, Serialize};

const EMBED_BATCH_SIZE: usize = 64;
const MAX_RETRIES: usize = 3;
const RETRY_BACKOFF: Duration = Duration::from_millis(1500);

#[derive(Clone)]
pub struct EmbedClient {
    http_client: Arc<dyn HttpClient>,
    base_url: String,
    model: String,
    query_instruction: String,
    api_key: Option<String>,
}

#[derive(Clone)]
pub struct RerankClient {
    http_client: Arc<dyn HttpClient>,
    base_url: String,
    model: String,
    api_key: Option<String>,
}

#[derive(Clone, Copy, Debug)]
pub struct RerankResult {
    pub index: usize,
    pub score: f32,
}

impl EmbedClient {
    pub fn new(
        http_client: Arc<dyn HttpClient>,
        base_url: impl Into<String>,
        model: impl Into<String>,
        query_instruction: impl Into<String>,
        api_key: Option<String>,
    ) -> Self {
        Self {
            http_client,
            base_url: trim_base_url(base_url.into()),
            model: model.into(),
            query_instruction: query_instruction.into(),
            api_key,
        }
    }

    pub async fn embed_documents(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let mut vectors = Vec::with_capacity(texts.len());
        for batch in texts.chunks(EMBED_BATCH_SIZE) {
            let response = self
                .post_embedding_with_fallback(batch)
                .await
                .context("embedding document batch")?;
            vectors.extend(response.into_vectors()?);
        }
        Ok(vectors)
    }

    pub async fn embed_query(&self, query: &str) -> Result<Vec<f32>> {
        let formatted = format!("Instruct: {}\nQuery: {query}", self.query_instruction);
        let response = self
            .post_embedding_with_fallback(&[formatted])
            .await
            .context("embedding query")?;
        response
            .into_vectors()?
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("embedding response contained no vectors"))
    }

    async fn post_embedding_with_fallback(&self, input: &[String]) -> Result<EmbeddingResponse> {
        let payload = EmbeddingRequest {
            model: &self.model,
            input,
        };

        match self.post_json("/embeddings", &payload).await {
            Ok(response) => Ok(response),
            Err(first_err) => match self.post_json("/v1/embeddings", &payload).await {
                Ok(response) => Ok(response),
                Err(second_err) => Err(second_err).with_context(|| {
                    format!("embedding fallback failed after /embeddings failed: {first_err}")
                }),
            },
        }
    }

    async fn post_json<T, R>(&self, path: &str, payload: &T) -> Result<R>
    where
        T: Serialize + ?Sized,
        R: for<'de> Deserialize<'de>,
    {
        post_json_with_retries(
            self.http_client.clone(),
            &self.base_url,
            path,
            self.api_key.as_deref(),
            payload,
        )
        .await
    }
}

impl RerankClient {
    pub fn new(
        http_client: Arc<dyn HttpClient>,
        base_url: impl Into<String>,
        model: impl Into<String>,
        api_key: Option<String>,
    ) -> Self {
        Self {
            http_client,
            base_url: trim_base_url(base_url.into()),
            model: model.into(),
            api_key,
        }
    }

    pub async fn rerank(
        &self,
        query: &str,
        documents: &[String],
        top_n: usize,
    ) -> Result<Vec<RerankResult>> {
        if documents.is_empty() {
            return Ok(Vec::new());
        }

        let payload = RerankRequest {
            model: &self.model,
            query,
            documents,
            return_documents: false,
            top_n,
        };
        let response: RerankResponse = post_json_with_retries(
            self.http_client.clone(),
            &self.base_url,
            "/rerank",
            self.api_key.as_deref(),
            &payload,
        )
        .await
        .context("reranking candidate documents")?;

        let mut results = response
            .results
            .into_iter()
            .filter_map(|item| {
                let score = item.relevance_score.or(item.score)?;
                Some(RerankResult {
                    index: item.index,
                    score,
                })
            })
            .collect::<Vec<_>>();
        results.sort_by(|a, b| b.score.total_cmp(&a.score));
        Ok(results)
    }
}

async fn post_json_with_retries<T, R>(
    http_client: Arc<dyn HttpClient>,
    base_url: &str,
    path: &str,
    api_key: Option<&str>,
    payload: &T,
) -> Result<R>
where
    T: Serialize + ?Sized,
    R: for<'de> Deserialize<'de>,
{
    let mut last_err = None;
    for attempt in 0..MAX_RETRIES {
        match post_json_once::<T, R>(http_client.clone(), base_url, path, api_key, payload).await {
            Ok(response) => return Ok(response),
            Err(err) => {
                last_err = Some(err);
                if attempt + 1 < MAX_RETRIES {
                    tokio::time::sleep(RETRY_BACKOFF * (1 << attempt)).await;
                }
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow!("request failed without an error")))
}

async fn post_json_once<T, R>(
    http_client: Arc<dyn HttpClient>,
    base_url: &str,
    path: &str,
    api_key: Option<&str>,
    payload: &T,
) -> Result<R>
where
    T: Serialize + ?Sized,
    R: for<'de> Deserialize<'de>,
{
    let url = format!("{base_url}{path}");
    let mut request = Request::builder()
        .method(Method::POST)
        .uri(url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json");

    if let Some(api_key) = api_key.filter(|key| !key.trim().is_empty()) {
        request = request.header("Authorization", format!("Bearer {}", api_key.trim()));
    }

    let body = serde_json::to_string(payload).context("serializing request payload")?;
    let mut response = http_client
        .send(request.body(AsyncBody::from(body))?)
        .await?;
    let status = response.status();
    let mut body = String::new();
    response.body_mut().read_to_string(&mut body).await?;

    if !status.is_success() {
        bail!("request failed: {status}; body: {body}");
    }

    serde_json::from_str(&body).with_context(|| format!("parsing response body: {body}"))
}

fn trim_base_url(url: String) -> String {
    url.trim_end_matches('/').to_string()
}

#[derive(Serialize)]
struct EmbeddingRequest<'a> {
    model: &'a str,
    input: &'a [String],
}

#[derive(Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingData>,
}

impl EmbeddingResponse {
    fn into_vectors(self) -> Result<Vec<Vec<f32>>> {
        let vectors = self
            .data
            .into_iter()
            .map(|item| item.embedding)
            .collect::<Vec<_>>();
        if vectors.iter().any(|vector| vector.is_empty()) {
            bail!("embedding response contained an empty vector");
        }
        Ok(vectors)
    }
}

#[derive(Deserialize)]
struct EmbeddingData {
    embedding: Vec<f32>,
}

#[derive(Serialize)]
struct RerankRequest<'a> {
    model: &'a str,
    query: &'a str,
    documents: &'a [String],
    return_documents: bool,
    top_n: usize,
}

#[derive(Deserialize)]
struct RerankResponse {
    #[serde(default, alias = "data")]
    results: Vec<RerankResponseItem>,
}

#[derive(Deserialize)]
struct RerankResponseItem {
    #[serde(alias = "idx")]
    index: usize,
    relevance_score: Option<f32>,
    score: Option<f32>,
}

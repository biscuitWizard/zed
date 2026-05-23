use std::sync::Arc;

use anyhow::{Context as _, Result, anyhow};
use cloud_llm_client::{WebSearchResponse, WebSearchResult};
use credentials_provider::CredentialsProvider;
use futures::AsyncReadExt as _;
use gpui::{App, AppContext as _, Task};
use http_client::{AsyncBody, HttpClient, Method, Request};
use serde::{Deserialize, Serialize};
use web_search::{WebSearchProvider, WebSearchProviderId};

pub const EXA_WEB_SEARCH_PROVIDER_ID: &str = "exa.ai";
pub const EXA_API_URL: &str = "https://api.exa.ai";
const EXA_SEARCH_PATH: &str = "/search";
const EXA_DEFAULT_NUM_RESULTS: usize = 10;
const EXA_DEFAULT_TEXT_MAX_CHARACTERS: usize = 1000;

pub struct ExaWebSearchProvider {
    http_client: Arc<dyn HttpClient>,
    credentials_provider: Arc<dyn CredentialsProvider>,
}

impl ExaWebSearchProvider {
    pub fn new(
        http_client: Arc<dyn HttpClient>,
        credentials_provider: Arc<dyn CredentialsProvider>,
    ) -> Self {
        Self {
            http_client,
            credentials_provider,
        }
    }
}

impl WebSearchProvider for ExaWebSearchProvider {
    fn id(&self) -> WebSearchProviderId {
        WebSearchProviderId(EXA_WEB_SEARCH_PROVIDER_ID.into())
    }

    fn search(&self, query: String, cx: &mut App) -> Task<Result<WebSearchResponse>> {
        let http_client = self.http_client.clone();
        let credentials_provider = self.credentials_provider.clone();
        cx.spawn(async move |cx| {
            let credentials = credentials_provider
                .read_credentials(EXA_API_URL, &cx)
                .await
                .context("failed to read Exa API key from system keychain")?;
            let Some((_, api_key_bytes)) = credentials else {
                return Err(anyhow!(
                    "Exa API key not configured. Add one in the Anthropic provider settings or set the EXA_API_KEY environment variable."
                ));
            };
            let api_key = String::from_utf8(api_key_bytes)
                .context("stored Exa API key is not valid UTF-8")?;

            cx.background_spawn(perform_exa_search(http_client, api_key, query))
                .await
        })
    }
}

async fn perform_exa_search(
    http_client: Arc<dyn HttpClient>,
    api_key: String,
    query: String,
) -> Result<WebSearchResponse> {
    let body = ExaSearchRequest {
        query,
        num_results: EXA_DEFAULT_NUM_RESULTS,
        contents: ExaContents {
            text: ExaText {
                max_characters: EXA_DEFAULT_TEXT_MAX_CHARACTERS,
            },
        },
    };
    let body_json = serde_json::to_string(&body)?;

    let url = format!("{EXA_API_URL}{EXA_SEARCH_PATH}");
    let request = Request::builder()
        .method(Method::POST)
        .uri(url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .header("x-api-key", api_key)
        .body(AsyncBody::from(body_json))?;

    let mut response = http_client.send(request).await?;
    let status = response.status();
    let mut body = String::new();
    response.body_mut().read_to_string(&mut body).await?;

    if !status.is_success() {
        anyhow::bail!("Exa web search failed.\nStatus: {status:?}\nBody: {body}");
    }

    let parsed: ExaSearchResponse = serde_json::from_str(&body)
        .with_context(|| format!("failed to parse Exa search response: {body}"))?;

    Ok(WebSearchResponse {
        results: parsed
            .results
            .into_iter()
            .map(|result| WebSearchResult {
                title: result.title.unwrap_or_default(),
                url: result.url,
                text: result.text.unwrap_or_default(),
            })
            .collect(),
    })
}

#[derive(Serialize)]
struct ExaSearchRequest {
    query: String,
    #[serde(rename = "numResults")]
    num_results: usize,
    contents: ExaContents,
}

#[derive(Serialize)]
struct ExaContents {
    text: ExaText,
}

#[derive(Serialize)]
struct ExaText {
    #[serde(rename = "maxCharacters")]
    max_characters: usize,
}

#[derive(Deserialize)]
struct ExaSearchResponse {
    #[serde(default)]
    results: Vec<ExaResult>,
}

#[derive(Deserialize)]
struct ExaResult {
    url: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    text: Option<String>,
}

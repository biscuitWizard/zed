use credentials_provider::CredentialsProvider;
use gpui::{App, AppContext as _, Entity, Global, SharedString};
use language_model::{ApiKeyState, EnvVar, env_var};
use std::sync::Arc;

use crate::settings::ContextIndexSettings;
use settings::Settings as _;

static EMBED_API_KEY_ENV_VAR: std::sync::LazyLock<EnvVar> = env_var!("CONTEXT_INDEX_EMBED_API_KEY");
static RERANK_API_KEY_ENV_VAR: std::sync::LazyLock<EnvVar> =
    env_var!("CONTEXT_INDEX_RERANK_API_KEY");
static HYDE_API_KEY_ENV_VAR: std::sync::LazyLock<EnvVar> = env_var!("CONTEXT_INDEX_HYDE_API_KEY");

const DEFAULT_EMBED_URL: &str = "http://192.168.1.50:7997";
const DEFAULT_RERANK_URL: &str = "http://192.168.1.50:7997";
const DEFAULT_HYDE_URL: &str = "http://localhost:7997";

struct GlobalEmbedApiKey(Entity<ApiKeyState>);
impl Global for GlobalEmbedApiKey {}

struct GlobalRerankApiKey(Entity<ApiKeyState>);
impl Global for GlobalRerankApiKey {}

struct GlobalHydeApiKey(Entity<ApiKeyState>);
impl Global for GlobalHydeApiKey {}

pub fn embed_api_url(cx: &App) -> SharedString {
    let url = &ContextIndexSettings::get_global(cx).embed_api_url;
    if url.is_empty() {
        DEFAULT_EMBED_URL.into()
    } else {
        SharedString::from(url.clone())
    }
}

pub fn rerank_api_url(cx: &App) -> SharedString {
    let url = &ContextIndexSettings::get_global(cx).rerank_api_url;
    if url.is_empty() {
        DEFAULT_RERANK_URL.into()
    } else {
        SharedString::from(url.clone())
    }
}

pub fn hyde_api_url(cx: &App) -> SharedString {
    let url = &ContextIndexSettings::get_global(cx).hyde_api_url;
    if url.is_empty() {
        DEFAULT_HYDE_URL.into()
    } else {
        SharedString::from(url.clone())
    }
}

pub fn embed_api_key_state(cx: &mut App) -> Entity<ApiKeyState> {
    if let Some(global) = cx.try_global::<GlobalEmbedApiKey>() {
        return global.0.clone();
    }
    let url: SharedString = embed_api_url(cx);
    let entity = cx.new(|_| ApiKeyState::new(url, EMBED_API_KEY_ENV_VAR.clone()));
    cx.set_global(GlobalEmbedApiKey(entity.clone()));
    entity
}

pub fn rerank_api_key_state(cx: &mut App) -> Entity<ApiKeyState> {
    if let Some(global) = cx.try_global::<GlobalRerankApiKey>() {
        return global.0.clone();
    }
    let url: SharedString = rerank_api_url(cx);
    let entity = cx.new(|_| ApiKeyState::new(url, RERANK_API_KEY_ENV_VAR.clone()));
    cx.set_global(GlobalRerankApiKey(entity.clone()));
    entity
}

pub fn hyde_api_key_state(cx: &mut App) -> Entity<ApiKeyState> {
    if let Some(global) = cx.try_global::<GlobalHydeApiKey>() {
        return global.0.clone();
    }
    let url: SharedString = hyde_api_url(cx);
    let entity = cx.new(|_| ApiKeyState::new(url, HYDE_API_KEY_ENV_VAR.clone()));
    cx.set_global(GlobalHydeApiKey(entity.clone()));
    entity
}

pub fn credentials_provider(cx: &App) -> Arc<dyn CredentialsProvider> {
    zed_credentials_provider::global(cx)
}

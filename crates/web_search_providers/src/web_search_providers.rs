mod cloud;
mod exa;

use client::{Client, UserStore};
use credentials_provider::CredentialsProvider;
use gpui::{App, Context, Entity};
use http_client::HttpClient;
use language_model::{ANTHROPIC_PROVIDER_ID, LanguageModelRegistry};
use std::sync::Arc;
use web_search::{WebSearchProviderId, WebSearchRegistry};

pub fn init(client: Arc<Client>, user_store: Entity<UserStore>, cx: &mut App) {
    let registry = WebSearchRegistry::global(cx);
    let credentials_provider = client.credentials_provider();
    let http_client: Arc<dyn HttpClient> = client.http_client();
    registry.update(cx, |registry, cx| {
        register_web_search_providers(
            registry,
            client,
            user_store,
            http_client,
            credentials_provider,
            cx,
        );
    });
}

fn register_web_search_providers(
    registry: &mut WebSearchRegistry,
    client: Arc<Client>,
    user_store: Entity<UserStore>,
    http_client: Arc<dyn HttpClient>,
    credentials_provider: Arc<dyn CredentialsProvider>,
    cx: &mut Context<WebSearchRegistry>,
) {
    let language_model_registry = LanguageModelRegistry::global(cx);
    register_zed_web_search_provider(
        registry,
        client.clone(),
        user_store.clone(),
        &language_model_registry,
        cx,
    );
    register_exa_web_search_provider(
        registry,
        http_client.clone(),
        credentials_provider.clone(),
        &language_model_registry,
        cx,
    );

    cx.subscribe(
        &LanguageModelRegistry::global(cx),
        move |this, registry, event, cx| {
            if let language_model::Event::DefaultModelChanged = event {
                register_zed_web_search_provider(
                    this,
                    client.clone(),
                    user_store.clone(),
                    &registry,
                    cx,
                );
                register_exa_web_search_provider(
                    this,
                    http_client.clone(),
                    credentials_provider.clone(),
                    &registry,
                    cx,
                );
            }
        },
    )
    .detach();
}

fn register_zed_web_search_provider(
    registry: &mut WebSearchRegistry,
    client: Arc<Client>,
    user_store: Entity<UserStore>,
    language_model_registry: &Entity<LanguageModelRegistry>,
    cx: &mut Context<WebSearchRegistry>,
) {
    let using_zed_provider = language_model_registry
        .read(cx)
        .default_model()
        .is_some_and(|default| default.is_provided_by_zed());
    if using_zed_provider {
        registry.register_provider(
            cloud::CloudWebSearchProvider::new(client, user_store, cx),
            cx,
        )
    } else {
        registry.unregister_provider(WebSearchProviderId(
            cloud::ZED_WEB_SEARCH_PROVIDER_ID.into(),
        ));
    }
}

fn register_exa_web_search_provider(
    registry: &mut WebSearchRegistry,
    http_client: Arc<dyn HttpClient>,
    credentials_provider: Arc<dyn CredentialsProvider>,
    language_model_registry: &Entity<LanguageModelRegistry>,
    cx: &mut Context<WebSearchRegistry>,
) {
    let using_anthropic_provider = language_model_registry
        .read(cx)
        .default_model()
        .is_some_and(|default| default.provider.id() == ANTHROPIC_PROVIDER_ID);
    if using_anthropic_provider {
        registry.register_provider(
            exa::ExaWebSearchProvider::new(http_client, credentials_provider),
            cx,
        )
    } else {
        registry.unregister_provider(WebSearchProviderId(
            exa::EXA_WEB_SEARCH_PROVIDER_ID.into(),
        ));
    }
}

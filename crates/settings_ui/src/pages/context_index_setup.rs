use context_index::api_keys::{
    credentials_provider, embed_api_key_state, embed_api_url, hyde_api_key_state, hyde_api_url,
    rerank_api_key_state, rerank_api_url,
};
use gpui::{App, Entity, ScrollHandle, SharedString, TaskExt, prelude::*};
use language_model::ApiKeyState;
use ui::{ConfiguredApiCard, prelude::*};

use crate::{
    SettingField, SettingItem, SettingsFieldMetadata, SettingsPageItem, SettingsWindow, USER,
    components::{SettingsInputField, SettingsSectionHeader},
};

pub(crate) fn render_context_index_setup_page(
    settings_window: &SettingsWindow,
    scroll_handle: &ScrollHandle,
    window: &mut Window,
    cx: &mut Context<SettingsWindow>,
) -> AnyElement {
    let border_color = cx.theme().colors().border_variant;

    v_flex()
        .id("context-index-setup-page")
        .size_full()
        .overflow_y_scroll()
        .track_scroll(scroll_handle)
        .pt_2p5()
        .px_8()
        .pb_16()
        .gap_4()
        .child(
            v_flex()
                .gap_2()
                .child(Label::new("Context Indexer").size(LabelSize::Large))
                .child(
                    Label::new(
                        "The context indexer maintains content hashes for all text files in your project. \
                         Configure the AI service endpoints below for embedding, reranking, and HyDE.",
                    )
                    .color(Color::Muted)
                    .size(LabelSize::Small),
                ),
        )
        .child(stats_card(border_color))
        .child(render_provider_section(
            "Embeddings",
            "Generates vector embeddings for semantic search.",
            embed_api_key_state(cx),
            |cx| embed_api_url(cx),
            embed_settings(),
            settings_window,
            window,
            cx,
        ))
        .child(render_provider_section(
            "Reranker",
            "Re-ranks search results for relevance.",
            rerank_api_key_state(cx),
            |cx| rerank_api_url(cx),
            rerank_settings(),
            settings_window,
            window,
            cx,
        ))
        .child(render_provider_section(
            "HyDE",
            "Hypothetical Document Embeddings for improved retrieval.",
            hyde_api_key_state(cx),
            |cx| hyde_api_url(cx),
            hyde_settings(),
            settings_window,
            window,
            cx,
        ))
        .into_any_element()
}

fn stats_card(border_color: gpui::Hsla) -> impl IntoElement {
    v_flex()
        .p_4()
        .rounded_lg()
        .border_1()
        .border_color(border_color)
        .gap_2()
        .child(Label::new("Indexing Stats").size(LabelSize::Default))
        .child(stat_row("Status", "Phase 1 — In-memory file hash index"))
        .child(stat_row(
            "Note",
            "Stats update live during scans (see Zed log for details)",
        ))
}

fn stat_row(label: &str, value: &str) -> impl IntoElement {
    h_flex()
        .justify_between()
        .child(
            Label::new(label.to_string())
                .color(Color::Muted)
                .size(LabelSize::Small),
        )
        .child(Label::new(value.to_string()).size(LabelSize::Small))
}

fn render_provider_section(
    title: &'static str,
    description: &'static str,
    api_key_state: Entity<ApiKeyState>,
    current_url: fn(&mut App) -> SharedString,
    setting_items: Box<[SettingsPageItem]>,
    settings_window: &SettingsWindow,
    window: &mut Window,
    cx: &mut Context<SettingsWindow>,
) -> AnyElement {
    let weak_page = cx.weak_entity();
    let creds_provider = credentials_provider(cx);

    _ = window.use_keyed_state(current_url(cx), cx, |_, cx| {
        let task = api_key_state.update(cx, |key_state, cx| {
            key_state.load_if_needed(
                current_url(cx),
                |state| state,
                creds_provider.clone(),
                cx,
            )
        });
        cx.spawn(async move |_, cx| {
            task.await.ok();
            weak_page
                .update(cx, |_, cx| {
                    cx.notify();
                })
                .ok();
        })
    });

    let (has_key, env_var_name, is_from_env_var) = api_key_state.read_with(cx, |state, _| {
        (
            state.has_key(),
            Some(state.env_var_name().clone()),
            state.is_from_env_var(),
        )
    });

    let write_key = move |api_key: Option<String>, cx: &mut App| {
        let creds = credentials_provider(cx);
        api_key_state
            .update(cx, |key_state, cx| {
                let url = current_url(cx);
                key_state.store(url, api_key, |key_state| key_state, creds, cx)
            })
            .detach_and_log_err(cx);
    };

    let additional_fields = settings_window
        .render_sub_page_items_section(setting_items.iter().enumerate(), true, window, cx)
        .into_any_element();

    let base_container = v_flex().id(title).min_w_0().pt_8().gap_1p5();
    let header = SettingsSectionHeader::new(title).no_padding(true);

    let configured_card_label = if is_from_env_var {
        "API Key Set in Environment Variable"
    } else {
        "API Key Configured"
    };

    let container = if has_key {
        base_container.child(header).child(
            ConfiguredApiCard::new(configured_card_label)
                .button_label("Reset Key")
                .button_tab_index(0)
                .disabled(is_from_env_var)
                .when_some(env_var_name, |this, env_var_name| {
                    this.when(is_from_env_var, |this| {
                        this.tooltip_label(format!(
                            "To reset your API key, unset the {} environment variable.",
                            env_var_name
                        ))
                    })
                })
                .on_click(move |_, _, cx| {
                    write_key(None, cx);
                }),
        )
    } else {
        base_container.child(header).child(
            h_flex()
                .pt_2p5()
                .w_full()
                .min_w_0()
                .justify_between()
                .child(
                    v_flex()
                        .w_full()
                        .min_w_0()
                        .max_w_1_2()
                        .child(Label::new("API Key"))
                        .child(
                            Label::new(description)
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                        )
                        .when_some(env_var_name, |this, env_var_name| {
                            this.child({
                                let label = format!(
                                    "Or set the {} env var and restart Zed.",
                                    env_var_name.as_ref()
                                );
                                Label::new(label).size(LabelSize::Small).color(Color::Muted)
                            })
                        }),
                )
                .child(
                    SettingsInputField::new()
                        .tab_index(0)
                        .with_placeholder("xxxxxxxxxxxxxxxxxxxx")
                        .on_confirm(move |api_key, _window, cx| {
                            write_key(api_key.filter(|key| !key.is_empty()), cx);
                        }),
                ),
        )
    };

    container
        .child(
            div()
                .map(|this| if has_key { this.mt_1() } else { this.mt_4() })
                .child(additional_fields),
        )
        .into_any_element()
}

fn embed_settings() -> Box<[SettingsPageItem]> {
    Box::new([
        SettingsPageItem::SettingItem(SettingItem {
            title: "API URL",
            description: "The base URL for the embedding service.",
            field: Box::new(SettingField {
                pick: |settings| {
                    settings
                        .context_index
                        .as_ref()?
                        .embed
                        .as_ref()?
                        .api_url
                        .as_ref()
                },
                write: |settings, value, _app: &App| {
                    settings
                        .context_index
                        .get_or_insert_default()
                        .embed
                        .get_or_insert_default()
                        .api_url = value;
                },
                json_path: Some("context_index.embed.api_url"),
            }),
            metadata: Some(Box::new(SettingsFieldMetadata {
                placeholder: Some("http://localhost:7997"),
                ..Default::default()
            })),
            files: USER,
        }),
        SettingsPageItem::SettingItem(SettingItem {
            title: "Model",
            description: "The model identifier for embeddings.",
            field: Box::new(SettingField {
                pick: |settings| {
                    settings
                        .context_index
                        .as_ref()?
                        .embed
                        .as_ref()?
                        .model
                        .as_ref()
                },
                write: |settings, value, _app: &App| {
                    settings
                        .context_index
                        .get_or_insert_default()
                        .embed
                        .get_or_insert_default()
                        .model = value;
                },
                json_path: Some("context_index.embed.model"),
            }),
            metadata: Some(Box::new(SettingsFieldMetadata {
                placeholder: Some("Qwen/Qwen3-Embedding-4B"),
                ..Default::default()
            })),
            files: USER,
        }),
    ])
}

fn rerank_settings() -> Box<[SettingsPageItem]> {
    Box::new([
        SettingsPageItem::SettingItem(SettingItem {
            title: "API URL",
            description: "The base URL for the reranking service.",
            field: Box::new(SettingField {
                pick: |settings| {
                    settings
                        .context_index
                        .as_ref()?
                        .rerank
                        .as_ref()?
                        .api_url
                        .as_ref()
                },
                write: |settings, value, _app: &App| {
                    settings
                        .context_index
                        .get_or_insert_default()
                        .rerank
                        .get_or_insert_default()
                        .api_url = value;
                },
                json_path: Some("context_index.rerank.api_url"),
            }),
            metadata: Some(Box::new(SettingsFieldMetadata {
                placeholder: Some("http://localhost:7997"),
                ..Default::default()
            })),
            files: USER,
        }),
        SettingsPageItem::SettingItem(SettingItem {
            title: "Model",
            description: "The model identifier for reranking.",
            field: Box::new(SettingField {
                pick: |settings| {
                    settings
                        .context_index
                        .as_ref()?
                        .rerank
                        .as_ref()?
                        .model
                        .as_ref()
                },
                write: |settings, value, _app: &App| {
                    settings
                        .context_index
                        .get_or_insert_default()
                        .rerank
                        .get_or_insert_default()
                        .model = value;
                },
                json_path: Some("context_index.rerank.model"),
            }),
            metadata: Some(Box::new(SettingsFieldMetadata {
                placeholder: Some("BAAI/bge-reranker-v2-m3"),
                ..Default::default()
            })),
            files: USER,
        }),
    ])
}

fn hyde_settings() -> Box<[SettingsPageItem]> {
    Box::new([
        SettingsPageItem::SettingItem(SettingItem {
            title: "API URL",
            description: "The base URL for the HyDE service.",
            field: Box::new(SettingField {
                pick: |settings| {
                    settings
                        .context_index
                        .as_ref()?
                        .hyde
                        .as_ref()?
                        .api_url
                        .as_ref()
                },
                write: |settings, value, _app: &App| {
                    settings
                        .context_index
                        .get_or_insert_default()
                        .hyde
                        .get_or_insert_default()
                        .api_url = value;
                },
                json_path: Some("context_index.hyde.api_url"),
            }),
            metadata: Some(Box::new(SettingsFieldMetadata {
                placeholder: Some("http://localhost:7997"),
                ..Default::default()
            })),
            files: USER,
        }),
        SettingsPageItem::SettingItem(SettingItem {
            title: "Model",
            description: "The model identifier for HyDE generation.",
            field: Box::new(SettingField {
                pick: |settings| {
                    settings
                        .context_index
                        .as_ref()?
                        .hyde
                        .as_ref()?
                        .model
                        .as_ref()
                },
                write: |settings, value, _app: &App| {
                    settings
                        .context_index
                        .get_or_insert_default()
                        .hyde
                        .get_or_insert_default()
                        .model = value;
                },
                json_path: Some("context_index.hyde.model"),
            }),
            metadata: Some(Box::new(SettingsFieldMetadata {
                placeholder: Some("Qwen/Qwen3-4B"),
                ..Default::default()
            })),
            files: USER,
        }),
    ])
}

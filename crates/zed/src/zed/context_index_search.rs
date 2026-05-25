use anyhow::anyhow;
use context_index::{
    ContextIndexSettings, RetrievalToggles, api_keys, context_index_for_project,
    embed::{EmbedClient, RerankClient},
    retrieve::{RenderEvent, search_streaming},
};
use editor::{Editor, MultiBuffer};
use futures::{StreamExt as _, channel::mpsc};
use gpui::{
    App, AppContext as _, Context, DismissEvent, Entity, EventEmitter, Focusable, ParentElement,
    Render, SharedString, Styled, Window, actions,
};
use settings::Settings as _;
use ui::{
    ActiveTheme, Color, FluentBuilder, InteractiveElement, IntoElement, Label, LabelCommon,
    LabelSize, StyledExt, div, h_flex, v_flex,
};
use workspace::{ModalView, Workspace};

actions!(context_index, [Search]);

pub fn init(cx: &mut App) {
    cx.observe_new(
        |workspace: &mut Workspace, _window, _cx: &mut Context<Workspace>| {
            workspace.register_action(|workspace, _: &Search, window, cx| {
                let workspace_handle = cx.entity();
                workspace.toggle_modal(window, cx, move |window, cx| {
                    QueryInputModal::new(workspace_handle.clone(), window, cx)
                });
            });
        },
    )
    .detach();
}

pub struct QueryInputModal {
    workspace: Entity<Workspace>,
    editor: Entity<Editor>,
    last_error: Option<SharedString>,
}

impl EventEmitter<DismissEvent> for QueryInputModal {}
impl ModalView for QueryInputModal {}

impl Focusable for QueryInputModal {
    fn focus_handle(&self, cx: &App) -> gpui::FocusHandle {
        self.editor.focus_handle(cx)
    }
}

impl QueryInputModal {
    fn new(workspace: Entity<Workspace>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let editor = cx.new(|cx| {
            let mut editor = Editor::single_line(window, cx);
            editor.set_placeholder_text("Ask the context index...", window, cx);
            editor
        });

        Self {
            workspace,
            editor,
            last_error: None,
        }
    }

    fn cancel(&mut self, _: &menu::Cancel, _window: &mut Window, cx: &mut Context<Self>) {
        cx.emit(DismissEvent);
    }

    fn confirm(&mut self, _: &menu::Confirm, window: &mut Window, cx: &mut Context<Self>) {
        let query = self.editor.read(cx).text(cx).trim().to_string();
        if query.is_empty() {
            self.last_error = Some("Enter a query to search the context index.".into());
            cx.notify();
            return;
        }

        self.workspace.update(cx, |workspace, cx| {
            start_context_search(workspace, query, window, cx);
        });
        cx.emit(DismissEvent);
    }
}

impl Render for QueryInputModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();

        v_flex()
            .key_context("ContextIndexSearchModal")
            .on_action(cx.listener(Self::cancel))
            .on_action(cx.listener(Self::confirm))
            .elevation_3(cx)
            .w_96()
            .overflow_hidden()
            .child(
                div()
                    .p_2()
                    .border_b_1()
                    .border_color(theme.colors().border_variant)
                    .child(self.editor.clone()),
            )
            .child(
                h_flex()
                    .bg(theme.colors().editor_background)
                    .rounded_b_sm()
                    .w_full()
                    .p_2()
                    .gap_1()
                    .when_some(self.last_error.clone(), |this, error| {
                        this.child(Label::new(error).size(LabelSize::Small).color(Color::Error))
                    })
                    .when(self.last_error.is_none(), |this| {
                        this.child(
                            Label::new("Search the local context index.")
                                .color(Color::Muted)
                                .size(LabelSize::Small),
                        )
                    }),
            )
    }
}

fn start_context_search(
    workspace: &mut Workspace,
    query: String,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let project = workspace.project().clone();
    let Some(index) = context_index_for_project(&project, cx) else {
        workspace.show_error(&anyhow!("Context index is not initialized."), cx);
        return;
    };
    let Some(store) = index.read(cx).store().cloned() else {
        workspace.show_error(&anyhow!("Context index is not ready yet."), cx);
        return;
    };

    let settings = ContextIndexSettings::get_global(cx).clone();
    let embed_api_url = api_keys::embed_api_url(cx).to_string();
    let rerank_api_url = api_keys::rerank_api_url(cx).to_string();
    let embed_api_key = api_keys::embed_api_key_state(cx)
        .read(cx)
        .key(&embed_api_url)
        .map(|key| key.to_string());
    let rerank_api_key = api_keys::rerank_api_key_state(cx)
        .read(cx)
        .key(&rerank_api_url)
        .map(|key| key.to_string());
    let http_client = cx.http_client();

    let embed_client = EmbedClient::new(
        http_client.clone(),
        embed_api_url,
        settings.embed_model.clone(),
        settings.query_instruction.clone(),
        embed_api_key,
    );
    let rerank_client = RerankClient::new(
        http_client,
        rerank_api_url,
        settings.rerank_model.clone(),
        rerank_api_key,
    );
    let toggles = RetrievalToggles::from_settings(&settings);

    let title: String = "Context Index Search".to_string();
    let initial = format!(
        "# Context Index Search\n\n**Query:** `{}`\n\n",
        escape_backticks(&query)
    );
    let buffer = project.update(cx, |project, cx| {
        project.create_local_buffer(&initial, None, true, cx)
    });
    let language_registry = project.read(cx).languages().clone();
    let buffer_for_language = buffer.clone();
    cx.spawn(async move |_workspace, cx| {
        if let Ok(markdown) = language_registry.language_for_name("Markdown").await {
            let _ = buffer_for_language.update(cx, |buffer, cx| {
                buffer.set_language(Some(markdown), cx);
            });
        }
    })
    .detach();
    let multibuffer =
        cx.new(|cx| MultiBuffer::singleton(buffer.clone(), cx).with_title(title.clone()));
    let editor = cx.new(|cx| {
        let mut editor = Editor::for_multibuffer(multibuffer, Some(project.clone()), window, cx);
        editor.set_breadcrumb_header(title.clone());
        editor.disable_mouse_wheel_zoom();
        editor
    });
    workspace.add_item_to_active_pane(Box::new(editor), None, true, window, cx);

    let (tx, mut rx) = mpsc::unbounded();
    let buffer_for_events = buffer.clone();
    cx.spawn(async move |_workspace, cx| {
        while let Some(event) = rx.next().await {
            let text = render_event(event);
            let _ = buffer_for_events.update(cx, |buffer, cx| {
                let end = buffer.len();
                buffer.edit([(end..end, text)], None, cx);
            });
        }
    })
    .detach();

    cx.spawn(async move |_workspace, _cx| {
        if let Err(err) = search_streaming(
            &store,
            &embed_client,
            &rerank_client,
            &query,
            &toggles,
            tx.clone(),
        )
        .await
        {
            let _ = tx.unbounded_send(RenderEvent::Stage {
                label: "Error",
                detail: err.to_string(),
            });
        }
    })
    .detach();
}

fn render_event(event: RenderEvent) -> String {
    match event {
        RenderEvent::Header { .. } => String::new(),
        RenderEvent::Stage { label, detail } => format!("- {label}... {detail}\n"),
        RenderEvent::Result { block_markdown } => block_markdown,
        RenderEvent::Footer {
            count,
            low_confidence,
            elapsed_ms,
        } => {
            let confidence = if low_confidence {
                " · low confidence"
            } else {
                ""
            };
            format!("\n---\n\n**{count} results{confidence} · {elapsed_ms} ms**\n")
        }
    }
}

fn escape_backticks(query: &str) -> String {
    query.replace('`', "\\`")
}

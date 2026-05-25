use context_index::{ContextIndexEvent, context_index_for_project};
use gpui::{
    App, Context, Empty, Entity, EventEmitter, IntoElement, ParentElement, Render, Subscription,
    Window,
};
use project::Project;
use settings_ui::open_settings_editor;
use ui::{Button, Tooltip, prelude::*};
use workspace::{StatusItemView, Workspace, item::ItemHandle};

pub struct ContextIndexIndicator {
    project: Entity<Project>,
    files_indexed: u64,
    currently_scanning: bool,
    scan_progress_done: u64,
    scan_progress_total: u64,
    enabled: bool,
    _subscription: Option<Subscription>,
}

impl ContextIndexIndicator {
    pub fn new(workspace: &Workspace, cx: &mut Context<Self>) -> Self {
        let project = workspace.project().clone();
        let subscription = Self::subscribe_to_index(&project, cx);

        let (files_indexed, currently_scanning, scan_done, scan_total, enabled) =
            if let Some(idx) = context_index_for_project(&project, cx) {
                let stats = idx.read(cx).stats();
                (
                    stats.files_indexed,
                    stats.currently_scanning,
                    stats.scan_progress_done,
                    stats.scan_progress_total,
                    stats.enabled,
                )
            } else {
                (0, false, 0, 0, false)
            };

        Self {
            project,
            files_indexed,
            currently_scanning,
            scan_progress_done: scan_done,
            scan_progress_total: scan_total,
            enabled,
            _subscription: subscription,
        }
    }

    fn subscribe_to_index(
        project: &Entity<Project>,
        cx: &mut Context<Self>,
    ) -> Option<Subscription> {
        let idx = context_index_for_project(project, cx)?;
        Some(cx.subscribe(&idx, Self::on_index_event))
    }

    fn on_index_event(
        &mut self,
        idx: Entity<context_index::ContextIndex>,
        _event: &ContextIndexEvent,
        cx: &mut Context<Self>,
    ) {
        let stats = idx.read(cx).stats();
        self.files_indexed = stats.files_indexed;
        self.currently_scanning = stats.currently_scanning;
        self.scan_progress_done = stats.scan_progress_done;
        self.scan_progress_total = stats.scan_progress_total;
        self.enabled = stats.enabled;
        cx.notify();
    }
}

impl EventEmitter<workspace::ToolbarItemEvent> for ContextIndexIndicator {}

impl Render for ContextIndexIndicator {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.enabled {
            return Empty.into_any_element();
        }

        let (label, tooltip) = if self.currently_scanning {
            let text = if self.scan_progress_total > 0 {
                format!(
                    "{}/{}",
                    self.scan_progress_done, self.scan_progress_total
                )
            } else {
                "…".to_string()
            };
            let tip = format!(
                "Context Indexer: scanning ({}/{})",
                self.scan_progress_done, self.scan_progress_total
            );
            (text, tip)
        } else {
            let text = format!("{}", self.files_indexed);
            let tip = format!("Context Indexer: {} files indexed", self.files_indexed);
            (text, tip)
        };

        div()
            .child(
                Button::new("context-index-indicator", label)
                    .label_size(LabelSize::Small)
                    .start_icon(
                        Icon::new(IconName::Hash)
                            .size(IconSize::Small)
                            .color(Color::Muted),
                    )
                    .map(|btn| {
                        if self.currently_scanning {
                            btn.loading(true)
                        } else {
                            btn
                        }
                    })
                    .tooltip(Tooltip::text(tooltip))
                    .on_click(cx.listener(|_this, _event, window, cx| {
                        open_settings_editor(
                            Some("context_index"),
                            None,
                            window.window_handle().downcast::<workspace::MultiWorkspace>(),
                            cx,
                        );
                    })),
            )
            .into_any_element()
    }
}

impl StatusItemView for ContextIndexIndicator {
    fn set_active_pane_item(
        &mut self,
        _active_pane_item: Option<&dyn ItemHandle>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self._subscription.is_none() {
            self._subscription = Self::subscribe_to_index(&self.project, cx);
            if let Some(idx) = context_index_for_project(&self.project, cx) {
                let stats = idx.read(cx).stats();
                self.files_indexed = stats.files_indexed;
                self.currently_scanning = stats.currently_scanning;
                self.scan_progress_done = stats.scan_progress_done;
                self.scan_progress_total = stats.scan_progress_total;
                self.enabled = stats.enabled;
            }
            cx.notify();
        }
    }

    fn hide_setting(&self, _: &App) -> Option<workspace::HideStatusItem> {
        None
    }
}

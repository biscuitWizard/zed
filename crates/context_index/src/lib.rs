pub mod api_keys;
pub mod hasher;
pub mod index;
pub mod proto_handlers;
pub mod settings;
pub mod stats;

use collections::HashMap;
use gpui::{App, AppContext as _, Entity, EntityId, Global};
use project::Project;
use ::settings::Settings as _;

pub use index::{ContextIndex, ContextIndexEvent};
pub use settings::ContextIndexSettings;
pub use stats::ContextIndexStats;

pub fn context_index_for_project(
    project: &Entity<Project>,
    cx: &App,
) -> Option<Entity<ContextIndex>> {
    cx.try_global::<ContextIndexRegistry>()
        .and_then(|reg| reg.indices.get(&project.entity_id()).cloned())
}

struct ContextIndexRegistry {
    indices: HashMap<EntityId, Entity<ContextIndex>>,
}

impl Global for ContextIndexRegistry {}

pub fn init(cx: &mut App) {
    cx.set_global(ContextIndexRegistry {
        indices: HashMap::default(),
    });

    cx.observe_new::<Project>(|project, _window, cx| {
        let enabled = ContextIndexSettings::get_global(cx).enabled;
        let fs = project.fs().clone();
        let worktree_store = project.worktree_store();
        let project_entity_id = cx.entity_id();
        let context_index = cx.new(|cx| ContextIndex::new(fs, worktree_store, enabled, cx));

        cx.global_mut::<ContextIndexRegistry>()
            .indices
            .insert(project_entity_id, context_index);

        let _ = cx.on_release(move |_project, cx| {
            cx.global_mut::<ContextIndexRegistry>()
                .indices
                .remove(&project_entity_id);
        });
    })
    .detach();
}

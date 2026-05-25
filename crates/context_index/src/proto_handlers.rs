use anyhow::Result;
use gpui::{AppContext as _, AsyncApp, Entity};
use rpc::TypedEnvelope;

use crate::index::ContextIndex;

pub async fn handle_get_stats(
    context_index: Entity<ContextIndex>,
    _envelope: TypedEnvelope<proto::GetContextIndexStats>,
    mut cx: AsyncApp,
) -> Result<proto::ContextIndexStats> {
    let stats = cx.update_entity(&context_index, |this, _| this.stats().to_proto());
    Ok(stats)
}

pub async fn handle_reset(
    context_index: Entity<ContextIndex>,
    _envelope: TypedEnvelope<proto::ResetContextIndex>,
    mut cx: AsyncApp,
) -> Result<proto::Ack> {
    cx.update_entity(&context_index, |this, cx| this.reset(cx));
    Ok(proto::Ack {})
}

pub async fn handle_set_enabled(
    context_index: Entity<ContextIndex>,
    envelope: TypedEnvelope<proto::SetContextIndexEnabled>,
    mut cx: AsyncApp,
) -> Result<proto::Ack> {
    let enabled = envelope.payload.enabled;
    cx.update_entity(&context_index, |this, cx| this.set_enabled(enabled, cx));
    Ok(proto::Ack {})
}

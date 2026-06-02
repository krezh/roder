//! Reactive hooks shared by the live tables.

use leptos::prelude::*;

use crate::app::events::{apply_event, RowMap, UidSet};
use crate::data;

/// Subscribe to a live resource list, re-subscribing whenever `url` re-reads a
/// changed signal. The `url` closure also performs any per-(re)subscribe reset as a
/// side effect, then yields the SSE URL (or `None` to skip). The returned SSE handle
/// is owned by the effect, so the previous stream closes on each re-subscribe.
pub(crate) fn use_sse_subscription(
    rows: RowMap,
    entering: UidSet,
    removing: UidSet,
    url: impl Fn() -> Option<String> + 'static,
) {
    Effect::new(move |_prev: Option<Option<data::SseHandle>>| {
        let url = url()?;
        data::subscribe(&url, move |ev| apply_event(rows, entering, removing, ev))
    });
}

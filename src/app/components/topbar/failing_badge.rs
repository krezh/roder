//! Live cluster-wide count of failing pods. Clicking jumps to the Pods view
//! filtered to problems. Backed by a single all-namespace pod watch.

use leptos::prelude::*;
use roder_core::{ResourceKind, ResourceRow, RowStatus};

use crate::app::hooks::use_sse_subscription;
use crate::app::state::{Catalog, OnlyProblems};
use crate::data;

use super::badge::use_animated_badge;

#[component]
pub(crate) fn FailingBadge() -> impl IntoView {
    let catalog = expect_context::<Catalog>().0;
    let selected_kind = expect_context::<RwSignal<Option<ResourceKind>>>();
    let selected_ns = expect_context::<RwSignal<Option<String>>>();
    let only_problems = expect_context::<OnlyProblems>().0;

    let pod_kind = Memo::new(move |_| {
        catalog
            .get()
            .into_iter()
            .find(|k| k.group.is_empty() && k.kind == "Pod")
    });
    let rows = RwSignal::new(std::collections::HashMap::<String, ResourceRow>::new());
    let entering = RwSignal::new(std::collections::BTreeSet::<String>::new());
    let removing = RwSignal::new(std::collections::BTreeSet::<String>::new());

    use_sse_subscription(rows, entering, removing, None, move || {
        let pk = pod_kind.get()?;
        Some(data::watch_url(&pk.key, None, None))
    });

    // Seed the badge's count from the last-known value so it doesn't vanish and
    // then pop back up across a refresh while the SSE snapshot is in flight.
    let cached_count = RwSignal::new(0usize);
    Effect::new(move |_| {
        if let Some(n) = data::storage_get("roder.failing_count").and_then(|s| s.parse().ok()) {
            cached_count.set(n);
        }
    });
    // `rows` holds every pod cluster-wide, which is never truly empty in a real
    // cluster — a non-empty map is a reliable proxy for "the first SSE snapshot
    // has landed", at which point the live count takes over from the cache.
    let loaded = Memo::new(move |_| rows.with(|m| !m.is_empty()));
    let count = Memo::new(move |_| {
        if loaded.get() {
            rows.with(|m| m.values().filter(|r| r.status == RowStatus::Error).count())
        } else {
            cached_count.get()
        }
    });
    Effect::new(move |_| {
        if loaded.get() {
            data::storage_set("roder.failing_count", &count.get().to_string());
        }
    });
    let badge_count = Memo::new(move |_| {
        let n = count.get();
        (n > 0).then_some(n)
    });
    let (badge_snapshot, badge_closing) = use_animated_badge(badge_count);

    view! {
        {move || {
            badge_snapshot.get().map(|n| {
                let go = move |_| {
                    if let Some(pk) = pod_kind.get_untracked() {
                        selected_ns.set(None);
                        only_problems.set(true);
                        selected_kind.set(Some(pk));
                    }
                };
                view! {
                    <button class="failbadge" class:closing=move || badge_closing.get() on:click=go
                        data-tip="Pods failing — click to view">
                        {n} " Pods"
                    </button>
                }
            })
        }}
    }
}

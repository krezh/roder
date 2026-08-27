//! Live count of failing Flux Kustomizations and HelmReleases.

use leptos::prelude::*;
use roder_core::{ResourceKind, RowStatus};

use crate::app::state::{Catalog, OnlyProblems};
use crate::data;

use super::badge::use_animated_badge;
use super::failure_watch::FailureWatchRows;

#[component]
pub(crate) fn FluxFailingBadge() -> impl IntoView {
    let catalog = expect_context::<Catalog>().0;
    let selected_kind = expect_context::<RwSignal<Option<ResourceKind>>>();
    let selected_ns = expect_context::<RwSignal<Option<String>>>();
    let only_problems = expect_context::<OnlyProblems>().0;

    let ks_kind = Memo::new(move |_| {
        catalog
            .get()
            .into_iter()
            .find(|k| k.group.ends_with("fluxcd.io") && k.kind == "Kustomization")
    });
    let hr_kind = Memo::new(move |_| {
        catalog
            .get()
            .into_iter()
            .find(|k| k.group.ends_with("fluxcd.io") && k.kind == "HelmRelease")
    });

    let failure_rows = expect_context::<FailureWatchRows>();
    let ks_rows = failure_rows.kustomizations;
    let hr_rows = failure_rows.helm_releases;

    // Seed the badge's count from the last-known value so it doesn't vanish and
    // then pop back up across a refresh while the SSE snapshots are in flight.
    let cached_count = RwSignal::new(0usize);
    Effect::new(move |_| {
        if let Some(n) = data::storage_get("roder.flux_failing_count").and_then(|s| s.parse().ok())
        {
            cached_count.set(n);
        }
    });
    // Unlike pods, a cluster can legitimately have zero Kustomizations/HelmReleases,
    // so `loaded` may never flip true — but `cached_count` is then also correctly 0.
    let loaded =
        Memo::new(move |_| ks_rows.with(|m| !m.is_empty()) || hr_rows.with(|m| !m.is_empty()));
    let ks_count = Memo::new(move |_| {
        ks_rows.with(|m| m.values().filter(|r| r.status == RowStatus::Error).count())
    });
    let hr_count = Memo::new(move |_| {
        hr_rows.with(|m| m.values().filter(|r| r.status == RowStatus::Error).count())
    });
    let count = Memo::new(move |_| {
        if loaded.get() {
            ks_count.get() + hr_count.get()
        } else {
            cached_count.get()
        }
    });
    Effect::new(move |_| {
        if loaded.get() {
            data::storage_set("roder.flux_failing_count", &count.get().to_string());
        }
    });
    let badge_label = Memo::new(move |_| {
        let n = count.get();
        (n > 0).then(|| {
            if loaded.get() {
                match (hr_count.get(), ks_count.get()) {
                    (hr, 0) => format!("HR {hr}"),
                    (0, ks) => format!("KS {ks}"),
                    (hr, ks) => format!("HR {hr} · KS {ks}"),
                }
            } else {
                format!("{n} Flux")
            }
        })
    });
    let (badge_snapshot, badge_closing) = use_animated_badge(badge_label);

    view! {
        {move || {
            badge_snapshot.get().map(|label| {
                let go = move |_| {
                    let kind = if hr_rows.with(|m| m.values().any(|r| r.status == RowStatus::Error)) {
                        hr_kind.get_untracked()
                    } else {
                        ks_kind.get_untracked()
                    };
                    if let Some(k) = kind {
                        selected_ns.set(None);
                        only_problems.set(true);
                        selected_kind.set(Some(k));
                    }
                };
                view! {
                    <button class="fluxbadge" data-tip="Flux resources failing — click to view"
                        class:closing=move || badge_closing.get() on:click=go>
                        {label}
                    </button>
                }
            })
        }}
    }
}

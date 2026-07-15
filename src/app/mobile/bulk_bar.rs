//! The mobile bulk-action bar shown while one or more cards are selected —
//! shared by every mobile list (single-kind, search, workspace pane) so the
//! same action set/markup doesn't get re-typed per list.

use leptos::prelude::*;

use crate::app::events::UidSet;
use crate::app::overlays::confirm::{ask_confirm, Confirm};

#[component]
pub(crate) fn MobileBulkBar(
    selected: UidSet,
    /// Turned off (clearing `selected`, via `use_select_mode`'s effect) by
    /// "Done" — the only way out of select mode now that hold is the only
    /// way in.
    select_mode: RwSignal<bool>,
    /// Every uid currently shown, for the "All" button.
    all_uids: impl Fn() -> Vec<String> + Copy + Send + Sync + 'static,
    /// Dispatches a bulk action by name (mirrors desktop's `do_bulk`/`fire_action`).
    do_bulk: impl Fn(&'static str) + Copy + Send + Sync + 'static,
    /// Opens logs for the selection. Always supplied; gated by `show_logs`
    /// (rather than made optional) since a runtime bool can't be threaded
    /// through an `Option<Callback<_>>` prop slot at the call site.
    on_logs: Callback<()>,
    #[prop(default = true)] show_logs: bool,
    #[prop(default = false)] bulk_workload: bool,
    #[prop(default = false)] bulk_flux: bool,
    #[prop(default = false)] bulk_helmrelease: bool,
    #[prop(default = false)] bulk_has_source_ref: bool,
) -> impl IntoView {
    let confirm = expect_context::<RwSignal<Option<Confirm>>>();

    view! {
        <div class="mobile-bulkbar-wrap" class:open=move || !selected.get().is_empty()>
            <div class="mobile-bulkbar">
                <span class="bulk-count">{move || format!("{} selected", selected.get().len())}</span>
                <button class="act" on:click=move |_| selected.set(all_uids().into_iter().collect())>"All"</button>
                <button class="act" on:click=move |_| select_mode.set(false)>"Done"</button>
                {show_logs.then(|| view! {
                    <button class="act" on:click=move |_| on_logs.run(())>"Logs"</button>
                })}
                {bulk_workload.then(|| view! {
                    <button class="act" on:click=move |_| do_bulk("restart")>"Restart"</button>
                })}
                {bulk_flux.then(|| view! {
                    <button class="act" on:click=move |_| do_bulk("flux-reconcile")>"Reconcile"</button>
                    {bulk_has_source_ref.then(|| view! {
                        <button class="act" on:click=move |_| do_bulk("flux-reconcile-with-source")>"Reconcile+src"</button>
                    })}
                    {bulk_helmrelease.then(|| view! {
                        <button class="act" on:click=move |_| do_bulk("flux-force")>"Force"</button>
                        <button class="act" on:click=move |_| do_bulk("flux-reset")>"Reset"</button>
                    })}
                    <button class="act" on:click=move |_| do_bulk("flux-suspend")>"Suspend"</button>
                    <button class="act" on:click=move |_| do_bulk("flux-resume")>"Resume"</button>
                })}
                <button class="act danger" on:click=move |_| {
                    let n = selected.get_untracked().len();
                    ask_confirm(confirm, format!("Delete {n} resources?"), "Delete", move || do_bulk("delete"));
                }>"Delete"</button>
            </div>
        </div>
    }
}

use leptos::prelude::*;
use roder_core::ActionSummary;

use crate::app::state::OnlyProblems;
use crate::app::ui::{ask_sweep, show_toast, show_toast_detail, SweepRequest, Toast, ToastKind};
use crate::data;

#[component]
pub(crate) fn MobileListActions() -> impl IntoView {
    let problems = expect_context::<OnlyProblems>().0;
    view! { <div class="mobile-list-actions" aria-label="Resource actions">
        <button type="button" class="mobile-problems-toggle" class:active=move || problems.get() aria-pressed=move || problems.get().to_string()
            on:click=move |_| problems.update(|value| *value = !*value)>"Problems"</button>
        <MobileSanitizeButton /><MobileSyncButton />
    </div> }
}

#[component]
fn MobileSanitizeButton() -> impl IntoView {
    let sweep = expect_context::<RwSignal<Option<SweepRequest>>>();
    let namespace = expect_context::<RwSignal<Option<String>>>();
    let toast = expect_context::<RwSignal<Option<Toast>>>();
    let sanitize = move |options: roder_core::SweepOptions| {
        let payload = serde_json::json!({
            "action": "sanitize",
            "namespace": namespace.get_untracked(),
            "sweep_options": options,
        });
        leptos::task::spawn_local(async move {
            match data::post_action(&payload).await {
                Ok(body) => {
                    let summary: roder_core::CleanupSummary =
                        serde_json::from_str(&body).unwrap_or_default();
                    let total = summary.pods_deleted + summary.jobs_deleted;
                    show_toast(
                        toast,
                        if total == 0 {
                            "Nothing to sweep".to_string()
                        } else {
                            format!(
                                "Swept {} pod(s), {} job(s)",
                                summary.pods_deleted, summary.jobs_deleted
                            )
                        },
                        ToastKind::Ok,
                    );
                }
                Err(error) => show_toast_detail(toast, "Sweep failed", Some(error), ToastKind::Err),
            }
        });
    };
    view! { <button type="button" class="mobile-sanitize-btn" on:click=move |_| ask_sweep(sweep, namespace.get_untracked(), sanitize)>"Sweep"</button> }
}

#[component]
fn MobileSyncButton() -> impl IntoView {
    let namespace = expect_context::<RwSignal<Option<String>>>();
    let toast = expect_context::<RwSignal<Option<Toast>>>();
    view! { <button type="button" class="mobile-sync-btn" on:click=move |_| {
        let payload = serde_json::json!({ "action": "flux-reconcile-all", "namespace": namespace.get_untracked() });
        leptos::task::spawn_local(async move { match data::post_action(&payload).await {
            Ok(body) => { let summary: ActionSummary = serde_json::from_str(&body).unwrap_or_default(); if summary.succeeded == 0 { show_toast(toast, "No Flux resources reconciled", ToastKind::Err); }
                else if summary.forbidden + summary.failed > 0 { show_toast_detail(toast, format!("Reconciled {} of {} operation(s)", summary.succeeded, summary.attempted), Some(format!("{} forbidden, {} failed", summary.forbidden, summary.failed)), ToastKind::Err); }
                else { show_toast(toast, format!("Reconcile requested for {} resource(s)", summary.succeeded), ToastKind::Ok); } }
            Err(error) => show_toast_detail(toast, "Sync failed", Some(error), ToastKind::Err),
        }});
    }>"Sync"</button> }
}

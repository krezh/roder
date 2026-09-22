//! One-click reconciliation sweep across every Flux resource (optionally
//! scoped to the selected namespace), mirroring `flux reconcile --all`.

use leptos::prelude::*;
use roder_core::ActionSummary;

use crate::app::ui::toast::{show_toast, show_toast_detail, ToastKind, Toasts};
use crate::data;

#[component]
pub(crate) fn SyncButton() -> impl IntoView {
    let selected_ns = expect_context::<RwSignal<Option<String>>>();
    let toast = expect_context::<Toasts>();

    let do_sync = move |_| {
        let ns = selected_ns.get_untracked();
        let payload = serde_json::json!({ "action": "flux-reconcile-all", "namespace": ns });
        leptos::task::spawn_local(async move {
            match data::post_action(&payload).await {
                Ok(body) => {
                    let summary: ActionSummary = serde_json::from_str(&body).unwrap_or_default();
                    if summary.succeeded == 0 {
                        show_toast(toast, "No Flux resources reconciled", ToastKind::Err);
                    } else if summary.forbidden + summary.failed > 0 {
                        show_toast_detail(
                            toast,
                            format!(
                                "Reconciled {} of {} operation(s)",
                                summary.succeeded, summary.attempted
                            ),
                            Some(format!(
                                "{} forbidden, {} failed",
                                summary.forbidden, summary.failed
                            )),
                            ToastKind::Err,
                        );
                    } else {
                        show_toast(
                            toast,
                            format!("Reconcile requested for {} resource(s)", summary.succeeded),
                            ToastKind::Ok,
                        );
                    }
                }
                Err(e) => show_toast_detail(toast, "Sync failed", Some(e), ToastKind::Err),
            }
        });
    };

    view! {
        <button class="sync-btn" data-tip="Reconcile all Flux resources" on:click=do_sync>
            "Sync"
        </button>
    }
}

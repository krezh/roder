//! One-click reconciliation sweep across every Flux resource (optionally
//! scoped to the selected namespace), mirroring `flux reconcile --all`.

use leptos::prelude::*;

use crate::app::overlays::toast::{show_toast, show_toast_detail, Toast, ToastKind};
use crate::data;

#[component]
pub(crate) fn SyncButton() -> impl IntoView {
    let selected_ns = expect_context::<RwSignal<Option<String>>>();
    let toast = expect_context::<RwSignal<Option<Toast>>>();

    let do_sync = move |_| {
        let ns = selected_ns.get_untracked();
        let payload = serde_json::json!({ "action": "flux-reconcile-all", "namespace": ns });
        leptos::task::spawn_local(async move {
            match data::post_action(&payload).await {
                Ok(body) => {
                    let n: usize = body.trim().parse().unwrap_or(0);
                    if n == 0 {
                        show_toast(toast, "No Flux resources reconciled", ToastKind::Err);
                    } else {
                        show_toast(
                            toast,
                            format!("Reconcile requested for {n} resource(s)"),
                            ToastKind::Ok,
                        );
                    }
                }
                Err(e) => show_toast_detail(toast, "Sync failed", Some(e), ToastKind::Err),
            }
        });
    };

    view! {
        <button class="sync-btn tip-anchor" on:click=do_sync>
            "Sync"
            <span class="tooltip">"Reconcile all Flux resources"</span>
        </button>
    }
}

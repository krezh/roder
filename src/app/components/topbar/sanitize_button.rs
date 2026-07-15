//! One-click sweep of dead pods + finished jobs, mirroring k9s's sanitize command.

use leptos::prelude::*;

use crate::app::overlays::confirm::{Confirm, ConfirmButton};
use crate::app::overlays::toast::{show_toast, show_toast_detail, Toast, ToastKind};
use crate::data;

#[component]
pub(crate) fn SanitizeButton() -> impl IntoView {
    let confirm = expect_context::<RwSignal<Option<Confirm>>>();
    let selected_ns = expect_context::<RwSignal<Option<String>>>();
    let toast = expect_context::<RwSignal<Option<Toast>>>();

    let do_sanitize = move || {
        let ns = selected_ns.get_untracked();
        let payload = serde_json::json!({ "action": "sanitize", "namespace": ns });
        leptos::task::spawn_local(async move {
            match data::post_action(&payload).await {
                Ok(body) => {
                    let summary: roder_core::CleanupSummary =
                        serde_json::from_str(&body).unwrap_or_default();
                    let total = summary.pods_deleted + summary.jobs_deleted;
                    if total == 0 {
                        show_toast(toast, "Nothing to sweep", ToastKind::Ok);
                    } else {
                        show_toast(
                            toast,
                            format!(
                                "Swept {} pod(s), {} job(s)",
                                summary.pods_deleted, summary.jobs_deleted
                            ),
                            ToastKind::Ok,
                        );
                    }
                }
                Err(e) => show_toast_detail(toast, "Sweep failed", Some(e), ToastKind::Err),
            }
        });
    };

    view! {
        <button class="sweep-btn tip-anchor"
            on:click=move |_| {
                confirm.set(Some(Confirm {
                    message: "Delete all dead pods and finished jobs?".into(),
                    buttons: vec![ConfirmButton::new("Sweep", do_sanitize)],
                }));
            }>
            "Sweep"
            <span class="tooltip">"Delete dead pods and finished jobs"</span>
        </button>
    }
}

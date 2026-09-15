//! The dead pod/job sweep: its request, the preview both shells show, and
//! the result formatting. The desktop dialog and the mobile sheet render
//! these, so they sit in the shared layer.

use std::sync::Arc;

use leptos::prelude::*;

use super::toast::{show_toast, show_toast_detail, Toast, ToastKind};

#[derive(Clone)]
pub(crate) struct SweepRequest {
    pub(crate) namespace: Option<String>,
    pub(crate) on_confirm: Arc<dyn Fn(roder_core::SweepOptions) + Send + Sync>,
}

pub(crate) fn sweep_result(summary: &roder_core::CleanupSummary) -> (String, Option<String>) {
    let deleted = summary.pods_deleted + summary.jobs_deleted;
    let title = if deleted == 0 {
        "Nothing swept".to_string()
    } else {
        format!(
            "Swept {} pod(s), {} job(s)",
            summary.pods_deleted, summary.jobs_deleted
        )
    };
    let errors = summary
        .forbidden
        .iter()
        .chain(&summary.failed)
        .cloned()
        .collect::<Vec<_>>();
    (title, (!errors.is_empty()).then(|| errors.join("\n")))
}

pub(crate) fn run_sweep(
    toast: RwSignal<Option<Toast>>,
    namespace: Option<String>,
    options: roder_core::SweepOptions,
) {
    let payload = serde_json::json!({
        "action": "sanitize",
        "namespace": namespace,
        "sweep_options": options,
    });
    leptos::task::spawn_local(async move {
        match crate::data::post_action(&payload).await {
            Ok(body) => match serde_json::from_str::<roder_core::CleanupSummary>(&body) {
                Ok(summary) => {
                    let (message, detail) = sweep_result(&summary);
                    if detail.is_some() {
                        show_toast_detail(toast, message, detail, ToastKind::Err);
                    } else {
                        show_toast(toast, message, ToastKind::Ok);
                    }
                }
                Err(error) => show_toast_detail(
                    toast,
                    "Sweep failed",
                    Some(format!("Invalid server response: {error}")),
                    ToastKind::Err,
                ),
            },
            Err(error) => show_toast_detail(toast, "Sweep failed", Some(error), ToastKind::Err),
        }
    });
}

pub(crate) fn ask_sweep(
    signal: RwSignal<Option<SweepRequest>>,
    namespace: Option<String>,
    action: impl Fn(roder_core::SweepOptions) + Send + Sync + 'static,
) {
    signal.set(Some(SweepRequest {
        namespace,
        on_confirm: Arc::new(action),
    }));
}

pub(crate) fn use_sweep_preview(
    namespace: Option<String>,
    options: RwSignal<roder_core::SweepOptions>,
) -> RwSignal<Option<Result<roder_core::SweepCounts, String>>> {
    let preview = RwSignal::new(None);
    let generation = RwSignal::new(0u32);
    Effect::new(move |_| {
        let options = options.get();
        let request_generation = generation.get_untracked().wrapping_add(1);
        generation.set(request_generation);
        if options.is_empty() {
            preview.set(Some(Ok(roder_core::SweepCounts::default())));
            return;
        }
        preview.set(None);
        let payload = serde_json::json!({
            "action": "sanitize-preview",
            "namespace": namespace.clone(),
            "sweep_options": options,
        });
        leptos::task::spawn_local(async move {
            let result = match crate::data::post_action(&payload).await {
                Ok(body) => serde_json::from_str(&body).map_err(|error| error.to_string()),
                Err(error) => Err(error),
            };
            if generation.get_untracked() == request_generation {
                preview.set(Some(result));
            }
        });
    });
    preview
}

#[component]
pub(crate) fn SweepPreview(
    preview: RwSignal<Option<Result<roder_core::SweepCounts, String>>>,
) -> impl IntoView {
    view! {
        <div class="sweep-preview">
            {move || match preview.get() {
                None => "Counting matching resources...".to_string(),
                Some(Err(error)) => format!("Unable to count matching resources: {error}"),
                Some(Ok(summary)) => {
                    let total = summary.pods + summary.jobs;
                    format!("{total} matching: {} pod(s), {} job(s)", summary.pods, summary.jobs)
                }
            }}
        </div>
    }
}

#[component]
pub(crate) fn SweepOption(
    options: RwSignal<roder_core::SweepOptions>,
    field: fn(&mut roder_core::SweepOptions) -> &mut bool,
    label: &'static str,
    hint: &'static str,
) -> impl IntoView {
    view! {
        <label class="opt-row">
            <input type="checkbox" class="check check-static"
                prop:checked=move || options.with(|value| {
                    let mut value = *value;
                    *field(&mut value)
                })
                on:change=move |event| options.update(|value| *field(value) = event_target_checked(&event)) />
            <span>{label}</span>
            <span class="hint">{hint}</span>
        </label>
    }
}

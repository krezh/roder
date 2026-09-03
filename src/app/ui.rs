use std::sync::Arc;

use leptos::prelude::*;
use roder_core::{DeletePropagation, ResourceKind};

const CLOSE_MS: u64 = 160;
pub(crate) const TOAST_MS: u64 = 4000;
const MAX_TOAST_ITEMS: usize = 6;

#[derive(Clone)]
pub(crate) struct ConfirmButton {
    pub(crate) label: String,
    pub(crate) on_click: Arc<dyn Fn() + Send + Sync>,
}

impl ConfirmButton {
    pub(crate) fn new(
        label: impl Into<String>,
        on_click: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        Self {
            label: label.into(),
            on_click: Arc::new(on_click),
        }
    }
}

#[derive(Clone)]
pub(crate) struct Confirm {
    pub(crate) message: String,
    pub(crate) buttons: Vec<ConfirmButton>,
}

pub(crate) fn ask_confirm(
    signal: RwSignal<Option<Confirm>>,
    message: impl Into<String>,
    label: impl Into<String>,
    action: impl Fn() + Send + Sync + 'static,
) {
    signal.set(Some(Confirm {
        message: message.into(),
        buttons: vec![ConfirmButton::new(label, action)],
    }));
}

#[derive(Clone)]
pub(crate) struct SweepRequest {
    pub(crate) namespace: Option<String>,
    pub(crate) on_confirm: Arc<dyn Fn(roder_core::SweepOptions) + Send + Sync>,
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

#[derive(Clone)]
pub(crate) struct DeleteRequest {
    pub(crate) message: String,
    pub(crate) on_confirm: Arc<dyn Fn(bool, Option<DeletePropagation>) + Send + Sync>,
}

pub(crate) fn ask_delete(
    signal: RwSignal<Option<DeleteRequest>>,
    message: impl Into<String>,
    action: impl Fn(bool, Option<DeletePropagation>) + Send + Sync + 'static,
) {
    signal.set(Some(DeleteRequest {
        message: message.into(),
        on_confirm: Arc::new(action),
    }));
}

pub(crate) fn delete_extra(
    force: bool,
    propagation: Option<DeletePropagation>,
) -> serde_json::Value {
    serde_json::json!({ "force": force, "propagation": propagation })
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToastKind {
    Ok,
    Err,
    Progress,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Toast {
    pub(crate) id: u64,
    pub(crate) title: String,
    pub(crate) items: Vec<String>,
    pub(crate) detail: Option<String>,
    pub(crate) kind: ToastKind,
    pub(crate) progress: Option<usize>,
}

#[derive(Clone, Copy)]
pub(crate) struct ProgressToast(u64);

fn next_toast_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT_ID: AtomicU64 = AtomicU64::new(0);
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

pub(crate) fn show_toast(
    signal: RwSignal<Option<Toast>>,
    title: impl Into<String>,
    kind: ToastKind,
) {
    show_toast_full(signal, title, Vec::new(), None::<String>, kind);
}

pub(crate) fn show_toast_detail(
    signal: RwSignal<Option<Toast>>,
    title: impl Into<String>,
    detail: Option<impl Into<String>>,
    kind: ToastKind,
) {
    show_toast_full(signal, title, Vec::new(), detail, kind);
}

pub(crate) fn show_toast_list(
    signal: RwSignal<Option<Toast>>,
    title: impl Into<String>,
    items: Vec<String>,
    kind: ToastKind,
) {
    show_toast_full(signal, title, items, None::<String>, kind);
}

pub(crate) fn show_toast_full(
    signal: RwSignal<Option<Toast>>,
    title: impl Into<String>,
    mut items: Vec<String>,
    detail: Option<impl Into<String>>,
    kind: ToastKind,
) {
    if items.len() > MAX_TOAST_ITEMS {
        let rest = items.len() - (MAX_TOAST_ITEMS - 1);
        items.truncate(MAX_TOAST_ITEMS - 1);
        items.push(format!("+{rest} more"));
    }
    signal.set(Some(Toast {
        id: next_toast_id(),
        title: title.into(),
        items,
        detail: detail.map(Into::into),
        kind,
        progress: None,
    }));
}

pub(crate) fn show_progress_toast(
    signal: RwSignal<Option<Toast>>,
    title: impl Into<String>,
    detail: impl Into<String>,
) -> ProgressToast {
    let id = next_toast_id();
    signal.set(Some(Toast {
        id,
        title: title.into(),
        items: Vec::new(),
        detail: Some(detail.into()),
        kind: ToastKind::Progress,
        progress: Some(0),
    }));
    ProgressToast(id)
}

pub(crate) fn update_progress_toast(
    signal: RwSignal<Option<Toast>>,
    handle: ProgressToast,
    detail: impl Into<String>,
    progress: usize,
) {
    let detail = detail.into();
    signal.update(|current| {
        if let Some(current) = current.as_mut().filter(|toast| toast.id == handle.0) {
            current.detail = Some(detail);
            current.progress = Some(progress.min(100));
        }
    });
}

pub(crate) fn use_bool_overlay(
    open: RwSignal<bool>,
) -> (RwSignal<bool>, RwSignal<bool>, impl Fn() + Copy) {
    let visible = RwSignal::new(false);
    let closing = RwSignal::new(false);
    let do_close = move || {
        if !closing.get_untracked() {
            closing.set(true);
            open.set(false);
            set_timeout(
                move || {
                    if closing.get_untracked() {
                        visible.set(false);
                        closing.set(false);
                    }
                },
                std::time::Duration::from_millis(CLOSE_MS),
            );
        }
    };
    Effect::new(move |_| {
        if open.get() {
            visible.set(true);
            closing.set(false);
        } else if visible.get_untracked() && !closing.get_untracked() {
            closing.set(true);
            set_timeout(
                move || {
                    if closing.get_untracked() {
                        visible.set(false);
                        closing.set(false);
                    }
                },
                std::time::Duration::from_millis(CLOSE_MS),
            );
        }
    });
    (visible, closing, do_close)
}

pub(crate) fn use_option_overlay<T: Clone + Send + Sync + 'static>(
    signal: RwSignal<Option<T>>,
) -> (RwSignal<Option<T>>, RwSignal<bool>, impl Fn() + Copy) {
    let snapshot = RwSignal::new(None::<T>);
    let closing = RwSignal::new(false);
    let do_close = move || {
        if !closing.get_untracked() {
            closing.set(true);
            signal.set(None);
            set_timeout(
                move || {
                    if closing.get_untracked() {
                        snapshot.set(None);
                        closing.set(false);
                    }
                },
                std::time::Duration::from_millis(CLOSE_MS),
            );
        }
    };
    Effect::new(move |_| {
        let value = signal.get();
        if value.is_some() {
            snapshot.set(value);
            closing.set(false);
        } else if snapshot.get_untracked().is_some() && !closing.get_untracked() {
            closing.set(true);
            set_timeout(
                move || {
                    if closing.get_untracked() {
                        snapshot.set(None);
                        closing.set(false);
                    }
                },
                std::time::Duration::from_millis(CLOSE_MS),
            );
        }
    });
    (snapshot, closing, do_close)
}

pub(crate) fn use_dialog_focus(dialog_ref: NodeRef<leptos::html::Div>) {
    #[cfg(target_arch = "wasm32")]
    {
        use wasm_bindgen::JsCast;

        Effect::new(move |_| {
            let Some(dialog) = dialog_ref.get() else {
                return;
            };
            let items = dialog_focusables(&dialog);
            if let Some(first) = items.first() {
                let _ = first.focus();
            } else {
                let _ = dialog.focus();
            }
        });

        Effect::new(move |_| {
            let handle = window_event_listener(leptos::ev::keydown, move |event| {
                if event.key() != "Tab" {
                    return;
                }
                let Some(dialog) = dialog_ref.get_untracked() else {
                    return;
                };
                let items = dialog_focusables(&dialog);
                if items.is_empty() {
                    event.prevent_default();
                    let _ = dialog.focus();
                    return;
                }
                let active = web_sys::window()
                    .and_then(|window| window.document())
                    .and_then(|document| document.active_element());
                let current = items.iter().position(|item| {
                    active
                        .as_ref()
                        .is_some_and(|active| item.dyn_ref::<web_sys::Element>() == Some(active))
                });
                let next = match (current, event.shift_key()) {
                    (Some(0), true) | (None, true) => items.len() - 1,
                    (Some(index), true) => index - 1,
                    (Some(index), false) if index + 1 < items.len() => index + 1,
                    _ => 0,
                };
                event.prevent_default();
                let _ = items[next].focus();
            });
            on_cleanup(move || handle.remove());
        });
    }

    #[cfg(not(target_arch = "wasm32"))]
    let _ = dialog_ref;
}

#[cfg(target_arch = "wasm32")]
fn dialog_focusables(dialog: &web_sys::HtmlDivElement) -> Vec<web_sys::HtmlElement> {
    use wasm_bindgen::JsCast;

    let Ok(nodes) = dialog.query_selector_all(
        "button:not([disabled]), a[href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex='-1'])",
    ) else {
        return Vec::new();
    };
    (0..nodes.length())
        .filter_map(|index| nodes.item(index))
        .filter_map(|node| node.dyn_into::<web_sys::HtmlElement>().ok())
        .filter(|element| element.offset_width() > 0 || element.offset_height() > 0)
        .collect()
}

pub(crate) fn fuzzy_match(pattern: &str, text: &str) -> Option<(Vec<usize>, i32)> {
    let pattern: Vec<char> = pattern.to_lowercase().chars().collect();
    let lowered = text.to_lowercase();
    let chars: Vec<char> = lowered.chars().collect();
    if pattern.is_empty() {
        return Some((Vec::new(), 0));
    }
    let mut positions = Vec::new();
    let mut pattern_index = 0;
    let mut score = 0;
    let mut last = None;
    for (index, character) in chars.iter().enumerate() {
        if pattern_index < pattern.len() && *character == pattern[pattern_index] {
            positions.push(index);
            if last.is_some_and(|last| index == last + 1) {
                score += 10;
            }
            if index == 0 || matches!(chars[index - 1], '_' | '-' | ' ') {
                score += 15;
            }
            if text.chars().nth(index) == Some(pattern[pattern_index]) {
                score += 5;
            }
            score += 1;
            last = Some(index);
            pattern_index += 1;
        }
    }
    (pattern_index == pattern.len()).then_some((positions, score))
}

pub(crate) fn highlight(text: &str, positions: &[usize]) -> Vec<(String, bool)> {
    if positions.is_empty() {
        return vec![(text.to_string(), false)];
    }
    let positions: std::collections::HashSet<_> = positions.iter().copied().collect();
    let mut output = Vec::new();
    let mut buffer = String::new();
    let mut matched = positions.contains(&0);
    for (index, character) in text.chars().enumerate() {
        let next_matched = positions.contains(&index);
        if next_matched != matched {
            output.push((std::mem::take(&mut buffer), matched));
            matched = next_matched;
        }
        buffer.push(character);
    }
    if !buffer.is_empty() {
        output.push((buffer, matched));
    }
    output
}

pub(crate) fn filter_kinds(
    catalog: &[ResourceKind],
    query: &str,
) -> Vec<(ResourceKind, Vec<usize>)> {
    let mut matches: Vec<_> = catalog
        .iter()
        .filter_map(|kind| {
            if query.is_empty() {
                return Some((kind.clone(), Vec::new(), 0));
            }
            let plural = fuzzy_match(query, &kind.plural);
            let name = fuzzy_match(query, &kind.kind);
            match (plural, name) {
                (None, None) => None,
                (Some((positions, score)), None) | (None, Some((positions, score))) => {
                    Some((kind.clone(), positions, score))
                }
                (Some(a), Some(b)) => {
                    let (positions, score) = if a.1 >= b.1 { a } else { b };
                    Some((kind.clone(), positions, score))
                }
            }
        })
        .collect();
    matches.sort_by_key(|(_, _, score)| std::cmp::Reverse(*score));
    matches.truncate(60);
    matches
        .into_iter()
        .map(|(kind, positions, _)| (kind, positions))
        .collect()
}

pub(crate) fn filter_namespaces(
    namespaces: Vec<String>,
    selected: Option<String>,
    query: &str,
) -> Vec<(Option<String>, String, Vec<usize>)> {
    let mut values: Vec<Option<String>> = std::iter::once(None)
        .chain(namespaces.into_iter().map(Some))
        .collect();
    if query.is_empty() {
        if let Some(index) = selected.and_then(|selected| {
            values
                .iter()
                .position(|value| value.as_ref() == Some(&selected))
        }) {
            let active = values.remove(index);
            values.insert(1, active);
        }
    }
    let mut scored: Vec<_> = values
        .into_iter()
        .filter_map(|value| {
            let label = value.as_deref().unwrap_or("All namespaces").to_string();
            if query.is_empty() {
                Some((value, label, Vec::new(), 0))
            } else {
                fuzzy_match(query, &label)
                    .map(|(positions, score)| (value, label, positions, score))
            }
        })
        .collect();
    if !query.is_empty() {
        scored.sort_by_key(|(_, _, _, score)| std::cmp::Reverse(*score));
    }
    scored
        .into_iter()
        .map(|(value, label, positions, _)| (value, label, positions))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn namespace_filter_keeps_all_and_active_first() {
        let values =
            filter_namespaces(vec!["alpha".into(), "beta".into()], Some("beta".into()), "");
        assert_eq!(
            values
                .iter()
                .map(|value| value.0.as_deref())
                .collect::<Vec<_>>(),
            vec![None, Some("beta"), Some("alpha")]
        );
    }

    #[test]
    fn fuzzy_match_rewards_contiguous_word_prefixes() {
        assert!(
            fuzzy_match("dep", "Deployment").unwrap().1
                > fuzzy_match("dep", "DebugEndpoint").unwrap().1
        );
    }
}

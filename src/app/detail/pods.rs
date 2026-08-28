//! The pod-info modal and the workload's live pod list.

use leptos::prelude::*;
use roder_core::RowStatus;

use crate::app::controllers::detail::use_pod_watch;
use crate::app::state::{CtxMenu, DetailTarget, PodModalTarget, Tick};
use crate::app::util::color::dot_class;
use crate::data;

use super::RowDetail;

/// Centered modal showing a single pod's info (opened by clicking a pod in a
/// workload's Pods tab). Reuses `RowDetail` for the Info/YAML/Logs tabs.
#[component]
pub(crate) fn PodModal() -> impl IntoView {
    let pod_modal = expect_context::<PodModalTarget>().0;
    let (snapshot, closing, do_close) = crate::app::overlays::use_option_overlay(pod_modal);

    view! {
        {move || snapshot.get().map(|target| {
            let name = target.name.clone();
            view! {
                <div class="pmodal-scrim" class:closing=move || closing.get()
                    on:click=move |_| do_close()></div>
                <div class="pmodal" class:closing=move || closing.get()>
                    <div class="pmodal-head">
                        <span class="pmodal-title">{name}</span>
                        <button class="pmodal-close" on:click=move |_| do_close()>"✕"</button>
                    </div>
                    <RowDetail target=target on_delete=move || do_close() />
                </div>
            }
        })}
    }
}

/// Live list of the pods owned by the expanded workload (matched by its selector,
/// via a label-filtered watch). Clicking a pod opens it in a centered modal.
#[component]
pub(crate) fn PodsTab(namespace: String, selector: String) -> impl IntoView {
    let pod_modal = expect_context::<PodModalTarget>().0;
    let ctx = expect_context::<RwSignal<Option<CtxMenu>>>();
    let tick = expect_context::<Tick>().0;
    let watch = use_pod_watch(namespace, selector);
    let rows = watch.rows;
    let shown_uids = watch.shown_uids;
    let pod_kind = watch.pod_kind;

    view! {
        <div class="rd-body">
            <div class="pods-mini">
                <For each=move || shown_uids.get() key=|u| u.clone() let:uid>
                    {
                        let uid2 = uid.clone();
                        let row = Memo::new(move |_| rows.with(|m| m.get(&uid2).cloned()));
                        let st = move || dot_class(row.get().map(|r| r.status).unwrap_or(RowStatus::Unknown));
                        let open = move |_| {
                            if let (Some(r), Some(pk)) = (row.get_untracked(), pod_kind.get_untracked()) {
                                pod_modal.set(Some(DetailTarget { key: pk.key, namespace: r.namespace, name: r.name }));
                            }
                        };
                        let on_ctx = move |e: leptos::ev::MouseEvent| {
                            e.prevent_default();
                            if let (Some(r), Some(pk)) = (row.get_untracked(), pod_kind.get_untracked()) {
                                ctx.set(Some(CtxMenu {
                                    x: e.client_x(),
                                    y: e.client_y(),
                                    target: DetailTarget { key: pk.key, namespace: r.namespace.clone(), name: r.name.clone() },
                                    node: None,
                                    uid: r.uid.clone(),
                                }));
                            }
                        };
                        view! {
                            <div class="pm-row" on:click=open on:contextmenu=on_ctx>
                                <span class=move || format!("pm-dot {}", st())></span>
                                <span class="pm-name">{move || row.get().map(|r| r.name).unwrap_or_default()}</span>
                                <span class="pm-phase" style=move || format!("color:var(--{})", st())>
                                    {move || row.get().and_then(|r| r.cells.get(1).cloned()).unwrap_or_default()}
                                </span>
                                <span class="pm-num">{move || row.get().and_then(|r| r.cells.first().cloned()).unwrap_or_default()}</span>
                                <span class="pm-num">{move || {
                                    let v = row.get().and_then(|r| r.cells.get(2).cloned()).unwrap_or_default();
                                    if data::cell_needs_tick(&v) {
                                        tick.get();
                                    }
                                    let v = data::humanize_cell(&v);
                                    if let Some((main, hint)) = v.split_once('\x1f') {
                                        format!("⟳ {main} ({hint})")
                                    } else {
                                        format!("⟳ {v}")
                                    }
                                }}</span>
                                <span class="pm-num">{move || { tick.get(); data::humanize_age(&row.get().and_then(|r| r.created)) }}</span>
                            </div>
                        }
                    }
                </For>
            </div>
            {move || shown_uids.get().is_empty().then(|| view! { <div class="muted pad">"No pods match this selector."</div> })}
        </div>
    }
}

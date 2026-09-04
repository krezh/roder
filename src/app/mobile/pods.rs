use leptos::prelude::*;
use roder_core::RowStatus;

use crate::app::controllers::detail::use_pod_watch;
use crate::app::state::{CtxMenu, DetailTarget, PodModalTarget, Tick};
use crate::app::util::color::dot_class;
use crate::data;

use super::detail_view::MobileRowDetail;

#[component]
pub(crate) fn MobilePodModal() -> impl IntoView {
    let modal = expect_context::<PodModalTarget>().0;
    let (snapshot, closing, close) = crate::app::ui::use_option_overlay(modal);
    view! { {move || snapshot.get().map(|target| view! {
        <div class="mobile-modal-scrim" class:closing=move || closing.get() on:click=move |_| close()></div>
        <section class="mobile-detail mobile-pod-modal open" class:closing=move || closing.get()><header class="mobile-detail-head"><button class="mobile-detail-back" on:click=move |_| close()>"Back"</button><strong class="mobile-detail-title">{target.name.clone()}</strong></header><div class="mobile-detail-body"><MobileRowDetail target on_delete=move || close() /></div></section>
    })} }
}

#[component]
pub(crate) fn MobilePodsTab(namespace: String, selector: String) -> impl IntoView {
    let watch = use_pod_watch(namespace, selector);
    let modal = expect_context::<PodModalTarget>().0;
    let context = expect_context::<RwSignal<Option<CtxMenu>>>();
    let tick = expect_context::<Tick>().0;
    view! { <div class="rd-body"><div class="pods-mini"><For each=move || watch.shown_uids.get() key=Clone::clone let:uid>{
        let row_uid = uid.clone(); let row = Memo::new(move |_| watch.rows.with(|rows| rows.get(&row_uid).cloned())); let status = move || dot_class(row.get().map(|row| row.status).unwrap_or(RowStatus::Unknown));
        let target = move || row.get_untracked().zip(watch.pod_kind.get_untracked()).map(|(row, kind)| DetailTarget { key: kind.key, namespace: row.namespace, name: row.name });
        view! { <article class="pm-row" on:click=move |_| if let Some(target) = target() { modal.set(Some(target)) } on:contextmenu=move |event| { event.prevent_default(); if let (Some(target), Some(row)) = (target(), row.get_untracked()) { context.set(Some(CtxMenu { x: event.client_x(), y: event.client_y(), target, node: None, #[cfg(target_arch = "wasm32")] focus_first: false, uid: row.uid })) } }><span class=move || format!("pm-dot {}", status())></span><span class="pm-name">{move || row.get().map(|row| row.name).unwrap_or_default()}</span><span class="pm-phase" style=move || format!("color:var(--{})", status())>{move || row.get().and_then(|row| row.cells.get(1).cloned()).unwrap_or_default()}</span><span class="pm-num">{move || row.get().and_then(|row| row.cells.first().cloned()).unwrap_or_default()}</span><span class="pm-num">{move || { tick.get(); data::humanize_cell(&row.get().and_then(|row| row.cells.get(2).cloned()).unwrap_or_default()) }}</span><span class="pm-num">{move || { tick.get(); data::humanize_age(&row.get().and_then(|row| row.created)) }}</span></article> }
    }</For></div>{move || watch.shown_uids.get().is_empty().then(|| view! { <div class="muted pad">"No pods match this selector."</div> })}</div> }
}

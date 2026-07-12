//! Shared row + header building blocks for the live resource table.
//!
//! [`ResourceRow`] is the wrapper `<div class="grid-row row">` that owns the
//! click / long-press / context-menu / transition-end handlers. Cell content
//! is supplied by the caller as `children`, so the same wrapper is used by
//! the single-kind `ResourceView` and the multi-kind `SearchResultsView`.
//!
//! [`NameCell`] is the shared Name cell (checkbox + status-tinted name), used
//! by both views.

use leptos::prelude::*;
use roder_core::RowStatus;

use crate::app::events::{range_select, UidSet};
use crate::app::hooks::RowPress;
use crate::app::state::{CtxMenu, DetailTarget};
use crate::app::util::color::name_color;

/// A live table row. Owns the row-level interactions (click, shift-click range
/// select, ctrl/meta toggle, long-press multi-select, right-click context menu,
/// transition-end unmount). Cell content is provided as `children`.
///
/// `node_for_ctx` yields an optional node string for the context menu (pods show
/// the node they're scheduled on).
#[component]
pub(crate) fn ResourceRow<N>(
    uid: String,
    target: DetailTarget,
    detail: RwSignal<Option<DetailTarget>>,
    ctx_menu: RwSignal<Option<CtxMenu>>,
    selected: UidSet,
    last_clicked: RwSignal<Option<String>>,
    entering: UidSet,
    removing: UidSet,
    on_unmount: Callback<String>,
    shown_uids: Memo<Vec<String>>,
    press: RowPress,
    node_for_ctx: N,
    children: Children,
) -> impl IntoView
where
    N: Fn() -> Option<String> + Copy + Send + Sync + 'static,
{
    let is_active = {
        let t = target.clone();
        move || detail.get().as_ref() == Some(&t)
    };
    let t_click = target.clone();
    let t_ctx = target;
    let uid_chk = uid.clone();
    let uid_en = uid.clone();
    let uid_rm = uid.clone();
    let uid_te = uid.clone();
    let uid_pd = uid.clone();
    let uid_ctx = uid.clone();
    let uid_ctrl = uid;

    view! {
        <div class="grid-row row"
            class:active=is_active
            class:selected=move || selected.get().contains(&uid_chk)
            class:entering=move || entering.get().contains(&uid_en)
            class:removing=move || removing.get().contains(&uid_rm)
            on:click=move |e: leptos::ev::MouseEvent| {
                if press.fired.get_value() { press.fired.set_value(false); return; }
                if e.shift_key() {
                    range_select(selected, last_clicked, shown_uids, &uid_ctrl);
                } else if e.ctrl_key() || e.meta_key() {
                    let u = uid_ctrl.clone();
                    selected.update(|s| { if !s.remove(&u) { s.insert(u.clone()); } });
                    last_clicked.set(Some(u));
                } else {
                    let t = t_click.clone();
                    detail.update(|d| if d.as_ref() == Some(&t) { *d = None } else { *d = Some(t.clone()) });
                }
            }
            on:pointerdown=move |e: leptos::ev::PointerEvent| {
                press.fired.set_value(false);
                if e.button() != 0 || e.ctrl_key() || e.shift_key() || e.meta_key() { return; }
                press.xy.set_value((e.client_x(), e.client_y()));
                let u = uid_pd.clone();
                let sel = selected;
                let lc = last_clicked;
                let handle = press.handle;
                let fired = press.fired;
                let h = set_timeout_with_handle(move || {
                    sel.update(|s| { s.insert(u.clone()); });
                    lc.set(Some(u.clone()));
                    fired.set_value(true);
                    handle.set_value(None);
                }, std::time::Duration::from_millis(450)).ok();
                press.handle.set_value(h);
            }
            on:pointermove=move |e: leptos::ev::PointerEvent| {
                if press.handle.with_value(|h| h.is_some()) {
                    let (sx, sy) = press.xy.get_value();
                    if (e.client_x() - sx).abs() > 10 || (e.client_y() - sy).abs() > 10 {
                        press.cancel.run(());
                    }
                }
            }
            on:pointerup=move |_| press.cancel.run(())
            on:pointercancel=move |_| press.cancel.run(())
            on:mousedown=move |e: leptos::ev::MouseEvent| {
                if e.shift_key() { e.prevent_default(); }
            }
            on:contextmenu=move |e: leptos::ev::MouseEvent| {
                e.prevent_default();
                ctx_menu.set(Some(CtxMenu {
                    x: e.client_x(),
                    y: e.client_y(),
                    target: t_ctx.clone(),
                    node: node_for_ctx(),
                    uid: uid_ctx.clone(),
                }));
            }
            on:transitionend=move |e: leptos::ev::TransitionEvent| {
                if e.property_name() == "grid-template-rows"
                    && removing.get_untracked().contains(&uid_te)
                {
                    on_unmount.run(uid_te.clone());
                    removing.update(|s| { s.remove(&uid_te); });
                }
            }>
            {children()}
        </div>
    }
}

/// The Name cell: checkbox (toggles/shift-selects on click) + status-tinted name.
#[component]
pub(crate) fn NameCell<F, S>(
    uid: String,
    name: F,
    status: S,
    selected: UidSet,
    last_clicked: RwSignal<Option<String>>,
    shown_uids: Memo<Vec<String>>,
) -> impl IntoView
where
    F: Fn() -> Option<String> + Copy + Send + Sync + 'static,
    S: Fn() -> Option<RowStatus> + Copy + Send + Sync + 'static,
{
    let uid_checked = uid.clone();
    view! {
        <div class="cell cell-name">
            <div class="cw"><div class="cwi">
                <button
                    type="button"
                    class="check"
                    role="checkbox"
                    aria-label="Toggle row selection"
                    aria-checked=move || selected.get().contains(&uid_checked)
                    tabindex=move || if selected.get().is_empty() { -1 } else { 0 }
                    on:click=move |e: leptos::ev::MouseEvent| {
                    e.stop_propagation();
                    if e.shift_key() {
                        range_select(selected, last_clicked, shown_uids, &uid);
                    } else {
                        let u = uid.clone();
                        selected.update(|s| { if !s.remove(&u) { s.insert(u.clone()); } });
                        last_clicked.set(Some(u));
                    }
                }></button>
                <span class="nm" style=move || name_color(status().unwrap_or(RowStatus::Unknown))>
                    {move || name().unwrap_or_default()}
                </span>
            </div></div>
        </div>
    }
}

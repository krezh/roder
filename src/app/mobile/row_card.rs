//! The mobile replacement for the desktop's dense grid row: one card per
//! resource. Tap opens detail (or toggles selection in Select mode);
//! hold enters multi-select, while the overflow button opens the action sheet.

use std::time::Duration;

use leptos::prelude::*;
use roder_core::{ResourceRow, RowStatus};

use crate::app::events::UidSet;
use crate::app::hooks::RowPress;
use crate::app::state::{CtxMenu, DetailTarget};
use crate::app::table_logic;
use crate::app::util::color::{dot_class, name_color};

/// Everything a card needs to render, projected from the underlying row (or
/// merged row, for multi-kind lists) by the caller.
#[derive(Clone, PartialEq)]
pub(crate) struct CardFields {
    pub(crate) name: String,
    pub(crate) status: RowStatus,
    pub(crate) namespace: Option<String>,
    /// Shown only in multi-kind contexts (search results).
    pub(crate) kind_label: Option<String>,
    /// Up to a couple of surfaced columns: (label, value, colored-by-status).
    pub(crate) extra: Vec<(String, String, bool)>,
    pub(crate) age: String,
}

impl CardFields {
    /// Build a card's fields from a live row and its snapshot schema.
    pub(crate) fn from_row(
        row: &ResourceRow,
        columns: &[String],
        kind_label: Option<String>,
    ) -> Self {
        let extra = table_logic::surfaced_cells(columns, &row.cells)
            .into_iter()
            .map(|(label, value, colored)| {
                // Collapse multi-value Table cells to the desktop's compact form.
                let value = value.replace('\n', ", ");
                let value = if crate::data::cell_needs_tick(&value) {
                    crate::data::humanize_cell(&value)
                } else {
                    value
                };
                (label, value, colored)
            })
            .collect();
        let cell = |header: &str| {
            columns
                .iter()
                .position(|column| column == header)
                .and_then(|i| row.cells.get(i))
                .cloned()
        };
        Self {
            name: cell("Name").unwrap_or_else(|| row.name.clone()),
            status: row.status,
            namespace: cell("Namespace").or_else(|| row.namespace.clone()),
            kind_label,
            extra,
            age: cell("Age")
                .map(|value| crate::data::humanize_cell(&value))
                .unwrap_or_default(),
        }
    }
}

/// Explicit multi-select toggle shared by every mobile list: while off,
/// `selected` is kept empty so a stray tap-to-toggle from a previous session
/// can't leave hidden selections around.
pub(crate) fn use_select_mode(selected: UidSet) -> RwSignal<bool> {
    let select_mode = RwSignal::new(false);
    Effect::new(move |_| {
        if !select_mode.get() {
            selected.set(Default::default());
        }
    });
    select_mode
}

#[component]
pub(crate) fn MobileRowCard(
    uid: String,
    target: DetailTarget,
    detail: RwSignal<Option<DetailTarget>>,
    ctx_menu: RwSignal<Option<CtxMenu>>,
    selected: UidSet,
    select_mode: RwSignal<bool>,
    press: RowPress,
    /// Pods show the node they're scheduled on in the action sheet; other kinds pass `|| None`.
    node_for_ctx: impl Fn() -> Option<String> + Copy + Send + Sync + 'static,
    fields: Memo<Option<CardFields>>,
) -> impl IntoView {
    let uid_click = uid.clone();
    let uid_pd = uid.clone();
    let uid_chk = uid.clone();
    let t_click = target.clone();
    let t_menu = target.clone();
    let uid_menu = uid.clone();

    view! {
        <div class="mobile-card" role="button" tabindex="0"
            class:selected=move || selected.get().contains(&uid_chk)
            on:click=move |_| {
                if press.fired.get_value() { press.fired.set_value(false); return; }
                if select_mode.get_untracked() {
                    let u = uid_click.clone();
                    selected.update(|s| { if !s.remove(&u) { s.insert(u); } });
                    // Deselecting the last item would otherwise strand select
                    // mode on with the bulk bar (and its only "Done" exit)
                    // hidden, since it only shows while something's selected.
                    if selected.get_untracked().is_empty() {
                        select_mode.set(false);
                    }
                } else {
                    let t = t_click.clone();
                    detail.update(|d| if d.as_ref() == Some(&t) { *d = None } else { *d = Some(t.clone()) });
                }
            }
            on:pointerdown=move |e: leptos::ev::PointerEvent| {
                press.fired.set_value(false);
                if e.button() != 0 { return; }
                press.xy.set_value((e.client_x(), e.client_y()));
                let fired = press.fired;
                let handle = press.handle;
                let uid = uid_pd.clone();
                let h = set_timeout_with_handle(move || {
                    fired.set_value(true);
                    handle.set_value(None);
                    select_mode.set(true);
                    selected.update(|s| { s.insert(uid.clone()); });
                }, Duration::from_millis(450)).ok();
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
            on:contextmenu=move |e: leptos::ev::MouseEvent| e.prevent_default()>
            {move || {
                let menu_target = t_menu.clone();
                let menu_uid = uid_menu.clone();
                let check_uid = uid.clone();
                fields.get().map(move |f| view! {
                <div class="mc-head">
                    <span class=format!("dot dot-{}", dot_class(f.status))></span>
                    <span class="mc-name" style=name_color(f.status)>{f.name}</span>
                    {select_mode.get().then(|| {
                        let uid_chk2 = check_uid.clone();
                        view! {
                            <span class="mc-check" class:on=move || selected.get().contains(&uid_chk2)></span>
                        }
                    })}
                    {(!select_mode.get()).then(|| view! {
                        <button class="mc-menu" aria-label="Resource actions"
                            on:pointerdown=move |e| e.stop_propagation()
                            on:click=move |e| {
                                e.stop_propagation();
                                press.cancel.run(());
                                ctx_menu.set(Some(CtxMenu {
                                    x: 0,
                                    y: 0,
                                    target: menu_target.clone(),
                                    node: node_for_ctx(),
                                    #[cfg(target_arch = "wasm32")]
                                    focus_first: false,
                                    uid: menu_uid.clone(),
                                }));
                            }>
                            <span aria-hidden="true">"•••"</span>
                        </button>
                    })}
                </div>
                <div class="mc-meta">
                    {f.kind_label.map(|k| view! { <span class="mc-chip mc-chip-kind">{k}</span> })}
                    {f.namespace.map(|ns| view! { <span class="mc-chip">{ns}</span> })}
                    <span class="mc-chip mc-chip-age">{f.age}</span>
                </div>
                {(!f.extra.is_empty()).then(|| {
                    // Colored extras are tinted by the row's overall computed
                    // `RowStatus` — the same source desktop's `colored_cols`
                    // branch uses (`kind_table.rs`) — not by guessing from the
                    // cell's raw text, which drifts from the real status (a
                    // HelmRelease's Ready value isn't one of a fixed word set).
                    let status_class = dot_class(f.status);
                    view! {
                        <div class="mc-extra">
                            {f.extra.into_iter().map(|(label, value, colored)| {
                                let cls = if colored { format!("mc-ex mc-ex-{status_class}") } else { "mc-ex".to_string() };
                                    view! {
                                        <span class=cls>
                                            <span class="mc-ex-label">{label}</span>
                                            <span class="mc-ex-value">{value}</span>
                                        </span>
                                    }
                            }).collect_view()}
                        </div>
                    }
                })}
                })
            }}
        </div>
    }
}

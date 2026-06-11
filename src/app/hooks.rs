//! Reactive hooks shared by the live tables.

use leptos::html::Div;
use leptos::prelude::*;

use roder_core::WatchEvent;

use crate::app::events::{apply_event, RowMap, UidSet};
use crate::app::state::{ConnectionState, SortKey};
use crate::data;

/// Subscribe to a live resource list, re-subscribing whenever `url` re-reads a
/// changed signal or when the connection is lost (e.g. the pod restarts).
/// The `url` closure also performs any per-(re)subscribe reset as a side effect,
/// then yields the SSE URL (or `None` to skip). The returned SSE handle is owned
/// by the effect, so the previous stream closes on each re-subscribe.
pub(crate) fn use_sse_subscription(
    rows: RowMap,
    entering: UidSet,
    removing: UidSet,
    url: impl Fn() -> Option<String> + 'static,
) {
    // A counter that the error handler bumps to re-trigger the subscription Effect.
    let reconnect: RwSignal<u32> = RwSignal::new(0);
    let conn = use_context::<ConnectionState>().map(|c| c.0);
    Effect::new(move |_prev: Option<Option<data::SseHandle>>| {
        reconnect.track();
        let url = url()?;
        data::subscribe_with_error(
            &url,
            move |ev| {
                if matches!(ev, WatchEvent::Snapshot { .. }) {
                    if let Some(c) = conn { c.set(true); }
                }
                apply_event(rows, entering, removing, ev)
            },
            move || {
                if let Some(c) = conn { c.set(false); }
                set_timeout(
                    move || reconnect.update(|n| *n += 1),
                    std::time::Duration::from_secs(3),
                )
            },
        )
    });
}

/// Per-table long-press ("hold") state, shared by every row.
#[derive(Clone, Copy)]
pub(crate) struct RowPress {
    pub handle: StoredValue<Option<TimeoutHandle>>,
    pub xy: StoredValue<(i32, i32)>,
    pub fired: StoredValue<bool>,
    pub cancel: Callback<()>,
}

/// Bundles every signal + NodeRef that the live table needs. Produced by
/// [`use_resource_table`]; passed to [`table_window`] and to row components.
#[derive(Clone, Copy)]
pub(crate) struct ResourceTable {
    pub rows: RowMap,
    pub selected: UidSet,
    pub last_clicked: RwSignal<Option<String>>,
    pub sort: RwSignal<(SortKey, bool)>,
    pub entering: UidSet,
    pub removing: UidSet,
    pub scroll_top: RwSignal<f64>,
    pub viewport_h: RwSignal<f64>,
    pub row_h: RwSignal<f64>,
    pub table_ref: NodeRef<Div>,
    pub press: RowPress,
}

/// Rows rendered beyond the viewport on each side, so scrolling doesn't flash blanks.
pub(crate) const OVERSCAN: usize = 12;

/// Create the shared table signals and long-press infra. Does not attach any
/// keyboard shortcuts — those live in [`KindTable`] and are opt-in per instance.
pub(crate) fn use_table_state() -> ResourceTable {
    let rows: RowMap = RwSignal::new(Default::default());
    let selected: UidSet = RwSignal::new(Default::default());
    let last_clicked = RwSignal::new(None::<String>);
    let sort = RwSignal::new((SortKey::Namespace, true));
    let entering: UidSet = RwSignal::new(Default::default());
    let removing: UidSet = RwSignal::new(Default::default());
    let scroll_top = RwSignal::new(0.0f64);
    let viewport_h = RwSignal::new(1000.0f64);
    let row_h = RwSignal::new(35.0f64);
    let table_ref = NodeRef::<Div>::new();
    let press_handle = StoredValue::new(None::<TimeoutHandle>);
    let press_xy = StoredValue::new((0i32, 0i32));
    let press_fired = StoredValue::new(false);
    let cancel_press = Callback::new(move |_| {
        press_handle.update_value(|h| {
            if let Some(h) = h.take() {
                h.clear();
            }
        });
    });
    let press = RowPress {
        handle: press_handle,
        xy: press_xy,
        fired: press_fired,
        cancel: cancel_press,
    };
    ResourceTable {
        rows,
        selected,
        last_clicked,
        sort,
        entering,
        removing,
        scroll_top,
        viewport_h,
        row_h,
        table_ref,
        press,
    }
}

/// Build the `(first, last)` virtual-window memo, and attach the RAF-measure +
/// scroll + resize listeners. Must be called once per table, after the caller has
/// derived `shown_uids` from `table.rows`.
pub(crate) fn table_window(
    table: ResourceTable,
    shown_uids: Memo<Vec<String>>,
) -> Memo<(usize, usize)> {
    // `ResourceTable` is `Copy`, so field access doesn't consume it; this lets us
    // bind `table_ref` only on wasm32 (the only target that uses it).
    let scroll_top = table.scroll_top;
    let viewport_h = table.viewport_h;
    let row_h = table.row_h;
    #[cfg(target_arch = "wasm32")]
    let table_ref = table.table_ref;
    let window = Memo::new(move |_| {
        let total = shown_uids.with(|v| v.len());
        let rh = row_h.get().max(1.0);
        let first = ((scroll_top.get() / rh).floor() as usize)
            .saturating_sub(OVERSCAN)
            .min(total);
        let count = (viewport_h.get() / rh).ceil() as usize + 2 * OVERSCAN;
        let last = (first + count).min(total);
        (first, last)
    });

    // Measure the real viewport + row height from the DOM. Done in a rAF so the
    // read happens *after* layout (a synchronous read can be 0/stale, which
    // collapsed the window before), guarded so a bad read never shrinks it.
    // Re-measures as the list (re)populates and on window resize.
    #[cfg(target_arch = "wasm32")]
    let measure = move || {
        request_animation_frame(move || {
            let Some(wrap) = table_ref.get_untracked() else {
                return;
            };
            let ch = wrap.client_height() as f64;
            if ch > 50.0 && (ch - viewport_h.get_untracked()).abs() > 0.5 {
                viewport_h.set(ch);
            }
            if let Ok(Some(row)) = wrap.query_selector(".grid-row.row") {
                let h = row.get_bounding_client_rect().height();
                if h > 1.0 && (h - row_h.get_untracked()).abs() > 0.5 {
                    row_h.set(h);
                }
            }
        });
    };
    Effect::new(move |_| {
        shown_uids.with(|v| v.len());
        #[cfg(target_arch = "wasm32")]
        measure();
    });
    Effect::new(move |_| {
        let h = window_event_listener(leptos::ev::resize, move |_| {
            #[cfg(target_arch = "wasm32")]
            measure();
        });
        on_cleanup(move || h.remove());
    });

    // Attach the scroll listener directly to the container. Scroll doesn't
    // bubble, so event delegation can miss it — a direct listener is reliable.
    Effect::new(move |_| {
        #[cfg(target_arch = "wasm32")]
        {
            use send_wrapper::SendWrapper;
            use wasm_bindgen::closure::Closure;
            use wasm_bindgen::JsCast;
            let Some(wrap) = table_ref.get() else { return };
            let el = wrap.clone();
            let cb = Closure::<dyn FnMut()>::new(move || {
                scroll_top.set(el.scroll_top() as f64);
                let ch = el.client_height() as f64;
                if ch > 50.0 {
                    viewport_h.set(ch);
                }
            });
            let cb_fn: js_sys::Function = cb.as_ref().unchecked_ref::<js_sys::Function>().clone();
            let _ = wrap.add_event_listener_with_callback("scroll", &cb_fn);
            // SendWrapper satisfies on_cleanup's Send+Sync bound; sound on wasm32 (single-threaded).
            let cleanup = SendWrapper::new((wrap, cb_fn, cb));
            on_cleanup(move || {
                let (wrap, cb_fn, cb) = cleanup.take();
                let _ = wrap.remove_event_listener_with_callback("scroll", &cb_fn);
                drop(cb);
            });
        }
    });

    window
}

//! One reusable, app-wide tooltip.

#[cfg(target_arch = "wasm32")]
use leptos::ev;
use leptos::prelude::*;

/// Cursor offset so the bubble doesn't sit directly under the pointer (and
/// therefore doesn't itself intercept the mouseleave that would dismiss it).
#[cfg(target_arch = "wasm32")]
const OFFSET_X: f64 = 14.0;
#[cfg(target_arch = "wasm32")]
const OFFSET_Y: f64 = 18.0;

/// Any element with a `data-tip` attribute shows it on hover, following the
/// cursor. Table cells (identified by their `.cwi` wrapper) only show it when
/// their content is actually truncated, so short cells don't pop noise —
/// everything else (buttons, badges) shows unconditionally, since they carry
/// no visible text to compare against. A single fixed-position bubble (styled
/// `.tooltip`) is reused everywhere instead of native `title` or per-feature
/// tooltip CSS.
#[component]
pub(crate) fn TooltipLayer() -> impl IntoView {
    // Content only changes when the hovered target changes — kept separate
    // from position so cursor movement never rebuilds the tooltip's DOM
    // (that would fight the `node_ref` below and thrash the clamp effect).
    let tip_content = RwSignal::new(None::<String>);
    // Raw, unclamped cursor position (client coords), updated on every move.
    // Only read client-side (ssr has no cursor to track).
    #[cfg_attr(not(target_arch = "wasm32"), allow(unused_variables))]
    let anchor = RwSignal::new((0.0f64, 0.0f64));
    // Final, viewport-clamped position — this alone drives the rendered `style`.
    let pos = RwSignal::new((0i32, 0i32));
    let tip_ref = NodeRef::<leptos::html::Div>::new();
    #[cfg(target_arch = "wasm32")]
    {
        use wasm_bindgen::JsCast;
        let h = window_event_listener(ev::mousemove, move |e: ev::MouseEvent| {
            let host = e
                .target()
                .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
                .and_then(|el| el.closest("[data-tip]").ok().flatten());
            let Some(host) = host else {
                tip_content.set(None);
                return;
            };
            let text = host.get_attribute("data-tip").unwrap_or_default();
            if text.is_empty() {
                tip_content.set(None);
                return;
            }
            let cx = f64::from(e.client_x());
            let cy = f64::from(e.client_y());
            // Already showing this exact tooltip — skip the eligibility
            // re-check on every pixel of intra-cell movement, but still track
            // the cursor below.
            let already_showing =
                tip_content.with_untracked(|t| t.as_deref() == Some(text.as_str()));
            if !already_showing {
                // `.cwi` marks a table cell (see table.rs) — those only show a
                // tip when genuinely truncated, or for list values (multiple
                // lines). Anything else carrying `data-tip` (buttons, badges)
                // has no visible text to compare against, so it always shows.
                let is_list = text.contains('\n');
                let cell_measure = host.query_selector(".cwi").ok().flatten();
                let truncated = cell_measure
                    .as_ref()
                    .is_some_and(|m| m.scroll_width() > m.client_width() + 1);
                if cell_measure.is_some() && !is_list && !truncated {
                    tip_content.set(None);
                    return;
                }
                tip_content.set(Some(text));
                // Optimistic position so there's no flash before the clamp
                // effect (which needs `tip_ref` mounted) refines it.
                pos.set(((cx + OFFSET_X) as i32, (cy + OFFSET_Y) as i32));
            }
            anchor.set((cx, cy));
        });
        on_cleanup(move || h.remove());

        // Clamp the bubble inside the viewport on every cursor move, flipping
        // above the cursor if it would overflow the bottom.
        Effect::new(move |_| {
            let (cx, cy) = anchor.get();
            if tip_content.with(|t| t.is_none()) {
                return;
            }
            let Some(el) = tip_ref.get() else {
                return;
            };
            let rect = el.get_bounding_client_rect();
            let (w, ht) = (rect.width(), rect.height());
            let win = web_sys::window().unwrap();
            let vw = win
                .inner_width()
                .ok()
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            let vh = win
                .inner_height()
                .ok()
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            let m = 8.0;
            let mut left = cx + OFFSET_X;
            let mut top = cy + OFFSET_Y;
            if w > 0.0 && left + w + m > vw {
                left = (vw - w - m).max(m);
            }
            if left < m {
                left = m;
            }
            if ht > 0.0 && top + ht + m > vh {
                top = (cy - OFFSET_Y - ht).max(m);
            }
            if top < m {
                top = m;
            }
            pos.set((left as i32, top as i32));
        });
    }
    view! {
        // Outer closure depends only on `tip_content`. `pos` drives the
        // `style` in place, so the element is never recreated on reposition —
        // otherwise node_ref would re-fire and loop with the clamp effect.
        {move || tip_content.get().map(|text| {
            // Newline-separated content renders as a list (e.g. hostnames); a
            // single line renders as plain text.
                let items: Vec<String> = text.split('\n').filter(|s| !s.is_empty()).map(|s| s.to_string()).collect();
                let body = if items.len() > 1 {
                    view! {
                        <ul class="tip-list">
                            {items.into_iter().map(|i| view! { <li>{i}</li> }).collect_view()}
                        </ul>
                    }.into_any()
                } else {
                    view! { <span>{text}</span> }.into_any()
                };
                view! {
                    <div class="tooltip" node_ref=tip_ref
                        style=move || { let (x, y) = pos.get(); format!("left:{x}px;top:{y}px") }>
                        {body}
                    </div>
                }
            })}
    }
}

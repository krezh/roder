//! One reusable, app-wide tooltip.

#[cfg(target_arch = "wasm32")]
use leptos::ev;
use leptos::prelude::*;

/// Any element with a `data-tip` attribute shows it on hover — but only when its
/// content is actually truncated, so short cells don't pop noise. A single
/// fixed-position bubble (styled `.tooltip`) is reused everywhere instead of native
/// `title` or per-feature tooltip CSS.
#[component]
pub(crate) fn TooltipLayer() -> impl IntoView {
    // (text, anchor_left, anchor_top). `pos` is the final, viewport-clamped position.
    let tip = RwSignal::new(None::<(String, f64, f64)>);
    let pos = RwSignal::new((0i32, 0i32));
    let tip_ref = NodeRef::<leptos::html::Div>::new();
    #[cfg(target_arch = "wasm32")]
    {
        use wasm_bindgen::JsCast;
        let h = window_event_listener(ev::mouseover, move |e: ev::MouseEvent| {
            let host = e
                .target()
                .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
                .and_then(|el| el.closest("[data-tip]").ok().flatten());
            let Some(host) = host else {
                tip.set(None);
                return;
            };
            let text = host.get_attribute("data-tip").unwrap_or_default();
            if text.is_empty() {
                tip.set(None);
                return;
            }
            // Already showing this exact tooltip — don't thrash on intra-cell moves.
            if tip.with_untracked(|t| t.as_ref().map(|(txt, _, _)| txt == &text).unwrap_or(false)) {
                return;
            }
            // Always show list values (multiple lines); single values only when clipped.
            let is_list = text.contains('\n');
            let measure = host
                .query_selector(".cwi")
                .ok()
                .flatten()
                .unwrap_or_else(|| host.clone());
            let truncated = measure.scroll_width() > measure.client_width() + 1;
            if !is_list && !truncated {
                tip.set(None);
                return;
            }
            let r = host.get_bounding_client_rect();
            let (ax, ay) = (r.left(), r.bottom() + 4.0);
            tip.set(Some((text, ax, ay)));
            pos.set((ax as i32, ay as i32));
        });
        on_cleanup(move || h.remove());

        // Once the bubble is rendered, clamp it inside the viewport (flip above the
        // anchor if it would overflow the bottom).
        Effect::new(move |_| {
            let Some((_, ax, ay)) = tip.get() else {
                return;
            };
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
            let mut left = ax;
            let mut top = ay;
            if w > 0.0 && left + w + m > vw {
                left = (vw - w - m).max(m);
            }
            if left < m {
                left = m;
            }
            if ht > 0.0 && top + ht + m > vh {
                top = (ay - 8.0 - ht).max(m);
            }
            if top < m {
                top = m;
            }
            pos.set((left as i32, top as i32));
        });
    }
    view! {
        // Outer closure depends only on `tip` (the content). `pos` drives the
        // `style` in place, so the element is never recreated on reposition —
        // otherwise node_ref would re-fire and loop with the clamp effect.
        {move || tip.get().map(|(text, _, _)| {
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
                    style=move || { let (x, y) = pos.get(); format!("position:fixed;left:{x}px;top:{y}px") }>
                    {body}
                </div>
            }
        })}
    }
}

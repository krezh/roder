//! Rendered text width via a cached offscreen canvas.
//!
//! Column auto-sizing needs to know which of several candidate strings is
//! visually widest. Byte/char length is not a valid proxy for that in a
//! proportional font (a shorter string can render wider than a longer one),
//! so this measures the actual glyph width instead.

#[cfg(target_arch = "wasm32")]
use std::cell::RefCell;

#[cfg(target_arch = "wasm32")]
thread_local! {
    // Lazily built on first use and reused for the process lifetime — creating
    // a canvas + 2D context per call would be needlessly expensive given how
    // often column sizing recomputes.
    static CTX: RefCell<Option<web_sys::CanvasRenderingContext2d>> = const { RefCell::new(None) };
}

#[cfg(target_arch = "wasm32")]
fn make_ctx() -> Option<web_sys::CanvasRenderingContext2d> {
    use wasm_bindgen::JsCast;
    let window = web_sys::window()?;
    let document = window.document()?;
    let canvas = document.create_element("canvas").ok()?;
    let canvas: web_sys::HtmlCanvasElement = canvas.dyn_into().ok()?;
    let ctx = canvas
        .get_context("2d")
        .ok()??
        .dyn_into::<web_sys::CanvasRenderingContext2d>()
        .ok()?;
    // Match the table's actual cell font (`.cell { font-weight: 500 }`). `.grid-table`
    // sets its own font-family (Monaspace Neon NF) distinct from `body` (Rubik), so
    // measuring from `body` would size columns against the wrong glyph widths —
    // that mismatch made columns visibly jitter as different rows scrolled into
    // view and got (mis)measured. Measure from a live `.grid-table` instead, with
    // a hardcoded fallback for the rare case none is mounted yet.
    let table_font = document
        .query_selector(".grid-table")
        .ok()
        .flatten()
        .and_then(|el| window.get_computed_style(&el).ok().flatten())
        .and_then(|s| s.get_property_value("font").ok())
        .filter(|f| !f.is_empty());
    let font = table_font.unwrap_or_else(|| {
        "13px \"Monaspace Neon NF\", ui-monospace, \"Cascadia Code\", monospace".to_string()
    });
    ctx.set_font(&format!("500 {font}"));
    Some(ctx)
}

/// Rendered width (CSS px) of `s` in the table cell font. Falls back to the byte
/// length before the canvas/font is available (first frame) or off the wasm32
/// target (SSR) — a crude proxy, but only ever used as a transient starting
/// point before the real measurement kicks in.
pub(crate) fn text_width(s: &str) -> f64 {
    #[cfg(target_arch = "wasm32")]
    {
        let measured = CTX.with(|cell| {
            let mut slot = cell.borrow_mut();
            if slot.is_none() {
                *slot = make_ctx();
            }
            slot.as_ref()
                .and_then(|ctx| ctx.measure_text(s).ok())
                .map(|m| m.width())
        });
        if let Some(w) = measured {
            return w;
        }
    }
    s.len() as f64
}

use leptos::ev::PointerEvent;

pub(crate) fn update_hover_origin(event: PointerEvent) {
    #[cfg(not(target_arch = "wasm32"))]
    let _ = event;

    #[cfg(target_arch = "wasm32")]
    {
        use wasm_bindgen::JsCast;

        let Some(card) = event
            .target()
            .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
            .and_then(|element| {
                element
                    .closest(".interactive-card, .detail-stat, .overview-grid > .kv")
                    .ok()
                    .flatten()
            })
        else {
            return;
        };
        let rect = card.get_bounding_client_rect();
        let Some(card) = card.dyn_ref::<web_sys::HtmlElement>() else {
            return;
        };
        let _ = card.style().set_property(
            "--card-hover-x",
            &format!("{}px", event.client_x() as f64 - rect.left()),
        );
        let _ = card.style().set_property(
            "--card-hover-y",
            &format!("{}px", event.client_y() as f64 - rect.top()),
        );
    }
}

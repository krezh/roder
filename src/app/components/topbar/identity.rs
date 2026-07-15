//! Topbar identity: current user's name, opening a dropdown with Access and
//! Sign out.

use leptos::prelude::*;

use crate::app::state::AccessReviewOpen;
use crate::data;

#[component]
pub(crate) fn Identity() -> impl IntoView {
    let identity = RwSignal::new(None::<serde_json::Value>);
    // Seed from the last-known identity so the name/menu doesn't flash empty
    // on refresh while the first `/api/me` round-trip is in flight.
    Effect::new(move |_| {
        if let Some(cached) = data::storage_get("roder.identity")
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        {
            identity.set(Some(cached));
        }
    });
    let me =
        LocalResource::new(|| async { data::fetch_json::<serde_json::Value>("/api/me").await });
    Effect::new(move |_| {
        if let Some(Ok(v)) = me.get() {
            if let Ok(json) = serde_json::to_string(&v) {
                data::storage_set("roder.identity", &json);
            }
            identity.set(Some(v));
        }
    });

    let menu_open = RwSignal::new(false);
    let (visible, closing, do_close) = crate::app::overlays::use_bool_overlay(menu_open);
    let access_open = expect_context::<AccessReviewOpen>().0;

    view! {
        <span class="identity">
            {move || identity.get().map(|v| {
                let who = v.get("email").and_then(|e| e.as_str())
                    .or_else(|| v.get("name").and_then(|n| n.as_str()))
                    .or_else(|| v.get("subject").and_then(|s| s.as_str()))
                    .unwrap_or("anonymous").to_string();
                view! {
                    <div class="user-menu">
                        <button class="user-menu-trigger" on:click=move |_| menu_open.update(|o| *o = !*o)>
                            {who}
                        </button>
                        {move || visible.get().then(|| view! {
                            <div class="ctx-scrim" on:click=move |_| do_close()></div>
                            <div class="ctx-menu user-menu-dropdown" class:closing=move || closing.get()>
                                <button class="ctx-item"
                                    on:click=move |_| { do_close(); access_open.set(true); }>
                                    "Access"
                                </button>
                                <a class="ctx-item" href="/auth/logout" rel="external">"Sign out"</a>
                            </div>
                        })}
                    </div>
                }
            })}
        </span>
    }
}

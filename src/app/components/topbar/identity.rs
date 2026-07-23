//! Topbar identity: current user's name, opening a dropdown (`components::dropdown::Dropdown`)
//! with Access and Sign out.

use leptos::prelude::*;

use crate::app::components::dropdown::{Dropdown, DropdownClose};
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

    let access_open = expect_context::<AccessReviewOpen>().0;

    view! {
        <span class="identity">
            {move || identity.get().map(|v| {
                let who = v.get("email").and_then(|e| e.as_str())
                    .or_else(|| v.get("name").and_then(|n| n.as_str()))
                    .or_else(|| v.get("subject").and_then(|s| s.as_str()))
                    .unwrap_or("anonymous").to_string();
                let who_label = who.clone();
                view! {
                    <Dropdown label=move || who_label.clone()>
                        <div class="dropdown-head">{who}</div>
                        <AccessItem access_open=access_open />
                        <a class="dropdown-item" href="/auth/logout" rel="external">"Sign out"</a>
                    </Dropdown>
                }
            })}
        </span>
    }
}

/// Its own component (rather than inlined into `Identity`'s `children`) so it
/// can pull `DropdownClose` from context — `children` runs inside
/// `Dropdown`'s reactive scope, where that context is available, but a plain
/// closure defined at `Identity`'s own top level would run outside it.
#[component]
fn AccessItem(access_open: RwSignal<bool>) -> impl IntoView {
    let close = expect_context::<DropdownClose>().0;
    view! {
        <button class="dropdown-item"
            on:click=move |_| { close.run(()); access_open.set(true); }>
            "Access"
        </button>
    }
}

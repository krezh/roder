use leptos::prelude::*;
use roder_core::ResourceKind;

use crate::app::components::kind_table::KindTable;
use crate::app::views::dashboard::Dashboard;
use crate::data;

#[component]
pub(crate) fn ResourceView() -> impl IntoView {
    let selected_kind = expect_context::<RwSignal<Option<ResourceKind>>>();
    let selected_ns = expect_context::<RwSignal<Option<String>>>();

    view! {
        {move || match selected_kind.get() {
            None => view! { <Dashboard /> }.into_any(),
            Some(kind) => {
                let storage_key = format!("roder.filter.{}", kind.key);
                let initial = data::storage_get(&storage_key).unwrap_or_default();
                let text_filter = RwSignal::new(initial);

                // Persist filter changes to localStorage.
                Effect::new(move |_| {
                    let val = text_filter.get();
                    if val.is_empty() {
                        data::storage_remove(&storage_key);
                    } else {
                        data::storage_set(&storage_key, &val);
                    }
                });

                let k = kind.clone();
                view! {
                    <KindTable
                        kind=kind
                        url_fn=move || {
                            let ns = if k.namespaced { selected_ns.get() } else { None };
                            Some(data::watch_url(&k.key, ns.as_deref(), None))
                        }
                        namespace=None
                        selector=None
                        text_filter=text_filter
                        keyboard=true
                        register_global_selection=true />
                }.into_any()
            }
        }}
    }
}

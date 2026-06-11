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
                        keyboard=true
                        register_global_selection=true />
                }.into_any()
            }
        }}
    }
}

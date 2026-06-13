use std::collections::HashMap;

use leptos::prelude::*;
use roder_core::{ResourceKind, WatchEvent};

use crate::app::components::kind_table::KindTable;
use crate::app::events::RowMap;
use crate::app::state::{Catalog, DetailTarget, PaneConfig, WorkspaceConf};
use crate::data;

#[component]
pub(crate) fn WorkspaceView() -> impl IntoView {
    let ws = expect_context::<WorkspaceConf>().0;

    // One row signal per pane (keyed by kind_key). Non-reactive storage so the
    // multi-watch Effect can write into individual signals without loops.
    let pane_rows: StoredValue<HashMap<String, RowMap>> = StoredValue::new(HashMap::new());

    // Single SSE connection for all workspace panes. Reconnects whenever the
    // pane set changes (add/remove/namespace change), replacing the old handle.
    let reconnect = RwSignal::new(0u32);
    Effect::new(move |_prev: Option<Option<data::SseHandle>>| {
        reconnect.track();
        let panes = ws.with(|w| w.panes.clone());
        if panes.is_empty() {
            return None;
        }

        pane_rows.update_value(|map| {
            map.retain(|k, _| panes.iter().any(|p| &p.kind_key == k));
            for p in &panes {
                map.entry(p.kind_key.clone())
                    .or_insert_with(|| RwSignal::new(HashMap::new()));
            }
        });

        let url = data::watch_multi_url(
            &panes
                .iter()
                .map(|p| (p.kind_key.as_str(), p.namespace.as_deref()))
                .collect::<Vec<_>>(),
        );

        data::subscribe_multi(
            &url,
            move |key, event| {
                pane_rows.with_value(|map| {
                    if let Some(&rows) = map.get(&key) {
                        match event {
                            WatchEvent::Snapshot { rows: r } => {
                                rows.set(r.into_iter().map(|row| (row.uid.clone(), row)).collect());
                            }
                            WatchEvent::Applied { row } => {
                                rows.update(|m| {
                                    m.insert(row.uid.clone(), row);
                                });
                            }
                            WatchEvent::Deleted { uid } => {
                                rows.update(|m| {
                                    m.remove(&uid);
                                });
                            }
                        }
                    }
                });
            },
            move || {
                set_timeout(
                    move || reconnect.update(|n| *n += 1),
                    data::reconnect_delay(),
                );
            },
        )
    });

    view! {
        <div class="workspace" style=move || {
            let n = ws.with(|w| w.panes.len());
            if n == 0 { return String::new(); }
            let cols = (n as f64).sqrt().ceil() as usize;
            let rows = n.div_ceil(cols);
            format!("grid-template-columns:repeat({cols},1fr);grid-template-rows:repeat({rows},1fr)")
        }>
            <For
                each=move || ws.with(|w| w.panes.clone())
                key=|cfg| cfg.kind_key.clone()
                let:cfg
            >
                {
                    pane_rows.update_value(|map| {
                        map.entry(cfg.kind_key.clone())
                            .or_insert_with(|| RwSignal::new(HashMap::new()));
                    });
                    let rows_sig = pane_rows
                        .with_value(|map| *map.get(&cfg.kind_key).expect("just inserted"));
                    let close_key = cfg.kind_key.clone();
                    let ns_key = cfg.kind_key.clone();
                    view! {
                        <PaneView
                            config=cfg
                            rows_sig=rows_sig
                            on_close=Callback::new(move |_| {
                                ws.update(|w| w.panes.retain(|p| p.kind_key != close_key));
                            })
                            on_ns_change=Callback::new(move |ns: Option<String>| {
                                ws.update(|w| {
                                    if let Some(p) = w.panes.iter_mut().find(|p| p.kind_key == ns_key) {
                                        p.namespace = ns;
                                    }
                                });
                            })
                        />
                    }
                }
            </For>
            {move || ws.with(|w| w.panes.is_empty()).then(|| view! {
                <div class="workspace-empty">
                    <p>
                        "No panes — right-click a resource kind in the sidebar to add it, "
                        "or use "<kbd>"in:kind"</kbd>" in the palette."
                    </p>
                </div>
            })}
        </div>
    }
}

#[component]
fn PaneView(
    config: PaneConfig,
    rows_sig: RowMap,
    on_close: Callback<()>,
    on_ns_change: Callback<Option<String>>,
) -> impl IntoView {
    let catalog = expect_context::<Catalog>().0;
    let _detail = expect_context::<RwSignal<Option<DetailTarget>>>();

    let kind_key = config.kind_key.clone();
    let kind: Memo<Option<ResourceKind>> =
        Memo::new(move |_| catalog.get().into_iter().find(|k| k.key == kind_key));

    let text_filter = RwSignal::new(String::new());
    let cfg_sv = StoredValue::new(config.clone());
    let pane_ns: RwSignal<Option<String>> = RwSignal::new(config.namespace);

    // Skip the first run (initial mount) to avoid a spurious ws.update.
    Effect::new(move |prev: Option<()>| {
        let ns = pane_ns.get();
        if prev.is_some() {
            on_ns_change.run(ns);
        }
    });

    view! {
        <div class="pane">
            {move || kind.get().map(|k| {
                let sel = cfg_sv.get_value().selector.clone();
                view! {
                    <KindTable
                        kind=k
                        url_fn=move || None
                        rows_override=rows_sig
                        on_close=on_close
                        namespace=None
                        selector=sel
                        text_filter=text_filter
                        ns_filter=pane_ns
                        keyboard=false />
                }
            })}
        </div>
    }
}

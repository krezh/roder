use std::collections::HashMap;

use leptos::prelude::*;
use roder_core::{ResourceKind, WatchEvent};

use crate::app::components::kind_table::KindTable;
use crate::app::events::RowMap;
use crate::app::hooks::Coalescer;
use crate::app::state::{
    Catalog, ConnectionState, Connectivity, DetailTarget, PaneConfig, WorkspaceConf,
};
use crate::data;

#[component]
pub(crate) fn WorkspaceView() -> impl IntoView {
    let ws = expect_context::<WorkspaceConf>().0;
    let connection = expect_context::<ConnectionState>().0;

    // One row signal per pane (keyed by kind_key). Non-reactive storage so the
    // multi-watch Effect can write into individual signals without loops.
    let pane_rows: StoredValue<HashMap<String, RowMap>> = StoredValue::new(HashMap::new());
    // False until the first SSE Snapshot arrives for each pane. Prevents KindTable
    // from mounting with empty rows on SPA navigation (where catalog is already loaded
    // so KindTable would otherwise appear before the SSE snapshot lands).
    let pane_loaded: StoredValue<HashMap<String, RwSignal<bool>>> =
        StoredValue::new(HashMap::new());

    // Single SSE connection for all workspace panes. Reconnects whenever the
    // pane set or any pane's namespace changes (both are stored in ws.panes).
    let reconnect = RwSignal::new(0u32);
    Effect::new(move |_prev: Option<Option<data::SseHandle>>| {
        reconnect.track();
        let panes = ws.with(|w| w.panes.clone());
        if panes.is_empty() {
            return None;
        }

        // Remove stale entries for panes that were closed. Signals for remaining
        // panes were created by the For item body (stable scope) — never here.
        pane_rows.update_value(|map| {
            map.retain(|k, _| panes.iter().any(|p| &p.kind_key == k));
        });
        pane_loaded.update_value(|map| {
            map.retain(|k, _| panes.iter().any(|p| &p.kind_key == k));
        });

        // Build the watch URL from ws.panes directly. Namespace changes arrive
        // here because on_ns_persist uses ws.update (tracked), so changing a
        // pane's namespace re-runs this Effect and reconnects the SSE stream.
        let url = data::watch_multi_url(
            &panes
                .iter()
                .map(|p| (p.kind_key.as_str(), p.namespace.as_deref()))
                .collect::<Vec<_>>(),
        );
        let probe_url = url.clone();

        // Coalesce the multiplexed burst (one `Applied` per pod per metrics scrape,
        // across every pane) into a single synchronous drain, so each pane's
        // `rows_override` effect — and thus its sort — runs once per burst, not per
        // event. Fresh per (re)subscribe, so it starts with an empty buffer.
        let coalescer = Coalescer::new(move |batch: Vec<(String, WatchEvent)>| {
            for (key, event) in batch {
                let is_snapshot = matches!(event, WatchEvent::Snapshot { .. });
                pane_rows.with_value(|map| {
                    if let Some(&rows) = map.get(&key) {
                        match event {
                            // Workspace panes keep their catalog headers, so the
                            // snapshot's columns aren't tracked here.
                            WatchEvent::Snapshot { rows: r, .. } => {
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
                            WatchEvent::Forbidden { message } => {
                                // The server has already stopped this pane's
                                // informer; surfacing this per-pane in the UI is
                                // a follow-up. Log for now.
                                leptos::logging::warn!("watch forbidden for pane {key}: {message}");
                            }
                        }
                    }
                });
                if is_snapshot {
                    pane_loaded.with_value(|map| {
                        if let Some(&loaded) = map.get(&key) {
                            loaded.set(true);
                        }
                    });
                }
            }
        });

        data::subscribe_multi(
            &url,
            move |key, event| coalescer.push((key, event)),
            move || {
                let url = probe_url.clone();
                leptos::task::spawn_local(async move {
                    connection.set(Connectivity::Error(data::probe_error(url).await));
                });
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
                    // Signals are created here (stable For-item scope) so they are
                    // never owned by the SSE Effect scope and survive Effect re-runs.
                    pane_rows.update_value(|map| {
                        map.entry(cfg.kind_key.clone())
                            .or_insert_with(|| RwSignal::new(HashMap::new()));
                    });
                    let rows_sig = pane_rows
                        .with_value(|map| *map.get(&cfg.kind_key).expect("just inserted"));
                    pane_loaded.update_value(|map| {
                        map.entry(cfg.kind_key.clone())
                            .or_insert_with(|| RwSignal::new(false));
                    });
                    let loaded_sig = pane_loaded
                        .with_value(|map| *map.get(&cfg.kind_key).expect("just inserted"));
                    // Local namespace signal — drives the KindTable dropdown and is
                    // persisted to ws (which re-triggers the SSE Effect) on change.
                    let pane_ns = RwSignal::new(cfg.namespace.clone());
                    let close_key = cfg.kind_key.clone();
                    let persist_key = cfg.kind_key.clone();
                    view! {
                        <PaneView
                            config=cfg
                            rows_sig=rows_sig
                            loaded_sig=loaded_sig
                            pane_ns=pane_ns
                            on_close=Callback::new(move |_| {
                                ws.update(|w| w.panes.retain(|p| p.kind_key != close_key));
                            })
                            on_ns_persist=Callback::new(move |ns: Option<String>| {
                                // Persist namespace to ws. Using update (not update_untracked)
                                // so the SSE Effect re-runs and reconnects with the new namespace.
                                ws.update(|w| {
                                    if let Some(p) =
                                        w.panes.iter_mut().find(|p| p.kind_key == persist_key)
                                    {
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
    loaded_sig: RwSignal<bool>,
    /// Local namespace signal shared with KindTable for the dropdown. Writing
    /// triggers on_ns_persist → ws.update → SSE Effect reconnects.
    pane_ns: RwSignal<Option<String>>,
    on_close: Callback<()>,
    /// Persists the namespace to ws (triggers SSE reconnect via ws.update).
    on_ns_persist: Callback<Option<String>>,
) -> impl IntoView {
    let catalog = expect_context::<Catalog>().0;
    let _detail = expect_context::<RwSignal<Option<DetailTarget>>>();

    let kind_key = config.kind_key.clone();
    let kind: Memo<Option<ResourceKind>> =
        Memo::new(move |_| catalog.get().into_iter().find(|k| k.key == kind_key));

    let text_filter = RwSignal::new(String::new());
    let cfg_sv = StoredValue::new(config.clone());

    Effect::new(move |prev: Option<()>| {
        let ns = pane_ns.get();
        if prev.is_some() {
            on_ns_persist.run(ns);
        }
    });

    view! {
        <div class="pane">
            {move || {
                let k = kind.get();
                let loaded = loaded_sig.get();
                k.and_then(|k| {
                    if !loaded { return None; }
                    let sel = cfg_sv.get_value().selector.clone();
                    Some(view! {
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
                    })
                })
            }}
        </div>
    }
}

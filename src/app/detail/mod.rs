//! Resource detail: a right-docked drawer holding actions, Info / YAML / Logs tabs,
//! and the pod listing for pod-owning workloads.

pub(crate) mod info;
pub(crate) mod metrics;
pub(crate) mod pods;

use leptos::prelude::*;
use roder_core::ObjectDetail;

use crate::app::components::table::ScaleControl;
use crate::app::logs::LogsView;
use crate::app::overlays::confirm::{ask_confirm, Confirm};
use crate::app::state::{DetailTarget, ExecOpen, ExecTarget};
use crate::app::util::format::parse_key;
use crate::app::util::json::selector_from;
use crate::app::util::predicate::KindKind;
use crate::app::util::yaml_hl;
use crate::data;

use self::info::info_view;
use self::metrics::MetricsChart;
use self::pods::PodsTab;

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Tab {
    Info,
    Yaml,
    Logs,
    Metrics,
    Talos,
}

/// Right-docked, drag-resizable detail drawer. Shows `RowDetail` for the currently
/// selected object (the `detail` context signal); slides out when nothing is open.
#[component]
pub(crate) fn DetailDrawer() -> impl IntoView {
    let detail = expect_context::<RwSignal<Option<DetailTarget>>>();
    let width = RwSignal::new(560i32);
    let dragging = RwSignal::new(false);

    #[cfg(target_arch = "wasm32")]
    {
        use leptos::ev;
        let mv = window_event_listener(ev::mousemove, move |e: ev::MouseEvent| {
            if !dragging.get_untracked() {
                return;
            }
            let vw = web_sys::window()
                .and_then(|w| w.inner_width().ok())
                .and_then(|v| v.as_f64())
                .unwrap_or(1280.0);
            let w = (vw - e.client_x() as f64).clamp(360.0, vw * 0.9);
            width.set(w as i32);
            e.prevent_default();
        });
        let up = window_event_listener(ev::mouseup, move |_| {
            if dragging.get_untracked() {
                dragging.set(false);
            }
        });
        on_cleanup(move || {
            mv.remove();
            up.remove();
        });
    }

    view! {
        // Always mounted; `.open` slides it in. `width` drives the style in place so
        // resizing never rebuilds the detail (and re-fetches).
        <div class="detailbar"
            class:open=move || detail.get().is_some()
            class:dragging=move || dragging.get()
            style=move || format!("width:{}px", width.get())>
            <div class="detailbar-resize"
                on:mousedown=move |e: leptos::ev::MouseEvent| { e.prevent_default(); dragging.set(true); }></div>
            <div class="detailbar-head">
                <span class="detailbar-title">{move || detail.get().map(|t| t.name).unwrap_or_default()}</span>
                <button class="detailbar-close" on:click=move |_| detail.set(None)>"✕"</button>
            </div>
            <div class="detailbar-body">
                {move || detail.get().map(|t| view! { <RowDetail target=t on_delete=move || detail.set(None) /> })}
            </div>
        </div>
    }
}

/// Inline detail for an expanded row: actions, a describe-style Info view (default),
/// YAML, and pod logs — selectable via tabs.
///
/// `on_delete` needs `Send + Sync` (unlike similar close-callback params
/// elsewhere, e.g. `TreeContent`'s `do_close`) because it's captured into the
/// `run` closure below, which in turn is captured by several `<Show
/// fallback=...>...</Show>` blocks in the actions section — `Show`'s children
/// go through `TypedChildrenFn`, whose `ToChildren` impl requires `Sync` in
/// addition to the `Send` that `.into_any()` alone would need for the tab
/// content further down.
#[component]
pub(crate) fn RowDetail(
    target: DetailTarget,
    on_delete: impl Fn() + Copy + 'static + Send + Sync,
) -> impl IntoView {
    let requested_tab = expect_context::<RwSignal<Option<Tab>>>();
    let confirm = expect_context::<RwSignal<Option<Confirm>>>();
    let status = RwSignal::new(None::<Result<String, String>>);
    let yaml = RwSignal::new(String::new());
    // Honor a tab requested via the context menu (e.g. "Logs"), then clear it.
    let initial_tab = requested_tab.get_untracked().unwrap_or(Tab::Info);
    if requested_tab.get_untracked().is_some() {
        requested_tab.set(None);
    }
    let tab = RwSignal::new(initial_tab);
    let yaml_editing = RwSignal::new(false);

    let (group, kind) = parse_key(&target.key);
    let kk = KindKind::new(&group, &kind);
    let is_workload = kk.is_workload();
    let is_scalable = kk.is_scalable();
    let is_flux = kk.is_flux();
    let is_helmrelease = kk.is_helmrelease();
    let has_source_ref = kk.has_source_ref();
    let is_eso = kk.is_eso();
    let is_pod = kk.is_pod();
    let is_node = kk.is_node();
    let features = LocalResource::new(move || async move {
        if !is_node {
            return false;
        }
        data::fetch_json::<serde_json::Value>("/api/features")
            .await
            .ok()
            .and_then(|v| v.get("talos").and_then(|v| v.as_bool()))
            .unwrap_or(false)
    });
    let talos_available = move || features.get().is_some_and(|enabled| enabled);
    // Pod-owning resources get a live "Pods" tab listing their pods by selector.
    let has_pods = is_workload || kk.is_job();
    let is_cronjob = kk.is_cronjob();
    let ns = target.namespace.clone().unwrap_or_default();
    let pod = target.name.clone();
    let exec_open = expect_context::<ExecOpen>().0;

    let tv = StoredValue::new(target.clone());
    let kind_sv = StoredValue::new(kind.clone());

    let t_obj = target.clone();
    let obj = LocalResource::new(move || {
        let t = t_obj.clone();
        async move {
            data::fetch_json::<ObjectDetail>(&data::detail_url(
                &t.key,
                t.namespace.as_deref(),
                &t.name,
            ))
            .await
            .ok()
        }
    });
    // Live spec.replicas (for pre-filling the scale input).
    let current_replicas = Memo::new(move |_| {
        obj.get().flatten().and_then(|d| {
            d.object
                .get("spec")
                .and_then(|s| s.get("replicas"))
                .and_then(|r| r.as_i64())
                .map(|n| n as i32)
        })
    });
    // Whether to show "Suspend" or "Resume": read once from the fetched object,
    // then refetched (see `run`) after a flux-suspend/flux-resume call so the
    // button reflects the persisted `spec.suspend` rather than flipping blindly.
    let is_suspended = move || {
        obj.get()
            .flatten()
            .and_then(|d| {
                d.object
                    .get("spec")
                    .and_then(|s| s.get("suspend"))
                    .and_then(|b| b.as_bool())
            })
            .unwrap_or(false)
    };
    Effect::new(move |_| {
        if let Some(Some(d)) = obj.get() {
            yaml.set(d.yaml.clone());
        }
    });

    let t_perm = target.clone();
    let perms = LocalResource::new(move || {
        let t = t_perm.clone();
        async move {
            data::fetch_json::<serde_json::Value>(&format!(
                "/api/permissions?key={}&namespace={}",
                t.key,
                t.namespace
                    .as_deref()
                    .map(data::percent_encode)
                    .unwrap_or_default()
            ))
            .await
            .ok()
        }
    });
    let can_patch = move || {
        perms
            .get()
            .flatten()
            .and_then(|v| v.get("patch").and_then(|b| b.as_bool()))
            .unwrap_or(false)
    };
    let can_delete = move || {
        perms
            .get()
            .flatten()
            .and_then(|v| v.get("delete").and_then(|b| b.as_bool()))
            .unwrap_or(false)
    };

    let run = move |action: &'static str, extra: serde_json::Value| {
        let t = tv.get_value();
        let mut body = serde_json::json!({
            "action": action, "key": t.key, "namespace": t.namespace, "name": t.name,
        });
        if let (Some(o), Some(ex)) = (body.as_object_mut(), extra.as_object()) {
            for (k, v) in ex {
                o.insert(k.clone(), v.clone());
            }
        }
        leptos::task::spawn_local(async move {
            match data::post_action(&body).await {
                Ok(_) => {
                    status.set(Some(Ok(format!("{action} ✓"))));
                    if action == "delete" {
                        on_delete();
                    }
                    if action == "flux-suspend" || action == "flux-resume" {
                        obj.refetch();
                    }
                }
                Err(e) => status.set(Some(Err(e))),
            }
        });
    };

    view! {
        <div class="rd">
            <div class="actions">
                {is_workload.then(|| view! {
                    <Show when=can_patch fallback=|| ()>
                        <button class="act" on:click=move |_| run("restart", serde_json::json!({}))>"Restart"</button>
                        {is_scalable.then(|| view! { <ScaleControl run=run current=current_replicas /> })}
                    </Show>
                })}
                {is_flux.then(|| view! {
                    <Show when=can_patch fallback=|| ()>
                        <button class="act" on:click=move |_| run("flux-reconcile", serde_json::json!({}))>"Reconcile"</button>
                        {has_source_ref.then(|| view! {
                            <button class="act" on:click=move |_| run("flux-reconcile-with-source", serde_json::json!({}))>"Reconcile w/ source"</button>
                        })}
                        {is_helmrelease.then(|| view! {
                            <button class="act" on:click=move |_| run("flux-force", serde_json::json!({}))>"Force"</button>
                            <button class="act" on:click=move |_| run("flux-reset", serde_json::json!({}))>"Reset"</button>
                        })}
                        <Show when=move || !is_suspended() fallback=|| ()>
                            <button class="act" on:click=move |_| run("flux-suspend", serde_json::json!({}))>"Suspend"</button>
                        </Show>
                        <Show when=is_suspended fallback=|| ()>
                            <button class="act" on:click=move |_| run("flux-resume", serde_json::json!({}))>"Resume"</button>
                        </Show>
                    </Show>
                })}
                {is_eso.then(|| view! {
                    <Show when=can_patch fallback=|| ()>
                        <button class="act" on:click=move |_| run("eso-refresh", serde_json::json!({}))>"Refresh"</button>
                    </Show>
                })}
                {is_cronjob.then(|| view! {
                    <Show when=can_patch fallback=|| ()>
                        <button class="act" on:click=move |_| run("cronjob-trigger", serde_json::json!({}))>"Trigger"</button>
                    </Show>
                })}
                {is_pod.then(|| {
                    let exec_ns  = ns.clone();
                    let exec_pod = pod.clone();
                    view! {
                        <button class="act" on:click=move |_| {
                            exec_open.set(Some(ExecTarget {
                                namespace: exec_ns.clone(),
                                pod: exec_pod.clone(),
                                container: None,
                                pending: false,
                                node_shell: false,
                            }));
                        }>"Shell"</button>
                    }
                })}
                {move || can_delete().then(|| view! {
                    <button class="act danger" on:click=move |_| {
                        ask_confirm(confirm, "Delete this resource?", move || run("delete", serde_json::json!({})));
                    }>"Delete"</button>
                })}
                {move || status.get().map(|s| match s {
                    Ok(m) => view! { <span class="act-ok">{m}</span> }.into_any(),
                    Err(e) => view! { <span class="act-err">{e}</span> }.into_any(),
                })}
            </div>

            <div class="rd-tabs">
                <button class="rd-tab" class:active=move || tab.get() == Tab::Info on:click=move |_| tab.set(Tab::Info)>"Info"</button>
                <button class="rd-tab" class:active=move || tab.get() == Tab::Yaml on:click=move |_| tab.set(Tab::Yaml)>"YAML"</button>
                {is_pod.then(|| view! {
                    <button class="rd-tab" class:active=move || tab.get() == Tab::Metrics on:click=move |_| tab.set(Tab::Metrics)>"Metrics"</button>
                    <button class="rd-tab" class:active=move || tab.get() == Tab::Logs on:click=move |_| tab.set(Tab::Logs)>"Logs"</button>
                })}
                {move || talos_available().then(|| view! {
                    <button class="rd-tab" class:active=move || tab.get() == Tab::Talos on:click=move |_| tab.set(Tab::Talos)>"Talos"</button>
                })}
            </div>

            <Suspense fallback=|| view! { <div class="pad muted">"Loading…"</div> }>
                {move || obj.get().flatten().map(|d| {
                    let ns = ns.clone();
                    let pod = pod.clone();
                    // Pod-owning resources list their pods right under the tabs (not as a tab).
                    let pods_section = has_pods.then(|| {
                        let sel = selector_from(&d.object);
                        let pns = d.namespace.clone().unwrap_or_default();
                        view! { <div class="rd-pods"><h4>"Pods"</h4><PodsTab namespace=pns selector=sel /></div> }
                    });
                    let d = d.clone();
                    let tab_content = move || match tab.get() {
                        Tab::Info => info_view(d.clone(), kind_sv.get_value()).into_any(),
                        Tab::Logs => view! { <LogsView url=format!("/api/logs?namespace={}&pod={}", data::percent_encode(&ns), data::percent_encode(&pod)) /> }.into_any(),
                        Tab::Metrics => view! { <MetricsChart namespace=ns.clone() name=pod.clone() /> }.into_any(),
                        Tab::Talos => view! { <TalosNodeView node=pod.clone() /> }.into_any(),
                        Tab::Yaml => view! {
                            <div class="yaml-pane">
                                <div class="yaml-head">
                                    <h4>"YAML"</h4>
                                    {move || can_patch().then(|| view! {
                                        <Show when=move || yaml_editing.get() fallback=|| ()>
                                            <button class="act"
                                                on:click=move |_| run("apply", serde_json::json!({ "yaml": yaml.get() }))>
                                                "Apply"
                                            </button>
                                        </Show>
                                        <button class="act"
                                            on:click=move |_| yaml_editing.update(|e| *e = !*e)>
                                            {move || if yaml_editing.get() { "View" } else { "Edit" }}
                                        </button>
                                    })}
                                </div>
                                {move || {
                                    if yaml_editing.get() {
                                        view! {
                                            <textarea class="yaml-edit" spellcheck="false"
                                                prop:value=move || yaml.get()
                                                on:input=move |e| yaml.set(event_target_value(&e))>
                                            </textarea>
                                        }.into_any()
                                    } else {
                                        let highlighted = yaml_hl::highlight_yaml(&yaml.get());
                                        view! {
                                            <pre class="yaml-view" inner_html=highlighted></pre>
                                        }.into_any()
                                    }
                                }}
                            </div>
                        }.into_any(),
                    };
                    view! { {pods_section} {tab_content} }
                })}
            </Suspense>
        </div>
    }
}

#[component]
fn TalosNodeView(node: String) -> impl IntoView {
    let confirm = expect_context::<RwSignal<Option<Confirm>>>();
    let action_status = RwSignal::new(None::<Result<String, String>>);
    let status_node = node.clone();
    let status = LocalResource::new(move || {
        let node = status_node.clone();
        async move {
            data::fetch_json::<roder_core::TalosNode>(&format!(
                "/api/talos/node?node={}",
                data::percent_encode(&node)
            ))
            .await
        }
    });
    let dmesg = LocalResource::new({
        let node = node.clone();
        move || {
            let node = node.clone();
            async move {
                data::fetch_json::<serde_json::Value>(&format!(
                    "/api/talos/dmesg?node={}",
                    data::percent_encode(&node)
                ))
                .await
                .map(|v| {
                    v.get("log")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string()
                })
            }
        }
    });

    view! {
        <Suspense fallback=|| view! { <div class="pad muted">"Loading Talos status…"</div> }>
            {move || {
                let node = node.clone();
                status.get().map(move |result| match result {
                Err(e) => view! { <div class="pad muted">{format!("Talos integration unavailable: {e}")}</div> }.into_any(),
                Ok(s) => {
                    let services = s.services;
                    let mounts = s.mounts;
                    let interfaces = s.interfaces;
                    let reboot_node = node.clone();
                    let shutdown_node = node.clone();
                    let services_node = node.clone();
                    view! {
                        <div class="info">
                            <div class="kv-grid">
                                <div class="kv"><span class="k">"Version"</span><span class="v">{s.version}</span></div>
                            </div>
                            <div class="actions">
                                <button class="act danger" on:click=move |_| {
                                    let target = reboot_node.clone();
                                    ask_confirm(confirm, "Reboot this Talos node?", move || talos_power_action(target.clone(), "reboot", action_status));
                                }>"Reboot"</button>
                                <button class="act danger" on:click=move |_| {
                                    let target = shutdown_node.clone();
                                    ask_confirm(confirm, "Shut down this Talos node?", move || talos_power_action(target.clone(), "shutdown", action_status));
                                }>"Shutdown"</button>
                                {move || action_status.get().map(|result| match result {
                                    Ok(message) => view! { <span class="act-ok">{message}</span> }.into_any(),
                                    Err(error) => view! { <span class="act-err">{error}</span> }.into_any(),
                                })}
                            </div>
                            <h4>"Services"</h4>
                            <table class="cond"><thead><tr><th>"Service"</th><th>"State"</th><th>"Health details"</th><th>"Actions"</th></tr></thead>
                                <tbody>{services.into_iter().map(|svc| {
                                    let service = svc.id.clone();
                                    let service_for_stop = service.clone();
                                    let service_for_restart = service.clone();
                                    let node_for_start = services_node.clone();
                                    let node_for_stop = services_node.clone();
                                    let node_for_restart = services_node.clone();
                                    let health = if svc.health_unknown { "Unknown" } else if svc.healthy { "Healthy" } else { "Unhealthy" };
                                    let details = [
                                        (!svc.message.is_empty()).then_some(svc.message),
                                        svc.last_change.map(|v| format!("changed {v}")),
                                        svc.events.last().map(|e| format!("{}: {}", e.state, e.message)),
                                    ].into_iter().flatten().collect::<Vec<_>>().join(" · ");
                                    view! {
                                        <tr><td>{svc.id}</td><td>{svc.state}</td><td><b>{health}</b>{(!details.is_empty()).then(|| view! { <div class="muted">{details}</div> })}</td>
                                        <td><div class="actions">
                                            <button class="act" on:click=move |_| talos_service_action(node_for_start.clone(), service.clone(), "start", action_status)>"Start"</button>
                                            <button class="act" on:click=move |_| talos_service_action(node_for_stop.clone(), service_for_stop.clone(), "stop", action_status)>"Stop"</button>
                                            <button class="act" on:click=move |_| talos_service_action(node_for_restart.clone(), service_for_restart.clone(), "restart", action_status)>"Restart"</button>
                                        </div></td></tr>
                                    }
                                }).collect_view()}</tbody>
                            </table>
                            <h4>"Mounts"</h4>
                            <table class="cond"><thead><tr><th>"Path"</th><th>"Filesystem"</th><th>"Used"</th></tr></thead>
                                <tbody>{mounts.into_iter().map(|m| {
                                    let pct = if m.size == 0 { 0.0 } else { 100.0 * (m.size - m.available) as f64 / m.size as f64 };
                                    view! { <tr><td>{m.mounted_on}</td><td>{m.filesystem}</td><td>{format!("{pct:.1}%")}</td></tr> }
                                }).collect_view()}</tbody>
                            </table>
                            <h4>"Disk I/O"</h4>
                            <table class="cond"><thead><tr><th>"Device"</th><th>"Read"</th><th>"Written"</th><th>"Operations"</th><th>"Active I/O"</th></tr></thead>
                                <tbody>{s.disks.into_iter().map(|d| view! {
                                    <tr><td>{d.name}</td><td>{format_bytes(d.read_bytes)}</td><td>{format_bytes(d.write_bytes)}</td><td>{format!("{} / {}", d.reads, d.writes)}</td><td>{format!("{} ({} ms)", d.io_in_progress, d.io_time_ms)}</td></tr>
                                }).collect_view()}</tbody>
                            </table>
                            <h4>"Network"</h4>
                            <table class="cond"><thead><tr><th>"Interface"</th><th>"RX"</th><th>"TX"</th><th>"Errors / Drops"</th></tr></thead>
                                <tbody>{interfaces.into_iter().map(|i| view! {
                                    <tr><td>{i.name}</td><td>{format_bytes(i.rx_bytes)}</td><td>{format_bytes(i.tx_bytes)}</td><td>{format!("{} / {}", i.rx_errors + i.tx_errors, i.rx_dropped + i.tx_dropped)}</td></tr>
                                }).collect_view()}</tbody>
                            </table>
                            <h4>"Kernel log"</h4>
                            <Suspense fallback=|| view! { <div class="muted">"Loading kernel log…"</div> }>
                                {move || dmesg.get().map(|result| match result {
                                    Ok(log) => view! { <pre class="yaml-view">{log}</pre> }.into_any(),
                                    Err(error) => view! { <div class="muted">{error}</div> }.into_any(),
                                })}
                            </Suspense>
                        </div>
                    }.into_any()
                }
                })
            }}
        </Suspense>
    }
}

fn talos_service_action(
    node: String,
    service: String,
    action: &'static str,
    status: RwSignal<Option<Result<String, String>>>,
) {
    leptos::task::spawn_local(async move {
        status.set(None);
        let result = data::post_action(&serde_json::json!({
            "action": format!("talos-service-{action}"),
            "name": node,
            "service": service,
        }))
        .await
        .map(|_| format!("service {action} requested"));
        status.set(Some(result));
    });
}

fn talos_power_action(
    node: String,
    action: &'static str,
    status: RwSignal<Option<Result<String, String>>>,
) {
    leptos::task::spawn_local(async move {
        status.set(None);
        let result = data::post_action(&serde_json::json!({
            "action": format!("talos-{action}"),
            "name": node,
        }))
        .await
        .map(|_| format!("{action} requested"));
        status.set(Some(result));
    });
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

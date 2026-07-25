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
use crate::app::overlays::delete::{ask_delete, delete_extra, DeleteRequest};
use crate::app::state::{DetailTarget, DrainOpen, DrainTarget, ExecOpen, ExecTarget};
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
    let (snapshot, closing, do_close) = crate::app::overlays::use_option_overlay(detail);

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
            class:open=move || snapshot.get().is_some()
            class:closing=move || closing.get()
            class:dragging=move || dragging.get()
            style=move || format!("width:{}px", width.get())>
            <div class="detailbar-resize"
                on:mousedown=move |e: leptos::ev::MouseEvent| { e.prevent_default(); dragging.set(true); }></div>
            <div class="detailbar-head">
                <span class="detailbar-title">{move || snapshot.get().map(|t| t.name).unwrap_or_default()}</span>
                <button class="detailbar-close" on:click=move |_| do_close()>"✕"</button>
            </div>
            <div class="detailbar-body">
                {move || snapshot.get().map(|t| view! { <RowDetail target=t on_delete=move || do_close() /> })}
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
    let delete_confirm = expect_context::<RwSignal<Option<DeleteRequest>>>();
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
            return roder_core::TalosCapabilities::default();
        }
        data::fetch_json::<serde_json::Value>("/api/features")
            .await
            .ok()
            .and_then(|v| v.get("talos").cloned())
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default()
    });
    let talos_available = move || features.get().is_some_and(|caps| caps.read);
    let talos_actions = move || features.get().is_some_and(|caps| caps.actions);
    let talos_config = move || features.get().is_some_and(|caps| caps.config);
    // Pod-owning resources get a live "Pods" tab listing their pods by selector.
    let has_pods = is_workload || kk.is_job();
    let is_cronjob = kk.is_cronjob();
    let is_kopiur_snapshot_policy = kk.is_kopiur_snapshot_policy();
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
                {is_kopiur_snapshot_policy.then(|| view! {
                    <Show when=can_patch fallback=|| ()>
                        <button class="act" on:click=move |_| run("kopiur-snapshot-now", serde_json::json!({}))>"Snapshot Now"</button>
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
                                image: String::new(),
                            }));
                        }>"Shell"</button>
                    }
                })}
                {move || can_delete().then(|| view! {
                    <button class="act danger" on:click=move |_| {
                        ask_delete(delete_confirm, "Delete this resource?", move |force, propagation| {
                            run("delete", delete_extra(force, propagation));
                        });
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
                })}
                {move || (is_pod || talos_available()).then(|| view! {
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
                        Tab::Logs => if is_pod {
                            view! { <LogsView url=format!("/api/logs?namespace={}&pod={}", data::percent_encode(&ns), data::percent_encode(&pod)) /> }.into_any()
                        } else {
                            view! { <LogsView url=format!("/api/talos/dmesg?node={}", data::percent_encode(&pod)) /> }.into_any()
                        },
                        Tab::Metrics => view! { <MetricsChart namespace=ns.clone() name=pod.clone() /> }.into_any(),
                        Tab::Talos => view! { <TalosNodeView node=pod.clone() key=tv.get_value().key actions=talos_actions() config=talos_config() /> }.into_any(),
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
fn TalosNodeView(node: String, key: String, actions: bool, config: bool) -> impl IntoView {
    let confirm = expect_context::<RwSignal<Option<Confirm>>>();
    let drain_open = expect_context::<DrainOpen>().0;
    let action_status = RwSignal::new(None::<Result<String, String>>);
    let pending_action = RwSignal::new(None::<String>);
    let drain_first = RwSignal::new(true);
    let load_config_diff = RwSignal::new(false);
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
    let config_diff = LocalResource::new({
        let node = node.clone();
        move || {
            let node = node.clone();
            let should_load = load_config_diff.get();
            async move {
                if !should_load {
                    return Ok(None);
                }
                data::fetch_json::<roder_core::TalosConfigDiff>(&format!(
                    "/api/talos/config-diff?node={}",
                    data::percent_encode(&node)
                ))
                .await
                .map(Some)
            }
        }
    });
    let refresh = Callback::new(move |_| status.refetch());

    view! {
        <Suspense fallback=|| view! { <div class="pad muted">"Loading Talos status…"</div> }>
            {move || {
                let node = node.clone();
                let key = key.clone();
                status.get().map(move |result| match result {
                Err(e) => view! { <div class="pad muted">{format!("Talos integration unavailable: {e}")}</div> }.into_any(),
                Ok(s) => {
                    let services = s.services;
                    let interfaces = s.interfaces;
                    let disk_inventory = s.disk_inventory;
                    let volumes = s.volumes;
                    let disks = s.disks;
                    let config_fingerprint = s.config_fingerprint;
                    let errors = s.errors;
                    let control_plane = s.control_plane;
                    let service_count = services.len();
                    let service_unknown = services.iter().filter(|service| service.health_unknown).count();
                    let service_running = services.iter().filter(|service| service.state.to_ascii_lowercase().contains("running")).count();
                    let service_issues = services.iter().filter(|service| {
                        !service.state.to_ascii_lowercase().contains("running")
                            || (!service.health_unknown && !service.healthy)
                    }).count();
                    let service_summary = if service_issues > 0 {
                        format!("{service_count} total · {service_issues} need attention")
                    } else if service_unknown > 0 {
                        format!("{service_running} running · {service_unknown} unmonitored")
                    } else {
                        format!("{service_running} running · all healthy")
                    };
                    let service_note_class = if service_issues > 0 { "detail-stat-note warning" } else { "detail-stat-note" };
                    let services_open = service_issues > 0;
                    let storage_summary = format!("{} drives · {} partitions", disk_inventory.len(), volumes.len());
                    let network_up = interfaces.iter().filter(|interface| interface.link_up == Some(true)).count();
                    let network_down = interfaces.iter().filter(|interface| interface.link_up == Some(false)).count();
                    let network_summary = format!("{network_up} up · {network_down} inactive");
                    let version = if s.version.is_empty() { "Unavailable".into() } else { s.version };
                    let fingerprint = config_fingerprint.as_ref().map(|value| short_fingerprint(value));
                    let reboot_node = node.clone();
                    let shutdown_node = node.clone();
                    let services_node = node.clone();
                    let reboot_key = key.clone();
                    let shutdown_key = key.clone();
                    view! {
                        <div class="info talos-view">
                            <div class="detail-stats">
                                <div class="detail-stat"><span class="detail-stat-label">"Talos"</span><b class="detail-stat-value">{version}</b><span class="detail-stat-note">{if control_plane { "control plane" } else { "worker" }}</span></div>
                                <div class="detail-stat"><span class="detail-stat-label">"Services"</span><b class="detail-stat-value">{format!("{service_running}/{service_count} running")}</b><span class=service_note_class>{if service_issues > 0 { format!("{service_issues} need attention") } else if service_unknown > 0 { format!("{service_unknown} without health checks") } else { "all health checks passing".into() }}</span></div>
                                <div class="detail-stat"><span class="detail-stat-label">"Storage"</span><b class="detail-stat-value">{format!("{} drives", disk_inventory.len())}</b><span class="detail-stat-note">{format!("{} partitions", volumes.len())}</span></div>
                                <div class="detail-stat"><span class="detail-stat-label">"Network"</span><b class="detail-stat-value">{format!("{network_up}/{} links up", interfaces.len())}</b><span class="detail-stat-note">{format!("{network_down} inactive")}</span></div>
                            </div>
                            {fingerprint.map(|value| view! { <div class="talos-fingerprint"><span>"Config"</span><code>{value}</code></div> })}
                            {(!errors.is_empty()).then(|| view! {
                                <div class="talos-errors">
                                    {errors.into_iter().map(|(section, error)| view! {
                                        <div class="warn-line"><b>{section}</b>": "{error}</div>
                                    }).collect_view()}
                                </div>
                            })}
                            {actions.then(|| view! { <div class="actions talos-power-actions">
                                <label class="talos-drain"><input type="checkbox" prop:checked=move || drain_first.get()
                                    on:change=move |e| drain_first.set(event_target_checked(&e)) />"Drain first"</label>
                                <button class="act danger" disabled=move || pending_action.get().is_some() on:click=move |_| {
                                    let target = reboot_node.clone();
                                    if drain_first.get_untracked() {
                                        // The etcd-quorum warning for a control-plane node
                                        // moves into the drain dialog itself (see
                                        // `overlays::drain`) — it shows there instead of here.
                                        drain_open.set(Some(DrainTarget {
                                            key: reboot_key.clone(),
                                            name: target,
                                            power: Some("reboot".to_string()),
                                            control_plane,
                                            job: None,
                                        }));
                                    } else {
                                        let warning = if control_plane { " This is a control-plane node; verify etcd quorum before continuing." } else { "" };
                                        ask_confirm(confirm, format!("Reboot {target}?{warning}"), "Reboot", move || talos_power_action(target.clone(), "reboot", action_status, pending_action));
                                    }
                                }>"Reboot"</button>
                                <button class="act danger" disabled=move || pending_action.get().is_some() on:click=move |_| {
                                    let target = shutdown_node.clone();
                                    if drain_first.get_untracked() {
                                        drain_open.set(Some(DrainTarget {
                                            key: shutdown_key.clone(),
                                            name: target,
                                            power: Some("shutdown".to_string()),
                                            control_plane,
                                            job: None,
                                        }));
                                    } else {
                                        let warning = if control_plane { " This is a control-plane node; verify etcd quorum before continuing." } else { "" };
                                        ask_confirm(confirm, format!("Shut down {target}?{warning}"), "Shut down", move || talos_power_action(target.clone(), "shutdown", action_status, pending_action));
                                    }
                                }>"Shutdown"</button>
                                {move || pending_action.get().map(|action| view! {
                                    <span class="muted">{if action == "reboot" { "Waiting for node recovery…".to_string() } else { format!("Running {action}…") }}</span>
                                })}
                                {move || action_status.get().map(|result| match result {
                                    Ok(message) => view! { <span class="act-ok">{message}</span> }.into_any(),
                                    Err(error) => view! { <span class="act-err">{error}</span> }.into_any(),
                                })}
                            </div> })}
                            <div class="talos-sections">
                            <details class="talos-section" open=services_open>
                            <summary><span>"Services"</span><span class=if service_issues == 0 { "talos-section-meta" } else { "talos-section-meta warning" }>{service_summary}</span></summary>
                            <div class="talos-section-body"><table class="cond talos-data-table talos-service-table"><thead><tr><th>"Service"</th><th>"State"</th><th>"Health"</th>{actions.then(|| view! { <th class="talos-service-actions">"Actions"</th> })}</tr></thead>
                                <tbody>{services.into_iter().map(|svc| {
                                    let service = svc.id.clone();
                                    let service_for_stop = service.clone();
                                    let service_for_restart = service.clone();
                                    let node_for_start = services_node.clone();
                                    let node_for_stop = services_node.clone();
                                    let node_for_restart = services_node.clone();
                                    let running = svc.state.to_ascii_lowercase().contains("running");
                                    let health = if svc.health_unknown { "Unknown" } else if svc.healthy { "Healthy" } else { "Unhealthy" };
                                    let details = (!svc.message.is_empty() && svc.message != "Health check successful").then_some(svc.message);
                                    view! {
                                        <tr><td><b>{svc.id}</b></td><td>{svc.state}</td><td><span class=if svc.health_unknown { "talos-health unknown" } else if svc.healthy { "talos-health healthy" } else { "talos-health unhealthy" }>{health}</span>{details.map(|detail| view! { <div class="muted">{detail}</div> })}</td>
                                        {actions.then(|| view! { <td class="talos-service-actions"><div class="actions">
                                            {(actions && !running).then(|| view! {
                                                <button class="act" disabled=move || pending_action.get().is_some()
                                                    on:click=move |_| talos_service_action(node_for_start.clone(), service.clone(), "start", action_status, pending_action, refresh)>"Start"</button>
                                            })}
                                            {(actions && running).then(|| view! {
                                                <button class="act" disabled=move || pending_action.get().is_some() on:click=move |_| {
                                                    let node = node_for_stop.clone();
                                                    let service = service_for_stop.clone();
                                                    ask_confirm(confirm, format!("Stop Talos service {service} on {node}?"), "Stop", move || talos_service_action(node.clone(), service.clone(), "stop", action_status, pending_action, refresh));
                                                }>"Stop"</button>
                                                <button class="act" disabled=move || pending_action.get().is_some() on:click=move |_| {
                                                    let node = node_for_restart.clone();
                                                    let service = service_for_restart.clone();
                                                    ask_confirm(confirm, format!("Restart Talos service {service} on {node}?"), "Restart", move || talos_service_action(node.clone(), service.clone(), "restart", action_status, pending_action, refresh));
                                                }>"Restart"</button>
                                            })}
                                        </div></td> })}</tr>
                                    }
                                }).collect_view()}</tbody>
                            </table></div>
                            </details>
                            <details class="talos-section">
                            <summary><span>"Storage"</span><span class="talos-section-meta">{storage_summary}</span></summary>
                            <div class="talos-section-body">
                            {(!disk_inventory.is_empty()).then(|| view! {
                                <h5>"Physical disks"</h5>
                                <table class="cond talos-data-table talos-disk-table"><thead><tr><th>"Device"</th><th>"Model"</th><th>"Capacity"</th><th>"Type"</th><th>"Lifetime I/O"</th><th>"Serial / WWID"</th></tr></thead>
                                    <tbody>{disk_inventory.into_iter().map(|disk| {
                                        let io = disks.iter().find(|stat| stat.name == disk.name || disk.path.ends_with(&format!("/{}", stat.name)));
                                        let kind = match (disk.rotational, disk.readonly) {
                                            (true, true) => "HDD · read-only",
                                            (true, false) => "HDD",
                                            (false, true) => "SSD · read-only",
                                            (false, false) => "SSD",
                                        };
                                        view! {
                                            <tr><td class="font-mono"><b>{disk.path}</b></td><td>{disk.model.unwrap_or_else(|| "—".into())}</td><td>{format_bytes(disk.size)}</td>
                                                <td>{kind}{disk.transport.map(|transport| view! { <div class="muted">{transport}</div> })}</td>
                                                <td>{io.map(|stat| view! { <div class="talos-disk-io"><span><i>"R"</i>{format_bytes(stat.read_bytes)}</span><span><i>"W"</i>{format_bytes(stat.write_bytes)}</span></div> }.into_any()).unwrap_or_else(|| view! { <span class="muted">"Unavailable"</span> }.into_any())}</td>
                                                <td class="font-mono">{disk.serial.or(disk.wwid).unwrap_or_else(|| "—".into())}</td></tr>
                                        }
                                    }).collect_view()}</tbody>
                                </table>
                            })}
                            {(!volumes.is_empty()).then(|| view! {
                                <h5>"Partitions"</h5>
                                <table class="cond talos-data-table talos-volume-table"><thead><tr><th>"Volume"</th><th>"Device"</th><th>"Filesystem"</th><th>"Capacity"</th><th>"Usage"</th><th>"State"</th></tr></thead>
                                    <tbody>{volumes.into_iter().map(|volume| {
                                        let total = volume.used_bytes.zip(volume.available_bytes).map(|(used, available)| used + available);
                                        let percent = volume.used_bytes.zip(total).and_then(|(used, total)| (total > 0).then_some(100.0 * used as f64 / total as f64));
                                        let phase = if volume.phase.is_empty() { "unknown".into() } else { volume.phase };
                                        view! { <tr>
                                            <td><b>{volume.name}</b>{volume.encryption.map(|provider| view! { <div class="muted">{format!("encrypted · {provider}")}</div> })}</td>
                                            <td class="font-mono">{volume.path}</td>
                                            <td>{volume.filesystem.unwrap_or_else(|| "—".into())}</td>
                                            <td>{format_bytes(volume.size)}</td>
                                            <td>{match percent {
                                                Some(percent) => view! { <div class="talos-usage"><span><i style=format!("width:{percent:.1}%")></i></span><b>{format!("{percent:.1}%")}</b></div> }.into_any(),
                                                None => view! { <span class="muted">"Not mounted"</span> }.into_any(),
                                            }}</td>
                                            <td>{phase}</td>
                                        </tr> }
                                    }).collect_view()}</tbody>
                                </table>
                            })}
                            </div>
                            </details>
                            <details class="talos-section">
                            <summary><span>"Network"</span><span class="talos-section-meta">{network_summary}</span></summary>
                            <div class="talos-section-body">
                            <table class="cond talos-data-table"><thead><tr><th>"Interface"</th><th>"Link"</th><th>"Addresses"</th><th>"Hardware"</th><th>"RX / TX"</th><th>"Errors / Drops"</th></tr></thead>
                                <tbody>{interfaces.into_iter().map(|i| view! {
                                    <tr><td><b>{i.name}</b>{i.kind.map(|kind| view! { <div class="muted">{kind}</div> })}</td>
                                        <td class=if i.link_up == Some(false) { "error" } else { "" }>
                                            {i.operational_state.unwrap_or_else(|| "unknown".into())}
                                            {i.speed_mbps.map(|speed| view! { <div class="muted">{format!("{speed} Mbps {}", i.duplex.unwrap_or_default())}</div> })}
                                        </td>
                                        <td class="font-mono">{if i.addresses.is_empty() { "—".into() } else { i.addresses.join(" · ") }}</td>
                                        <td>{i.hardware_address.unwrap_or_else(|| "—".into())}{i.mtu.map(|mtu| view! { <div class="muted">{format!("MTU {mtu}")}</div> })}</td>
                                        <td>{format!("{} / {}", format_bytes(i.rx_bytes), format_bytes(i.tx_bytes))}</td>
                                        <td>{format!("{} / {}", i.rx_errors + i.tx_errors, i.rx_dropped + i.tx_dropped)}</td></tr>
                                }).collect_view()}</tbody>
                            </table></div>
                            </details>
                            {config.then(|| view! {
                                <details class="talos-section">
                                <summary><span>"Configuration"</span><span class="talos-section-meta">{config_fingerprint.as_ref().map(|value| short_fingerprint(value)).unwrap_or_else(|| "unavailable".into())}</span></summary>
                                <div class="talos-section-body">
                                {move || (!load_config_diff.get()).then(|| view! {
                                    <button class="act" on:click=move |_| load_config_diff.set(true)>"Compare node configurations"</button>
                                })}
                                <Suspense fallback=|| view! { <div class="muted">"Comparing redacted configurations…"</div> }>
                                    {move || config_diff.get().map(|result| match result {
                                        Ok(Some(diff)) => view! {
                                            <div class="talos-config-diff">
                                                {diff.peers.into_iter().map(|peer| {
                                                    let summary = match peer.matches {
                                                        Some(true) => "matches".to_string(),
                                                        Some(false) => format!("{} differences", peer.differences.len()),
                                                        None => "unavailable".to_string(),
                                                    };
                                                    view! { <details>
                                                        <summary><b>{peer.node}</b>" — "{summary}</summary>
                                                        {peer.error.map(|error| view! { <div class="act-err">{error}</div> })}
                                                        {(!peer.differences.is_empty()).then(|| view! {
                                                            <table class="cond"><thead><tr><th>"Path"</th><th>"This node"</th><th>"Peer"</th></tr></thead>
                                                                <tbody>{peer.differences.into_iter().map(|difference| view! {
                                                                    <tr><td class="font-mono">{difference.path}</td>
                                                                        <td>{difference.node_value.unwrap_or_else(|| "<missing>".into())}</td>
                                                                        <td>{difference.peer_value.unwrap_or_else(|| "<missing>".into())}</td></tr>
                                                                }).collect_view()}</tbody>
                                                            </table>
                                                        })}
                                                    </details> }
                                                }).collect_view()}
                                            </div>
                                        }.into_any(),
                                        Ok(None) => ().into_any(),
                                        Err(error) => view! { <div class="act-err">{error}</div> }.into_any(),
                                    })}
                                </Suspense>
                                </div></details>
                            })}
                            </div>
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
    pending: RwSignal<Option<String>>,
    refresh: Callback<()>,
) {
    leptos::task::spawn_local(async move {
        status.set(None);
        pending.set(Some(format!("{action}:{service}")));
        let result = data::post_action(&serde_json::json!({
            "action": format!("talos-service-{action}"),
            "name": node,
            "service": service,
        }))
        .await
        .map(|_| format!("service {action} requested"));
        if result.is_ok() {
            refresh.run(());
        }
        status.set(Some(result));
        pending.set(None);
    });
}

/// Drives a plain (non-drain-first) Talos reboot/shutdown. The drain-first
/// path no longer goes through here — it opens the drain dialog
/// (`overlays::drain::DrainOverlay`) instead, which POSTs the same
/// `talos-{action}` action itself with `drain: true`.
fn talos_power_action(
    node: String,
    action: &'static str,
    status: RwSignal<Option<Result<String, String>>>,
    pending: RwSignal<Option<String>>,
) {
    leptos::task::spawn_local(async move {
        status.set(None);
        pending.set(Some(action.into()));
        let result = data::post_action(&serde_json::json!({
            "action": format!("talos-{action}"),
            "name": node,
        }))
        .await;
        status.set(Some(result.map(|_| {
            if action == "reboot" {
                "node returned Ready".into()
            } else {
                format!("{action} requested")
            }
        })));
        pending.set(None);
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

fn short_fingerprint(fingerprint: &str) -> String {
    fingerprint.chars().take(12).collect()
}

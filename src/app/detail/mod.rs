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
use crate::app::state::DetailTarget;
use crate::app::util::format::parse_key;
use crate::app::util::json::selector_from;
use crate::app::util::predicate::KindKind;
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
                {move || detail.get().map(|t| view! { <RowDetail target=t /> })}
            </div>
        </div>
    }
}

/// Inline detail for an expanded row: actions, a describe-style Info view (default),
/// YAML, and pod logs — selectable via tabs.
#[component]
pub(crate) fn RowDetail(target: DetailTarget) -> impl IntoView {
    let detail = expect_context::<RwSignal<Option<DetailTarget>>>();
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

    let (group, kind) = parse_key(&target.key);
    let kk = KindKind::new(&group, &kind);
    let is_workload = kk.is_workload();
    let is_scalable = kk.is_scalable();
    let is_flux = kk.is_flux();
    let is_eso = kk.is_eso();
    let is_pod = kk.is_pod();
    // Pod-owning resources get a live "Pods" tab listing their pods by selector.
    let has_pods = is_workload || kk.is_job();
    let is_cronjob = kk.is_cronjob();
    let ns = target.namespace.clone().unwrap_or_default();
    let pod = target.name.clone();

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
                Ok(()) => {
                    status.set(Some(Ok(format!("{action} ✓"))));
                    if action == "delete" {
                        detail.set(None);
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
                        <button class="act" on:click=move |_| run("flux-suspend", serde_json::json!({}))>"Suspend"</button>
                        <button class="act" on:click=move |_| run("flux-resume", serde_json::json!({}))>"Resume"</button>
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
                        Tab::Yaml => view! {
                            <div class="rd-body">
                                <div class="yaml-head">
                                    <h4>"YAML"</h4>
                                    {move || can_patch().then(|| view! {
                                        <button class="act" on:click=move |_| run("apply", serde_json::json!({ "yaml": yaml.get() }))>"Apply"</button>
                                    })}
                                </div>
                                <textarea class="yaml-edit" spellcheck="false"
                                    prop:value=move || yaml.get()
                                    on:input=move |e| yaml.set(event_target_value(&e))></textarea>
                            </div>
                        }.into_any(),
                    };
                    view! { {pods_section} {tab_content} }
                })}
            </Suspense>
        </div>
    }
}

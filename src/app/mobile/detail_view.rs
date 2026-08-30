use leptos::prelude::*;
use roder_core::ObjectDetail;

use crate::app::controllers::detail::{
    certificate_summary, format_bytes, short_fingerprint, talos_action, talos_config_diff,
    talos_node, use_metrics, DetailTab, ResourceDetailController,
};
use crate::app::jobs::CronJobJobs;
use crate::app::log_stream::{extract_timestamp, use_log_stream};
use crate::app::state::{
    DetailTarget, DrainOpen, DrainTarget, ExecOpen, ExecTarget, TalosFeatures,
};
use crate::app::ui::{ask_confirm, ask_delete, delete_extra, Confirm, DeleteRequest};
use crate::app::util::format::{ansi_to_html, camel_label, log_level, parse_key, parse_log_line};
use crate::app::util::json::{
    conditions, container_envs, container_images, data_entries, json_map, json_str, owner_refs,
    rbac_rules, section_scalars, selector_from, status_scalars,
};
use crate::app::util::predicate::KindKind;
use crate::app::util::yaml_hl;
use crate::data;

use super::pods::MobilePodsTab;

#[component]
pub(crate) fn MobileDetailView() -> impl IntoView {
    let detail = expect_context::<RwSignal<Option<DetailTarget>>>();
    let (snapshot, closing, close) = crate::app::ui::use_option_overlay(detail);
    view! {
        <section class="mobile-detail" class:open=move || snapshot.get().is_some()
            class:closing=move || closing.get() aria-label="Resource detail">
            <header class="mobile-detail-head">
                <button class="mobile-detail-back" aria-label="Back" on:click=move |_| close()>
                    <svg viewBox="0 0 24 24" aria-hidden="true"><path d="m15 18-6-6 6-6" /></svg>
                </button>
                <div class="mobile-detail-heading"><span>"Resource"</span>
                    <strong class="mobile-detail-title">{move || snapshot.get().map(|target| target.name).unwrap_or_default()}</strong>
                </div>
            </header>
            <div class="mobile-detail-body">
                {move || snapshot.get().map(|target| view! { <MobileRowDetail target on_delete=move || close() /> })}
            </div>
        </section>
    }
}

#[component]
pub(crate) fn MobileRowDetail(
    target: DetailTarget,
    on_delete: impl Fn() + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let requested_tab = expect_context::<RwSignal<Option<DetailTab>>>();
    let initial_tab = requested_tab.get_untracked().unwrap_or(DetailTab::Info);
    requested_tab.set(None);
    let tab = RwSignal::new(initial_tab);
    let controller = ResourceDetailController::new(target.clone());
    let permissions = controller.permissions;
    let yaml = RwSignal::new(String::new());
    let yaml_editing = RwSignal::new(false);
    Effect::new(move |_| {
        if let Some(Some(detail)) = controller.object.get() {
            yaml.set(detail.yaml);
        }
    });

    let (group, kind) = parse_key(&target.key);
    let kind_kind = KindKind::new(&group, &kind);
    let is_workload = kind_kind.is_workload();
    let is_scalable = kind_kind.is_scalable();
    let is_flux = kind_kind.is_flux();
    let is_helmrelease = kind_kind.is_helmrelease();
    let has_source_ref = kind_kind.has_source_ref();
    let is_eso = kind_kind.is_eso();
    let is_certificate = kind_kind.is_certificate();
    let is_pod = kind_kind.is_pod();
    let is_node = kind_kind.is_node();
    let is_job = kind_kind.is_job();
    let is_cronjob = kind_kind.is_cronjob();
    let is_snapshot_policy = kind_kind.is_kopiur_snapshot_policy();
    let has_pods = is_workload || is_job;
    let features = expect_context::<TalosFeatures>().0;
    let talos_available = move || is_node && features.get().read;
    let target_value = StoredValue::new(target.clone());
    let kind_value = StoredValue::new(kind);
    let confirm = expect_context::<RwSignal<Option<Confirm>>>();
    let delete_request = expect_context::<RwSignal<Option<DeleteRequest>>>();
    let exec = expect_context::<ExecOpen>().0;

    let run = move |action: &'static str, extra: serde_json::Value| {
        controller.run(target_value.get_value(), action, extra, on_delete)
    };
    let suspended = move || {
        controller
            .object
            .get()
            .flatten()
            .and_then(|detail| detail.object.pointer("/spec/suspend")?.as_bool())
            .unwrap_or(false)
    };
    let job_terminal = move || {
        controller.object.get().flatten().is_some_and(|detail| {
            detail
                .object
                .pointer("/status/conditions")
                .and_then(serde_json::Value::as_array)
                .is_some_and(|conditions| {
                    conditions.iter().any(|condition| {
                        matches!(
                            condition.get("type").and_then(serde_json::Value::as_str),
                            Some("Complete" | "Failed")
                        ) && condition.get("status").and_then(serde_json::Value::as_str)
                            == Some("True")
                    })
                })
        })
    };

    view! {
        <div class="rd mobile-rd">
            <div class="actions mobile-detail-actions">
                {is_workload.then(|| view! { <Show when=move || permissions.get().is_some_and(|p| p.patch)>
                    <button class="act" on:click=move |_| run("restart", serde_json::json!({}))>"Restart"</button>
                    {is_scalable.then(|| view! { <MobileScale controller target=target_value /> })}
                </Show> })}
                {is_flux.then(|| view! { <Show when=move || permissions.get().is_some_and(|p| p.patch)>
                    <button class="act" on:click=move |_| run("flux-reconcile", serde_json::json!({}))>"Reconcile"</button>
                    {has_source_ref.then(|| view! { <button class="act" on:click=move |_| run("flux-reconcile-with-source", serde_json::json!({}))>"Reconcile w/ source"</button> })}
                    {is_helmrelease.then(|| view! {
                        <button class="act" on:click=move |_| run("flux-force", serde_json::json!({}))>"Force"</button>
                        <button class="act" on:click=move |_| run("flux-reset", serde_json::json!({}))>"Reset"</button>
                    })}
                    <button class="act" on:click=move |_| if suspended() { run("flux-resume", serde_json::json!({})) } else { run("flux-suspend", serde_json::json!({})) }>
                        {move || if suspended() { "Resume" } else { "Suspend" }}
                    </button>
                </Show> })}
                {is_eso.then(|| view! { <Show when=move || permissions.get().is_some_and(|p| p.patch)><button class="act" on:click=move |_| run("eso-refresh", serde_json::json!({}))>"Refresh"</button></Show> })}
                {is_certificate.then(|| view! { <Show when=move || permissions.get().is_some_and(|p| p.update_status)><button class="act" on:click=move |_| ask_confirm(confirm, "Force renewal of this Certificate?", "Renew", move || run("certificate-renew", serde_json::json!({})))>"Force renew"</button></Show> })}
                {is_cronjob.then(|| view! { <Show when=move || permissions.get().is_some_and(|p| p.patch)><button class="act" on:click=move |_| run("cronjob-trigger", serde_json::json!({}))>"Trigger"</button></Show> })}
                {is_job.then(|| view! { <Show when=move || permissions.get().is_some_and(|p| p.create) && job_terminal()><button class="act" on:click=move |_| run("job-rerun", serde_json::json!({}))>"Re-run"</button></Show> })}
                {is_snapshot_policy.then(|| view! { <Show when=move || permissions.get().is_some_and(|p| p.patch)><button class="act" on:click=move |_| run("kopiur-snapshot-now", serde_json::json!({}))>"Snapshot Now"</button></Show> })}
                {is_pod.then(|| view! { <button class="act" on:click=move |_| {
                    let target = target_value.get_value();
                    exec.set(Some(ExecTarget { namespace: target.namespace.unwrap_or_default(), pod: target.name, container: None, pending: false, node_shell: false, image: String::new() }));
                }>"Shell"</button> })}
                <Show when=move || permissions.get().is_some_and(|p| p.delete)><button class="act danger" on:click=move |_| ask_delete(delete_request, "Delete this resource?", move |force, propagation| run("delete", delete_extra(force, propagation)))>"Delete"</button></Show>
                {move || controller.status.get().map(|result| match result { Ok(message) => view! { <span class="act-ok">{message}</span> }.into_any(), Err(error) => view! { <span class="act-err">{error}</span> }.into_any() })}
            </div>
            <nav class="rd-tabs mobile-detail-tabs" aria-label="Detail sections">
                <MobileTab tab current=DetailTab::Info label="Info" />
                <MobileTab tab current=DetailTab::Yaml label="YAML" />
                {is_pod.then(|| view! { <MobileTab tab current=DetailTab::Metrics label="Metrics" /> })}
                {move || (is_pod || talos_available()).then(|| view! { <MobileTab tab current=DetailTab::Logs label="Logs" /> })}
                {move || talos_available().then(|| view! { <MobileTab tab current=DetailTab::Talos label="Talos" /> })}
                {is_cronjob.then(|| view! { <MobileTab tab current=DetailTab::Jobs label="Jobs" /> })}
            </nav>
            <Suspense fallback=|| view! { <div class="pad muted">"Loading..."</div> }>
                {move || controller.object.get().flatten().map(|detail| {
                    let pods = has_pods.then(|| view! { <section class="rd-pods"><h4>"Pods"</h4><MobilePodsTab namespace=detail.namespace.clone().unwrap_or_default() selector=selector_from(&detail.object) /></section> });
                    let target = target_value.get_value();
                    let content = match tab.get() {
                        DetailTab::Info => view! { <MobileInfo detail kind=kind_value.get_value() /> }.into_any(),
                        DetailTab::Yaml => view! { <MobileYaml yaml editing=yaml_editing can_patch=permissions.get().is_some_and(|p| p.patch) run /> }.into_any(),
                        DetailTab::Logs => {
                            let url = if is_pod { format!("/api/logs?namespace={}&pod={}", data::percent_encode(target.namespace.as_deref().unwrap_or_default()), data::percent_encode(&target.name)) } else { format!("/api/talos/dmesg?node={}", data::percent_encode(&target.name)) };
                            view! { <MobileInlineLogs url /> }.into_any()
                        }
                        DetailTab::Metrics => view! { <MobileMetrics namespace=target.namespace.unwrap_or_default() name=target.name /> }.into_any(),
                        DetailTab::Talos => view! { <MobileTalos node=target.name key=target.key actions=features.get().actions config=features.get().config /> }.into_any(),
                        DetailTab::Jobs => view! { <CronJobJobs target=target_value.get_value() /> }.into_any(),
                    };
                    view! { {pods} {content} }
                })}
            </Suspense>
        </div>
    }
}

#[component]
fn MobileTab(tab: RwSignal<DetailTab>, current: DetailTab, label: &'static str) -> impl IntoView {
    view! { <button class="rd-tab" class:active=move || tab.get() == current on:click=move |_| tab.set(current)>{label}</button> }
}

#[component]
fn MobileScale(
    controller: ResourceDetailController,
    target: StoredValue<DetailTarget>,
) -> impl IntoView {
    let replicas = RwSignal::new(1i32);
    Effect::new(move |_| {
        if let Some(value) = controller
            .object
            .get()
            .flatten()
            .and_then(|detail| detail.object.pointer("/spec/replicas")?.as_i64())
        {
            replicas.set(value as i32);
        }
    });
    view! { <label class="mobile-detail-scale"><span>"Scale"</span><input type="number" min="0" prop:value=move || replicas.get() on:input=move |event| if let Ok(value) = event_target_value(&event).parse() { replicas.set(value) } />
    <button class="act" on:click=move |_| controller.run(target.get_value(), "scale", serde_json::json!({"replicas": replicas.get_untracked()}), || {})>"Apply"</button></label> }
}

#[component]
fn MobileYaml(
    yaml: RwSignal<String>,
    editing: RwSignal<bool>,
    can_patch: bool,
    run: impl Fn(&'static str, serde_json::Value) + Copy + Send + Sync + 'static,
) -> impl IntoView {
    view! { <section class="yaml-pane"><header class="yaml-head"><h4>"YAML"</h4>
        {can_patch.then(|| view! { {move || editing.get().then(|| view! { <button class="act" on:click=move |_| run("apply", serde_json::json!({"yaml": yaml.get_untracked()}))>"Apply"</button> })}<button class="act" on:click=move |_| editing.update(|value| *value = !*value)>{move || if editing.get() { "View" } else { "Edit" }}</button> })}
    </header>{move || if editing.get() { view! { <textarea class="yaml-edit" spellcheck="false" prop:value=move || yaml.get() on:input=move |event| yaml.set(event_target_value(&event))></textarea> }.into_any() } else { view! { <pre class="yaml-view" inner_html=yaml_hl::highlight_yaml(&yaml.get())></pre> }.into_any() }}</section> }
}

#[component]
fn MobileInlineLogs(url: String) -> impl IntoView {
    let stream = use_log_stream(url);
    let logs_ref = NodeRef::<leptos::html::Div>::new();
    #[cfg(target_arch = "wasm32")]
    Effect::new(move |_| {
        stream.filtered_lines.track();
        if stream.follow.get() {
            if let Some(element) = logs_ref.get_untracked() {
                request_animation_frame(move || element.set_scroll_top(element.scroll_height()));
            }
        }
    });
    view! { <section class="mobile-inline-logs"><div class="mobile-log-controls"><input class="mobile-log-filter" type="search" placeholder="Filter log lines" prop:value=move || stream.filter.get() on:input=move |event| stream.filter.set(event_target_value(&event)) />
    <div class="mobile-log-levels">{[("", "All"), ("error", "ERR"), ("warn", "WRN"), ("info", "INF"), ("debug", "DBG")].into_iter().map(|(level, label)| view! { <button class:active=move || stream.level_filter.get() == level on:click=move |_| stream.level_filter.set(level.into())>{label}</button> }).collect_view()}</div>
    <div class="mobile-log-toggles"><button class:on=move || stream.show_timestamps.get() on:click=move |_| stream.show_timestamps.update(|value| *value = !*value)>"Time"</button><button class:on=move || stream.follow.get() on:click=move |_| stream.follow.update(|value| *value = !*value)>"Follow"</button><button class:on=move || stream.wrap.get() on:click=move |_| stream.wrap.update(|value| *value = !*value)>"Wrap"</button></div></div>
    <div class="mobile-log-lines" class:nowrap=move || !stream.wrap.get() node_ref=logs_ref><For each=move || stream.filtered_lines.get() key=|(id, _)| *id let:item>{
        let parsed = parse_log_line(&item.1); let level = log_level(&item.1); let (timestamp, display) = if parsed.is_structured { (parsed.timestamp, parsed.display) } else { extract_timestamp(&parsed.display) }; let html = ansi_to_html(&display);
        view! { <div class="mobile-log-line">{move || stream.show_timestamps.get().then(|| timestamp.clone().map(|value| view! { <span class="mobile-log-time">{value}</span> }))}<span class=format!("mobile-log-level {level}")>{level.to_uppercase()}</span><span class="mobile-log-message" inner_html=html></span></div> }
    }</For></div></section> }
}

#[component]
fn MobileMetrics(namespace: String, name: String) -> impl IntoView {
    let points = use_metrics(namespace, name);
    view! { <section class="mobile-metrics">{move || points.get().map(|points| if points.is_empty() { view! { <div class="pad muted">"No metrics data yet. Waiting for metrics-server..."</div> }.into_any() } else {
        let max_cpu = points.iter().map(|point| point.cpu).fold(0.001, f64::max); let max_mem = points.iter().map(|point| point.mem).fold(1.0, f64::max); let latest = points.last().cloned().unwrap();
        view! { <div class="detail-stats"><div class="detail-stat"><span class="detail-stat-label">"CPU"</span><b class="detail-stat-value">{format!("{:.0}m", latest.cpu * 1000.0)}</b></div><div class="detail-stat"><span class="detail-stat-label">"Memory"</span><b class="detail-stat-value">{format_bytes(latest.mem as u64)}</b></div></div>
            <div class="mobile-metric-history">{points.into_iter().map(|point| view! { <div class="mobile-metric-sample"><i style:height=format!("{}%", point.cpu / max_cpu * 100.0)></i><i class="memory" style:height=format!("{}%", point.mem / max_mem * 100.0)></i></div> }).collect_view()}</div> }.into_any()
    })}</section> }
}

#[component]
fn MobileInfo(detail: ObjectDetail, kind: String) -> impl IntoView {
    let object = &detail.object;
    let is_event = kind == "Event";
    let certificate = (kind == "Certificate").then(|| certificate_summary(object));
    let created = json_str(object, &["metadata", "creationTimestamp"]);
    let owners = owner_refs(object);
    let mut status = status_scalars(object);
    if certificate.is_some() {
        status.retain(|(key, _)| {
            !matches!(
                key.as_str(),
                "notBefore" | "notAfter" | "renewalTime" | "revision"
            )
        });
    }
    let spec = section_scalars(object, "spec");
    let labels = json_map(object, &["metadata", "labels"]);
    let annotations = json_map(object, &["metadata", "annotations"]);
    let conds = conditions(object);
    let images = container_images(object);
    let envs = container_envs(object);
    let rules = if matches!(kind.as_str(), "Role" | "ClusterRole") {
        rbac_rules(object)
    } else {
        Vec::new()
    };
    let secret = kind == "Secret";
    let entries = if secret || kind == "ConfigMap" {
        data_entries(object, secret)
    } else {
        Vec::new()
    };
    let event_type = json_str(object, &["type"]);
    let event_reason = json_str(object, &["reason"]);
    let event_message = json_str(object, &["message"]);
    let event_action = json_str(object, &["action"]);
    let event_source = json_str(object, &["reportingComponent"])
        .or_else(|| json_str(object, &["source", "component"]));
    let event_count =
        json_str(object, &["series", "count"]).or_else(|| json_str(object, &["count"]));
    let event_first_seen = json_str(object, &["firstTimestamp"]);
    let event_last_seen = json_str(object, &["series", "lastObservedTime"])
        .or_else(|| json_str(object, &["eventTime"]))
        .or_else(|| json_str(object, &["lastTimestamp"]));
    let event_object = match (
        json_str(object, &["involvedObject", "kind"]),
        json_str(object, &["involvedObject", "name"]),
    ) {
        (Some(kind), Some(name)) => Some(format!("{kind} / {name}")),
        (None, Some(name)) => Some(name),
        _ => None,
    };
    view! { <section class="info mobile-info">
        {is_event.then(|| view! {
            <section class="event-detail-summary">
                <div class="event-detail-head"><div><span class="event-detail-label">"Kubernetes event"</span><h3>{event_reason.unwrap_or_else(|| "Unknown reason".into())}</h3></div>
                    {event_type.map(|value| {
                        let class = format!("event-type event-type-{}", value.to_lowercase());
                        view! { <span class=class>{value}</span> }
                    })}
                </div>
                {event_message.map(|value| view! { <div class="event-detail-message">{value}</div> })}
                <div class="event-detail-context">
                    {event_object.map(|value| view! { <div class="event-detail-row"><span>"Affected object"</span><strong>{value}</strong></div> })}
                    {event_source.map(|value| view! { <div class="event-detail-row"><span>"Reported by"</span><strong>{value}</strong></div> })}
                    {event_action.map(|value| view! { <div class="event-detail-row"><span>"Action"</span><strong>{value}</strong></div> })}
                </div>
                <div class="event-detail-stats">
                    {event_count.map(|value| view! { <div><span>"Occurrences"</span><strong>{value}</strong></div> })}
                    {event_first_seen.map(|value| { let age = data::humanize_age(&Some(value.clone())); view! { <div><span>"First seen"</span><strong title=value>{age}</strong></div> } })}
                    {event_last_seen.map(|value| { let age = data::humanize_age(&Some(value.clone())); view! { <div><span>"Last seen"</span><strong title=value>{age}</strong></div> } })}
                </div>
            </section>
        })}
        {certificate.map(|certificate| view! {
            <section class="certificate-detail-summary">
                <div class="certificate-detail-heading"><span>"Certificate lifecycle"</span><strong class=certificate.state_class>{certificate.state}</strong></div>
                <div class="detail-stats">
                    <div class="detail-stat"><span class="detail-stat-label">"Valid from"</span><span class="detail-stat-value" title=certificate.not_before_raw>{certificate.not_before}</span></div>
                    <div class="detail-stat"><span class="detail-stat-label">"Expires"</span><span class="detail-stat-value" title=certificate.not_after_raw>{certificate.not_after}</span></div>
                    <div class="detail-stat"><span class="detail-stat-label">"Scheduled renewal"</span><span class="detail-stat-value" title=certificate.renewal_time_raw>{certificate.renewal_time}</span></div>
                    <div class="detail-stat"><span class="detail-stat-label">"Revision"</span><span class="detail-stat-value">{certificate.revision}</span></div>
                    <div class="detail-stat"><span class="detail-stat-label">"Target Secret"</span><span class="detail-stat-value">{certificate.secret}</span></div>
                </div>
            </section>
        })}
        <div class="kv-grid">
            {detail.namespace.map(|value| view! { <div class="kv"><span class="k">"Namespace"</span><span class="v">{value}</span></div> })}
            {created.map(|value| view! { <div class="kv"><span class="k">"Age"</span><span class="v">{data::humanize_age(&Some(value))}</span></div> })}
            {owners.into_iter().map(|(kind, name)| view! { <div class="kv"><span class="k">"Controlled By"</span><span class="v">{format!("{kind}/{name}")}</span></div> }).collect_view()}
            {status.into_iter().map(|(key, value)| view! { <div class="kv"><span class="k">{camel_label(&key)}</span><span class="v">{value}</span></div> }).collect_view()}
        </div>
        {(!spec.is_empty()).then(|| view! { <h4>"Spec"</h4><div class="kv-cols">{spec.into_iter().map(|(key, value)| view! { <div class="kvc"><span class="kvc-k">{camel_label(&key)}</span><span class="kvc-v">{value}</span></div> }).collect_view()}</div> })}
        {(!images.is_empty()).then(|| view! { <h4>"Containers"</h4><div class="kv-cols container-images">{images.into_iter().map(|(name, image)| view! { <div class="kvc"><span class="kvc-k">{name}</span><span class="kvc-v">{image}</span></div> }).collect_view()}</div> })}
        {(!envs.is_empty()).then(|| { let multiple = envs.len() > 1; view! { <h4>"Env"</h4>{envs.into_iter().map(|container| view! { {multiple.then(|| view! { <div class="env-container-name">{container.container}</div> })}<div class="kvlist">{container.entries.into_iter().map(|(key, value)| view! { <div class="kvl"><span class="kvl-k">{format!("{key}:")}</span><span class="kvl-v">{value}</span></div> }).collect_view()}</div> }).collect_view()} } })}
        {(!rules.is_empty()).then(|| view! { <h4>"Rules"</h4><div class="mobile-rules">{rules.into_iter().map(|rule| view! { <article><b>{rule.resources}</b><span>{rule.verbs}</span><small>{format!("{} {}", rule.groups, rule.names)}</small></article> }).collect_view()}</div> })}
        {(!entries.is_empty()).then(|| view! { <h4>"Data"</h4>{secret.then(|| view! { <div class="hint">"Values are hidden - tap to reveal."</div> })}<div class="data">{entries.into_iter().map(|(key, value, hidden)| { let revealed = RwSignal::new(false); view! { <div class="data-row"><div class="data-key">{key}</div>{if hidden { view! { <pre class="data-val secret" class:revealed=move || revealed.get() on:click=move |_| revealed.set(true)>{value}</pre> }.into_any() } else { view! { <pre class="data-val">{value}</pre> }.into_any() }}</div> } }).collect_view()}</div> })}
        {(!conds.is_empty()).then(|| view! { <h4>"Conditions"</h4><div class="mobile-conditions">{conds.into_iter().map(|condition| { let class = match condition.status.as_str() { "True" => "cond-ok", "False" => "cond-error", _ => "cond-pending" }; view! { <article><div><b>{condition.type_}</b><span class=class>{condition.status}</span></div><strong>{condition.reason}</strong><p>{condition.message}</p></article> } }).collect_view()}</div> })}
        {[("Labels", labels), ("Annotations", annotations)].into_iter().filter_map(|(title, values)| (!values.is_empty()).then(|| view! { <h4>{title}</h4><div class="kvlist">{values.into_iter().map(|(key, value)| view! { <div class="kvl"><span class="kvl-k">{format!("{key}:")}</span><span class="kvl-v">{value}</span></div> }).collect_view()}</div> })).collect_view()}
        {(!detail.events.is_empty()).then(|| view! { <h4>"Events"</h4><div class="events">{detail.events.into_iter().take(12).map(|event| view! { <div class=format!("event ev-{}", event.type_.to_lowercase())><span class="ev-reason">{event.reason}</span><span class="ev-msg">{event.message}</span></div> }).collect_view()}</div> })}
    </section> }
}

#[component]
fn MobileTalos(node: String, key: String, actions: bool, config: bool) -> impl IntoView {
    let resource = talos_node(node.clone());
    let load_config = RwSignal::new(false);
    let config_diff = talos_config_diff(node.clone(), load_config);
    let pending = RwSignal::new(None::<String>);
    let action_status = RwSignal::new(None::<Result<String, String>>);
    let drain_first = RwSignal::new(true);
    let drain = expect_context::<DrainOpen>().0;
    let confirm = expect_context::<RwSignal<Option<Confirm>>>();
    let node_value = StoredValue::new(node);
    let key_value = StoredValue::new(key);
    let refresh = Callback::new(move |_| resource.refetch());
    view! { <Suspense fallback=|| view! { <div class="pad muted">"Loading Talos status..."</div> }>{move || resource.get().map(|result| match result { Err(error) => view! { <div class="pad muted">{format!("Talos integration unavailable: {error}")}</div> }.into_any(), Ok(status) => {
        let control_plane = status.control_plane; let services = status.services; let disks = status.disk_inventory; let volumes = status.volumes; let interfaces = status.interfaces; let disk_stats = status.disks; let errors = status.errors; let fingerprint = status.config_fingerprint.clone();
        let service_running = services.iter().filter(|service| service.state.to_lowercase().contains("running")).count();
        let service_issues = services.iter().filter(|service| !service.state.to_lowercase().contains("running") || (!service.health_unknown && !service.healthy)).count();
        let network_up = interfaces.iter().filter(|interface| interface.link_up == Some(true)).count();
        view! { <section class="info talos-view mobile-talos"><div class="detail-stats"><div class="detail-stat"><span class="detail-stat-label">"Talos"</span><b class="detail-stat-value">{status.version}</b><span class="detail-stat-note">{if control_plane { "control plane" } else { "worker" }}</span></div><div class="detail-stat"><span class="detail-stat-label">"Services"</span><b class="detail-stat-value">{format!("{service_running}/{} running", services.len())}</b><span class="detail-stat-note">{if service_issues == 0 { "all healthy".into() } else { format!("{service_issues} need attention") }}</span></div><div class="detail-stat"><span class="detail-stat-label">"Storage"</span><b class="detail-stat-value">{format!("{} drives", disks.len())}</b><span class="detail-stat-note">{format!("{} partitions", volumes.len())}</span></div><div class="detail-stat"><span class="detail-stat-label">"Network"</span><b class="detail-stat-value">{format!("{network_up}/{} links up", interfaces.len())}</b></div></div>
             {fingerprint.clone().map(|value| view! { <div class="talos-fingerprint"><span>"Config"</span><code>{short_fingerprint(&value)}</code></div> })}
             {(!errors.is_empty()).then(|| view! { <div class="talos-errors">{errors.into_iter().map(|(section, error)| view! { <div class="warn-line"><b>{section}</b>": "{error}</div> }).collect_view()}</div> })}
            {actions.then(|| view! { <div class="actions talos-power-actions"><label class="talos-drain"><input type="checkbox" prop:checked=move || drain_first.get() on:change=move |event| drain_first.set(event_target_checked(&event)) />"Drain first"</label>{[("reboot", "Reboot"), ("shutdown", "Shutdown")].into_iter().map(|(action, label)| view! { <button class="act danger" disabled=move || pending.get().is_some() on:click=move |_| {
                let node = node_value.get_value(); if drain_first.get_untracked() { drain.set(Some(DrainTarget { key: key_value.get_value(), name: node, power: Some(action.into()), control_plane, job: None })); } else { ask_confirm(confirm, format!("{label} {node}?"), label, move || talos_action(node.clone(), format!("talos-{action}"), None, action_status, pending, None)); }
            }>{label}</button> }).collect_view()}</div> })}
            {move || action_status.get().map(|result| match result { Ok(value) => view! { <div class="act-ok">{value}</div> }.into_any(), Err(error) => view! { <div class="act-err">{error}</div> }.into_any() })}
            <div class="talos-sections"><details class="talos-section" open><summary>"Services"</summary><div class="talos-section-body">{services.into_iter().map(|service| { let start_id = service.id.clone(); let restart_id = service.id.clone(); let stop_id = service.id.clone(); let running = service.state.to_lowercase().contains("running"); let health = if service.health_unknown { "Health unknown" } else if service.healthy { "Healthy" } else { "Unhealthy" }; view! { <article class="mobile-talos-row"><div><b>{service.id}</b><span>{service.state}</span><small>{health}" · "{service.message}</small></div>{actions.then(|| view! { <div class="actions">{(!running).then(|| view! { <button class="act" on:click=move |_| talos_action(node_value.get_value(), "talos-service-start".into(), Some(start_id.clone()), action_status, pending, Some(refresh))>"Start"</button> })}{running.then(|| view! { <button class="act" on:click=move |_| { let node = node_value.get_value(); let service = restart_id.clone(); ask_confirm(confirm, format!("Restart Talos service {service} on {node}?"), "Restart", move || talos_action(node.clone(), "talos-service-restart".into(), Some(service.clone()), action_status, pending, Some(refresh))); }>"Restart"</button><button class="act danger" on:click=move |_| { let node = node_value.get_value(); let service = stop_id.clone(); ask_confirm(confirm, format!("Stop Talos service {service} on {node}?"), "Stop", move || talos_action(node.clone(), "talos-service-stop".into(), Some(service.clone()), action_status, pending, Some(refresh))); }>"Stop"</button> })}</div> })}</article> } }).collect_view()}</div></details>
                <details class="talos-section"><summary>"Storage"</summary><div class="talos-section-body">{disks.into_iter().map(|disk| { let io = disk_stats.iter().find(|stat| stat.name == disk.name || disk.path.ends_with(&format!("/{}", stat.name))); let kind = if disk.rotational { "HDD" } else { "SSD" }; let access = if disk.readonly { " · read-only" } else { "" }; let io_summary = io.map(|stat| format!("read {} · written {}", format_bytes(stat.read_bytes), format_bytes(stat.write_bytes))).unwrap_or_else(|| "I/O unavailable".into()); view! { <article class="mobile-talos-row"><b>{disk.path}</b><span>{disk.model.unwrap_or_else(|| "Unknown model".into())}</span><small>{format!("{} · {kind}{access}", format_bytes(disk.size))}</small><small>{io_summary}</small><small>{disk.transport.or(disk.serial).or(disk.wwid).unwrap_or_default()}</small></article> } }).collect_view()}{volumes.into_iter().map(|volume| { let usage = volume.used_bytes.zip(volume.available_bytes).map(|(used, available)| { let total = used + available; if total == 0 { "0% used".into() } else { format!("{:.1}% used", 100.0 * used as f64 / total as f64) } }).unwrap_or_else(|| "not mounted".into()); view! { <article class="mobile-talos-row"><b>{volume.name}</b><span>{volume.path}</span><small>{format!("{} · {} · {usage}", volume.filesystem.unwrap_or_else(|| "unknown filesystem".into()), format_bytes(volume.size))}</small><small>{volume.phase}{volume.encryption.map(|provider| format!(" · encrypted ({provider})")).unwrap_or_default()}</small></article> } }).collect_view()}</div></details>
                <details class="talos-section"><summary>"Network"</summary><div class="talos-section-body">{interfaces.into_iter().map(|interface| { let state = interface.operational_state.unwrap_or_else(|| "unknown".into()); let speed = interface.speed_mbps.map(|speed| format!(" · {speed} Mbps {}", interface.duplex.unwrap_or_default())).unwrap_or_default(); let hardware = interface.hardware_address.unwrap_or_else(|| "unknown hardware address".into()); let mtu = interface.mtu.map(|value| format!(" · MTU {value}")).unwrap_or_default(); view! { <article class="mobile-talos-row"><b>{interface.name}</b><span>{state}{speed}</span><small>{if interface.addresses.is_empty() { "No addresses".into() } else { interface.addresses.join(" / ") }}</small><small>{hardware}{mtu}</small><small>{format!("RX {} · TX {} · {} errors · {} drops", format_bytes(interface.rx_bytes), format_bytes(interface.tx_bytes), interface.rx_errors + interface.tx_errors, interface.rx_dropped + interface.tx_dropped)}</small></article> } }).collect_view()}</div></details>
                {config.then(|| view! { <details class="talos-section"><summary>"Configuration"</summary><div class="talos-section-body"><button class="act" on:click=move |_| load_config.set(true)>"Compare node configurations"</button><Suspense fallback=|| view! { <div class="muted">"Comparing redacted configurations..."</div> }>{move || config_diff.get().map(|result| match result { Ok(Some(diff)) => view! { <div class="talos-config-diff">{diff.peers.into_iter().map(|peer| { let summary = match peer.matches { Some(true) => "matches".into(), Some(false) => format!("{} differences", peer.differences.len()), None => "unavailable".into() }; view! { <details><summary><b>{peer.node}</b>" · "{summary}</summary>{peer.error.map(|error| view! { <div class="act-err">{error}</div> })}{peer.differences.into_iter().map(|difference| view! { <div class="mobile-talos-diff"><code>{difference.path}</code><span>{difference.node_value.unwrap_or_else(|| "<missing>".into())}</span><span>{difference.peer_value.unwrap_or_else(|| "<missing>".into())}</span></div> }).collect_view()}</details> } }).collect_view()}</div> }.into_any(), Ok(None) => ().into_any(), Err(error) => view! { <div class="act-err">{error}</div> }.into_any() })}</Suspense></div></details> })}
            </div></section> }.into_any()
    } })}</Suspense> }
}

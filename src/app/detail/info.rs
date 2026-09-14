//! Describe view built from the complete object JSON plus curated operational
//! summaries for conditions, containers, data, and events.

use leptos::prelude::*;
use roder_core::ObjectDetail;

use crate::app::controllers::detail::certificate_summary;
use crate::app::util::format::{camel_label, condition_class, counted};
use crate::app::util::json::{
    conditions, container_envs, container_images, data_entries, json_map, json_str, owner_refs,
    rbac_rules, section_fields, section_fields_except, status_scalars, top_level_fields_except,
};
use crate::app::util::json_fields::{json_fields, json_fields_except};
use crate::data;

pub(crate) fn info_view(d: ObjectDetail, kind: String) -> impl IntoView {
    let o = &d.object;
    let is_event = kind == "Event";
    let certificate = (kind == "Certificate").then(|| certificate_summary(o));
    let created = json_str(o, &["metadata", "creationTimestamp"]);
    let labels = json_map(o, &["metadata", "labels"]);
    let annotations = json_map(o, &["metadata", "annotations"]);
    let owners = owner_refs(o);
    let conds = conditions(o);
    let mut stats = status_scalars(o);
    if certificate.is_some() {
        stats.retain(|(key, _)| {
            !matches!(
                key.as_str(),
                "notBefore" | "notAfter" | "renewalTime" | "revision"
            )
        });
    }
    let spec_count = section_fields(o, "spec").len();
    let status_detail_count = section_fields(o, "status").len();
    let spec_value = o.get("spec").cloned().unwrap_or_default();
    let status_value = o.get("status").cloned().unwrap_or_default();
    let metadata_value = o.get("metadata").cloned().unwrap_or_default();
    let additional_value = o.clone();
    let metadata_count = section_fields(o, "metadata").len();
    let metadata_detail_count = section_fields_except(
        o,
        "metadata",
        &[
            "name",
            "namespace",
            "creationTimestamp",
            "uid",
            "generation",
            "resourceVersion",
            "labels",
            "annotations",
        ],
    )
    .len();
    let additional_count = top_level_fields_except(
        o,
        &[
            "apiVersion",
            "kind",
            "metadata",
            "spec",
            "status",
            "data",
            "binaryData",
            "stringData",
        ],
    )
    .len();
    let rules = if kind == "Role" || kind == "ClusterRole" {
        rbac_rules(o)
    } else {
        Vec::new()
    };
    let is_secret = kind == "Secret";
    let entries = if is_secret || kind == "ConfigMap" {
        data_entries(o, is_secret)
    } else {
        Vec::new()
    };
    let images = container_images(o);
    let envs = container_envs(o);
    let event_type = json_str(o, &["type"]);
    let event_reason = json_str(o, &["reason"]);
    let event_message = json_str(o, &["message"]);
    let event_action = json_str(o, &["action"]);
    let event_source =
        json_str(o, &["reportingComponent"]).or_else(|| json_str(o, &["source", "component"]));
    let event_count = json_str(o, &["series", "count"]).or_else(|| json_str(o, &["count"]));
    let event_first_seen = json_str(o, &["firstTimestamp"]);
    let event_last_seen = json_str(o, &["series", "lastObservedTime"])
        .or_else(|| json_str(o, &["eventTime"]))
        .or_else(|| json_str(o, &["lastTimestamp"]));
    let event_object = match (
        json_str(o, &["involvedObject", "kind"]),
        json_str(o, &["involvedObject", "name"]),
    ) {
        (Some(kind), Some(name)) => Some(format!("{kind} / {name}")),
        (None, Some(name)) => Some(name),
        _ => None,
    };
    let api_version = json_str(o, &["apiVersion"]);
    let uid = json_str(o, &["metadata", "uid"]);
    let generation = json_str(o, &["metadata", "generation"]);
    let resource_version = json_str(o, &["metadata", "resourceVersion"]);
    let condition_count = conds.len();
    let container_count = images.len();
    let env_count = envs
        .iter()
        .map(|container| container.entries.len())
        .sum::<usize>();
    let rule_count = rules.len();
    let entry_count = entries.len();
    let label_count = labels.len();
    let annotation_count = annotations.len();
    let related_event_count = d.events.len();
    let warning_count = d
        .events
        .iter()
        .filter(|event| event.type_.eq_ignore_ascii_case("warning"))
        .count();

    view! {
        <div class="info">
            {is_event.then(|| view! {
                <section class="event-detail-summary">
                    <div class="event-detail-head">
                        <div>
                            <span class="event-detail-label">"Kubernetes event"</span>
                            <h3>{event_reason.unwrap_or_else(|| "Unknown reason".to_string())}</h3>
                        </div>
                        {event_type.map(|value| {
                            let class = format!("event-type event-type-{}", value.to_lowercase());
                            view! { <span class=class>{value}</span> }
                        })}
                    </div>
                    {event_message.map(|message| view! {
                        <div class="event-detail-message">{message}</div>
                    })}
                    <div class="event-detail-context">
                        {event_object.map(|value| view! {
                            <div class="event-detail-row"><span>"Affected object"</span><strong>{value}</strong></div>
                        })}
                        {event_source.map(|value| view! {
                            <div class="event-detail-row"><span>"Reported by"</span><strong>{value}</strong></div>
                        })}
                        {event_action.map(|value| view! {
                            <div class="event-detail-row"><span>"Action"</span><strong>{value}</strong></div>
                        })}
                    </div>
                    <div class="event-detail-stats">
                        {event_count.map(|value| view! {
                            <div><span>"Occurrences"</span><strong>{value}</strong></div>
                        })}
                        {event_first_seen.map(|value| {
                            let age = data::humanize_age(&Some(value.clone()));
                            view! { <div><span>"First seen"</span><strong data-tip=value>{age}</strong></div> }
                        })}
                        {event_last_seen.map(|value| {
                            let age = data::humanize_age(&Some(value.clone()));
                            view! { <div><span>"Last seen"</span><strong data-tip=value>{age}</strong></div> }
                        })}
                    </div>
                </section>
            })}

            {certificate.map(|certificate| view! {
                <section class="certificate-detail-summary">
                    <div class="certificate-detail-heading">
                        <span>"Certificate lifecycle"</span>
                        <strong class=certificate.state_class>{certificate.state}</strong>
                    </div>
                    <div class="detail-stats">
                        <div class="detail-stat">
                            <span class="detail-stat-label">"Valid from"</span>
                            <span class="detail-stat-value" data-tip=certificate.not_before_raw>{certificate.not_before}</span>
                        </div>
                        <div class="detail-stat">
                            <span class="detail-stat-label">"Expires"</span>
                            <span class="detail-stat-value" data-tip=certificate.not_after_raw>{certificate.not_after}</span>
                        </div>
                        <div class="detail-stat">
                            <span class="detail-stat-label">"Scheduled renewal"</span>
                            <span class="detail-stat-value" data-tip=certificate.renewal_time_raw>{certificate.renewal_time}</span>
                        </div>
                        <div class="detail-stat">
                            <span class="detail-stat-label">"Revision"</span>
                            <span class="detail-stat-value">{certificate.revision}</span>
                        </div>
                        <div class="detail-stat">
                            <span class="detail-stat-label">"Target Secret"</span>
                            <span class="detail-stat-value">{certificate.secret}</span>
                        </div>
                    </div>
                </section>
            })}

            <section class="info-overview" aria-label="Resource overview">
                <div class="info-overview-heading">
                    <h3>{kind}</h3>
                    {api_version.clone().map(|value| view! { <code>{value}</code> })}
                </div>
                <div class="kv-grid overview-grid">
                    {d.namespace.clone().map(|ns| view! {
                        <div class="kv"><span class="k">"Namespace"</span><span class="v">{ns}</span></div>
                    })}
                    {created.map(|ts| {
                        let age = data::humanize_age(&Some(ts.clone()));
                        let label = format!("{age}; {ts}");
                        view! { <div class="kv"><span class="k">"Created"</span><time class="v" datetime=ts.clone() data-tip=ts aria-label=label>{age}</time></div> }
                    })}
                    {owners.into_iter().map(|(owner_kind, name)| view! {
                        <div class="kv"><span class="k">"Owner"</span><span class="v">{format!("{owner_kind}/{name}")}</span></div>
                    }).collect_view()}
                    {stats.into_iter().map(|(key, value)| view! {
                        <div class="kv"><span class="k">{camel_label(&key)}</span><span class="v">{value}</span></div>
                    }).collect_view()}
                </div>
            </section>

            {(condition_count > 0).then(|| view! {
                <details class="info-section conditions-section" open>
                    <summary><span>"Conditions"</span><small>{counted(condition_count, "condition", "conditions")}</small></summary>
                    <div class="info-section-body condition-list">
                        {conds.into_iter().map(|condition| {
                            let class = condition_class(&condition.type_, &condition.status);
                            let transition = condition.last_transition.map(|value| {
                                let age = data::humanize_age(&Some(value.clone()));
                                let label = format!("{age}; {value}");
                                view! { <time datetime=value.clone() data-tip=value aria-label=label>{age}</time> }
                            });
                            view! {
                                <article class="condition-row">
                                    <div class="condition-head">
                                        <strong>{condition.type_}</strong>
                                        <span class=format!("condition-state {class}")>{condition.status}</span>
                                        {transition}
                                    </div>
                                    {(!condition.reason.is_empty()).then(|| view! { <div class="condition-reason">{condition.reason}</div> })}
                                    {(!condition.message.is_empty()).then(|| view! { <p>{condition.message}</p> })}
                                    {condition.observed_generation.map(|value| view! { <small>"Observed generation "{value}</small> })}
                                </article>
                            }
                        }).collect_view()}
                    </div>
                </details>
            })}

            {(status_detail_count > 0).then(|| view! {
                <details class="info-section">
                    <summary><span>"Status details"</span><small>{counted(status_detail_count, "field", "fields")}</small></summary>
                    <div class="info-section-body">{json_fields(status_value)}</div>
                </details>
            })}

            {(spec_count > 0).then(|| view! {
                <details class="info-section" open=spec_count <= 12>
                    <summary><span>"Specification"</span><small>{counted(spec_count, "field", "fields")}</small></summary>
                    <div class="info-section-body">{json_fields(spec_value)}</div>
                </details>
            })}

            {(container_count > 0).then(|| view! {
                <details class="info-section" open>
                    <summary><span>"Containers"</span><small>{counted(container_count, "image", "images")}</small></summary>
                    <div class="info-section-body kv-cols container-images">
                        {images.into_iter().map(|(name, image)| view! {
                            <div class="kvc"><span class="kvc-k">{name}</span><span class="kvc-v">{image}</span></div>
                        }).collect_view()}
                    </div>
                </details>
            })}

            {(env_count > 0).then(|| {
                let multi = envs.len() > 1;
                view! {
                    <details class="info-section">
                        <summary><span>"Environment"</span><small>{counted(env_count, "variable", "variables")}</small></summary>
                        <div class="info-section-body">
                            {envs.into_iter().map(|container| view! {
                                {multi.then(|| view! { <div class="env-container-name">{container.container}</div> })}
                                <div class="kvlist">
                                    {container.entries.into_iter().map(|(key, value)| view! {
                                        <div class="kvl"><span class="kvl-k">{key}</span><span class="kvl-v">{value}</span></div>
                                    }).collect_view()}
                                </div>
                            }).collect_view()}
                        </div>
                    </details>
                }
            })}

            {(rule_count > 0).then(|| view! {
                <details class="info-section" open>
                    <summary><span>"Access rules"</span><small>{counted(rule_count, "rule", "rules")}</small></summary>
                    <div class="info-section-body info-table-scroll">
                        <table class="cond rules">
                            <thead><tr><th>"API Groups"</th><th>"Resources"</th><th>"Verbs"</th><th>"Names / URLs"</th></tr></thead>
                            <tbody>{rules.into_iter().map(|rule| view! {
                                <tr><td>{rule.groups}</td><td>{rule.resources}</td><td class="rule-verbs">{rule.verbs}</td><td>{rule.names}</td></tr>
                            }).collect_view()}</tbody>
                        </table>
                    </div>
                </details>
            })}

            {(entry_count > 0).then(|| view! {
                <details class="info-section" open>
                    <summary><span>"Data"</span><small>{counted(entry_count, "entry", "entries")}</small></summary>
                    <div class="info-section-body">
                        {is_secret.then(|| view! { <div class="hint">"Values are hidden. Select a value to reveal it."</div> })}
                        <div class="data">
                            {entries.into_iter().map(|(key, value, secret)| {
                                let revealed = RwSignal::new(false);
                                view! {
                                    <div class="data-row">
                                        <div class="data-key">{key}</div>
                                        {if secret {
                                            view! { <button type="button" class="data-val secret" class:revealed=move || revealed.get()
                                                aria-pressed=move || revealed.get().to_string()
                                                aria-label=move || if revealed.get() { "Hide secret value" } else { "Reveal secret value" }
                                                on:click=move |_| revealed.update(|value| *value = !*value)>
                                                {move || if revealed.get() { value.clone() } else { "Hidden value".to_string() }}
                                            </button> }.into_any()
                                        } else {
                                            view! { <pre class="data-val">{value}</pre> }.into_any()
                                        }}
                                    </div>
                                }
                            }).collect_view()}
                        </div>
                    </div>
                </details>
            })}

            {(related_event_count > 0).then(|| view! {
                <details class="info-section events-section" open=warning_count > 0>
                    <summary>
                        <span>"Recent events"</span>
                        <small>{
                            let events = counted(related_event_count, "event", "events");
                            if warning_count > 0 { format!("{events} · {}", counted(warning_count, "warning", "warnings")) } else { events }
                        }</small>
                    </summary>
                    <div class="info-section-body events">
                        {d.events.iter().map(|event| {
                            let age = event.age.clone().map(|value| {
                                let label = data::humanize_age(&Some(value.clone()));
                                let accessible_label = format!("{label}; {value}");
                                view! { <time datetime=value.clone() data-tip=value aria-label=accessible_label>{label}</time> }
                            });
                            view! {
                                <article class=format!("event ev-{}", event.type_.to_lowercase())>
                                    <div class="event-head">
                                        <strong class="ev-reason">{event.reason.clone()}</strong>
                                        <span class="event-meta">{age}{(event.count > 1).then(|| view! { <span>{format!("×{}", event.count)}</span> })}</span>
                                    </div>
                                    <p class="ev-msg">{event.message.clone()}</p>
                                </article>
                            }
                        }).collect_view()}
                    </div>
                </details>
            })}

            {(additional_count > 0).then(|| view! {
                <details class="info-section">
                    <summary><span>"Additional fields"</span><small>{counted(additional_count, "field", "fields")}</small></summary>
                    <div class="info-section-body">{json_fields_except(additional_value, &[
                        "apiVersion", "kind", "metadata", "spec", "status", "data", "binaryData", "stringData",
                    ])}</div>
                </details>
            })}

            <details class="info-section metadata-section">
                <summary>
                    <span>"Resource metadata"</span>
                    <small>{counted(metadata_count, "field", "fields")}</small>
                </summary>
                <div class="info-section-body">
                    <div class="kv-grid metadata-grid">
                        {api_version.map(|value| view! { <div class="kv"><span class="k">"API version"</span><span class="v">{value}</span></div> })}
                        {uid.map(|value| view! { <div class="kv"><span class="k">"UID"</span><span class="v font-mono">{value}</span></div> })}
                        {generation.map(|value| view! { <div class="kv"><span class="k">"Generation"</span><span class="v">{value}</span></div> })}
                        {resource_version.map(|value| view! { <div class="kv"><span class="k">"Resource version"</span><span class="v font-mono">{value}</span></div> })}
                    </div>
                    {(metadata_detail_count > 0).then(|| view! {
                        <h5>"Additional metadata"</h5>
                        {json_fields_except(metadata_value, &[
                            "name", "namespace", "creationTimestamp", "uid", "generation", "resourceVersion", "labels", "annotations",
                        ])}
                    })}
                    {(label_count > 0).then(|| view! {
                        <h5>"Labels"</h5>
                        <div class="kvlist">{labels.into_iter().map(|(key, value)| view! {
                            <div class="kvl"><span class="kvl-k">{key}</span><span class="kvl-v">{value}</span></div>
                        }).collect_view()}</div>
                    })}
                    {(annotation_count > 0).then(|| view! {
                        <h5>"Annotations"</h5>
                        <div class="kvlist">{annotations.into_iter().map(|(key, value)| view! {
                            <div class="kvl"><span class="kvl-k">{key}</span><span class="kvl-v">{value}</span></div>
                        }).collect_view()}</div>
                    })}
                </div>
            </details>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn certificate_summary_prioritizes_active_renewal() {
        let summary = certificate_summary(&json!({
            "spec": {"secretName": "api-tls"},
            "status": {
                "notAfter": "2026-10-01T12:00:00Z",
                "renewalTime": "2026-09-01T12:00:00Z",
                "revision": 3,
                "conditions": [
                    {"type": "Ready", "status": "True"},
                    {"type": "Issuing", "status": "True"}
                ]
            }
        }));
        assert_eq!(summary.state, "Renewing");
        assert_eq!(summary.state_class, "pending");
        assert_eq!(summary.not_after, "2026-10-01 12:00:00");
        assert_eq!(summary.revision, "3");
        assert_eq!(summary.secret, "api-tls");
    }
}

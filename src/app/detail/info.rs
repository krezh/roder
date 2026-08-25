//! Describe-style summary built from the object JSON: metadata, conditions,
//! status fields, labels, data (for ConfigMaps/Secrets), and events.

use leptos::prelude::*;
use roder_core::ObjectDetail;

use crate::app::util::format::camel_label;
use crate::app::util::json::{
    conditions, container_envs, container_images, data_entries, json_map, json_str, owner_refs,
    rbac_rules, section_scalars, status_scalars,
};
use crate::data;

pub(crate) fn info_view(d: ObjectDetail, kind: String) -> impl IntoView {
    let o = &d.object;
    let is_event = kind == "Event";
    let created = json_str(o, &["metadata", "creationTimestamp"]);
    let labels = json_map(o, &["metadata", "labels"]);
    let annotations = json_map(o, &["metadata", "annotations"]);
    let owners = owner_refs(o);
    let conds = conditions(o);
    let stats = status_scalars(o);
    let spec = section_scalars(o, "spec");
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

            <div class="kv-grid">
                {d.namespace.clone().map(|ns| view! {
                    <div class="kv"><span class="k">"Namespace"</span><span class="v">{ns}</span></div>
                })}
                {created.map(|ts| view! {
                    <div class="kv"><span class="k">"Age"</span><span class="v">{data::humanize_age(&Some(ts))}</span></div>
                })}
                {owners.into_iter().map(|(k, n)| view! {
                    <div class="kv"><span class="k">"Controlled By"</span><span class="v">{format!("{k}/{n}")}</span></div>
                }).collect_view()}
                {stats.into_iter().map(|(k, v)| view! {
                    <div class="kv"><span class="k">{camel_label(&k)}</span><span class="v">{v}</span></div>
                }).collect_view()}
            </div>

            {(!spec.is_empty()).then(|| view! {
                <h4>"Spec"</h4>
                <div class="kv-cols">
                    {spec.into_iter().map(|(k, v)| view! {
                        <div class="kvc"><span class="kvc-k">{camel_label(&k)}</span><span class="kvc-v">{v}</span></div>
                    }).collect_view()}
                </div>
            })}

            {(!images.is_empty()).then(|| view! {
                <h4>"Containers"</h4>
                <div class="kv-cols container-images">
                    {images.into_iter().map(|(name, image)| view! {
                        <div class="kvc"><span class="kvc-k">{name}</span><span class="kvc-v">{image}</span></div>
                    }).collect_view()}
                </div>
            })}

            {(!envs.is_empty()).then(|| {
                let multi = envs.len() > 1;
                view! {
                    <h4>"Env"</h4>
                    {envs.into_iter().map(|ce| view! {
                        {multi.then(|| view! { <div class="env-container-name">{ce.container}</div> })}
                        <div class="kvlist">
                            {ce.entries.into_iter().map(|(k, v)| view! {
                                <div class="kvl">
                                    <span class="kvl-k">{format!("{k}:")}</span>
                                    <span class="kvl-v">{v}</span>
                                </div>
                            }).collect_view()}
                        </div>
                    }).collect_view()}
                }
            })}

            {(!rules.is_empty()).then(|| view! {
                <h4>"Rules"</h4>
                <table class="cond rules">
                    <thead><tr><th>"API Groups"</th><th>"Resources"</th><th>"Verbs"</th><th>"Names / URLs"</th></tr></thead>
                    <tbody>
                        {rules.into_iter().map(|r| view! {
                            <tr>
                                <td>{r.groups}</td>
                                <td>{r.resources}</td>
                                <td class="rule-verbs">{r.verbs}</td>
                                <td>{r.names}</td>
                            </tr>
                        }).collect_view()}
                    </tbody>
                </table>
            })}

            {(!entries.is_empty()).then(|| view! {
                <h4>"Data"</h4>
                {is_secret.then(|| view! { <div class="hint">"Values are hidden — click to reveal."</div> })}
                <div class="data">
                    {entries.into_iter().map(|(k, v, secret)| {
                        let revealed = RwSignal::new(false);
                        view! {
                            <div class="data-row">
                                <div class="data-key">{k}</div>
                                {if secret {
                                    view! {
                                        <pre class="data-val secret" class:revealed=move || revealed.get()
                                            on:click=move |_| revealed.set(true)>{v}</pre>
                                    }.into_any()
                                } else {
                                    view! { <pre class="data-val">{v}</pre> }.into_any()
                                }}
                            </div>
                        }
                    }).collect_view()}
                </div>
            })}

            {(!conds.is_empty()).then(|| view! {
                <h4>"Conditions"</h4>
                <table class="cond">
                    <thead><tr><th>"Type"</th><th>"Status"</th><th>"Reason"</th><th>"Message"</th></tr></thead>
                    <tbody>
                        {conds.into_iter().map(|c| {
                            let cls = match c.status.as_str() { "True" => "ok", "False" => "error", _ => "pending" };
                            view! {
                                <tr>
                                    <td>{c.type_}</td>
                                    <td><span class=format!("cond-{cls}")>{c.status}</span></td>
                                    <td>{c.reason}</td>
                                    <td class="cond-msg">{c.message}</td>
                                </tr>
                            }
                        }).collect_view()}
                    </tbody>
                </table>
            })}

            {(!labels.is_empty()).then(|| view! {
                <h4>"Labels"</h4>
                <div class="kvlist">
                    {labels.into_iter().map(|(k, v)| view! {
                        <div class="kvl"><span class="kvl-k">{format!("{}:", k)}</span><span class="kvl-v">{v}</span></div>
                    }).collect_view()}
                </div>
            })}

            {(!annotations.is_empty()).then(|| view! {
                <h4>"Annotations"</h4>
                <div class="kvlist">
                    {annotations.into_iter().map(|(k, v)| view! {
                        <div class="kvl"><span class="kvl-k">{format!("{}:", k)}</span><span class="kvl-v">{v}</span></div>
                    }).collect_view()}
                </div>
            })}

            {(!d.events.is_empty()).then(|| view! {
                <h4>"Events"</h4>
                <div class="events">
                    {d.events.iter().take(12).map(|e| view! {
                        <div class=format!("event ev-{}", e.type_.to_lowercase())>
                            <span class="ev-reason">{e.reason.clone()}</span>
                            <span class="ev-msg">{e.message.clone()}</span>
                        </div>
                    }).collect_view()}
                </div>
            })}
        </div>
    }
}

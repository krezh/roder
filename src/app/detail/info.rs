//! Describe-style summary built from the object JSON: metadata, conditions,
//! status fields, labels, data (for ConfigMaps/Secrets), and events.

use leptos::prelude::*;
use roder_core::ObjectDetail;

use crate::app::util::format::camel_label;
use crate::app::util::json::{
    conditions, data_entries, json_map, json_str, owner_refs, rbac_rules, section_scalars,
    status_scalars,
};
use crate::data;

pub(crate) fn info_view(d: ObjectDetail, kind: String) -> impl IntoView {
    let o = &d.object;
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

    view! {
        <div class="info">
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

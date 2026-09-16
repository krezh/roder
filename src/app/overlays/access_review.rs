//! RBAC access review overlay: which verbs the current identity may perform
//! across every known resource kind, given OIDC passthrough.

use std::collections::HashSet;

use leptos::prelude::*;
use roder_core::{AccessRow, Category, ACCESS_REVIEW_OPERATIONS};

use crate::app::state::AccessReviewOpen;
use crate::data;

/// Identical `<colgroup>` for the header and body tables, since `table-layout:
/// fixed` needs matching column widths declared in both to stay aligned.
fn access_colgroup() -> impl IntoView {
    view! {
        <colgroup>
            <col style="width: 34%" />
            {ACCESS_REVIEW_OPERATIONS.iter().map(|_| view! { <col /> }).collect_view()}
        </colgroup>
    }
}

fn grouped_rows(rows: Vec<AccessRow>) -> Vec<(Category, Vec<AccessRow>)> {
    let mut groups: Vec<(Category, Vec<AccessRow>)> = Vec::new();
    for row in rows {
        match groups.last_mut() {
            Some((category, rows)) if *category == row.category => rows.push(row),
            _ => groups.push((row.category.clone(), vec![row])),
        }
    }
    groups
}

fn has_restriction(row: &AccessRow) -> bool {
    row.operations
        .iter()
        .any(|(_, allowed)| *allowed == Some(false))
}

#[component]
pub(crate) fn AccessReview() -> impl IntoView {
    let open = expect_context::<AccessReviewOpen>().0;
    let selected_ns = expect_context::<RwSignal<Option<String>>>();
    let (visible, closing, do_close) = crate::app::ui::use_bool_overlay(open);
    let dialog_ref = NodeRef::<leptos::html::Div>::new();
    crate::app::ui::use_dialog_focus(dialog_ref);

    let rows = RwSignal::new(None::<Result<Vec<AccessRow>, String>>);
    let query = RwSignal::new(String::new());
    let restrictions_only = RwSignal::new(false);
    let categories = RwSignal::new(Vec::<Category>::new());
    let expanded = RwSignal::new(HashSet::<Category>::new());
    // Fetch fresh on every open (permissions can change) rather than eagerly
    // on mount — this is ~250 SelfSubjectAccessReview calls, best kept
    // on-demand even though `can()` caches each one for 30s.
    Effect::new(move |_| {
        if open.get() {
            query.set(String::new());
            restrictions_only.set(false);
            let ns = selected_ns.get_untracked();
            leptos::task::spawn_local(async move {
                let url = match ns {
                    Some(ns) if !ns.is_empty() => {
                        format!("/api/access-review?namespace={}", data::percent_encode(&ns))
                    }
                    _ => "/api/access-review".to_string(),
                };
                let result = data::fetch_json::<Vec<AccessRow>>(&url).await;
                if let Ok(loaded) = &result {
                    let groups = grouped_rows(loaded.clone());
                    categories.set(
                        groups
                            .iter()
                            .map(|(category, _)| category.clone())
                            .collect(),
                    );
                    expanded.set(
                        groups
                            .iter()
                            .filter(|(category, rows)| {
                                category.order() <= Category::Rbac.order() || rows.len() <= 10
                            })
                            .map(|(category, _)| category.clone())
                            .collect(),
                    );
                } else {
                    categories.set(Vec::new());
                    expanded.set(HashSet::new());
                }
                rows.set(Some(result));
            });
        } else {
            rows.set(None);
        }
    });

    view! {
        {move || visible.get().then(|| view! {
            <div class="access-scrim" class:closing=move || closing.get()
                on:click=move |_| do_close()></div>
            <div class="access-modal" class:closing=move || closing.get() node_ref=dialog_ref
                role="dialog" aria-modal="true" tabindex="-1"
                on:click=move |e: leptos::ev::MouseEvent| e.stop_propagation()>
                <div class="access-head">
                    <span class="access-title">"Access Review"</span>
                    <span class="access-sub">
                        {move || selected_ns.get().unwrap_or_else(|| "All namespaces".to_string())}
                    </span>
                    <button class="access-close" on:click=move |_| do_close()>"✕"</button>
                </div>
                <div class="access-body">
                    {move || match rows.get() {
                        None => view! { <div class="access-loading muted">"Loading…"</div> }.into_any(),
                        Some(Err(e)) => view! { <div class="access-error error">{format!("Failed to load: {e}")}</div> }.into_any(),
                        Some(Ok(rows)) => {
                            let total = rows.len();
                            let needle = query.get().trim().to_lowercase();
                            let searching = !needle.is_empty();
                            let only_restricted = restrictions_only.get();
                            let filtered = rows.into_iter().filter(|row| {
                                (!only_restricted || has_restriction(row))
                                    && (needle.is_empty()
                                        || row.kind.to_lowercase().contains(&needle)
                                        || row.group.to_lowercase().contains(&needle)
                                        || row.category.label().to_lowercase().contains(&needle))
                            }).collect::<Vec<_>>();
                            let shown = filtered.len();
                            let groups = grouped_rows(filtered);
                            let open_categories = expanded.get();
                            let empty_message = if only_restricted {
                                "No restricted resources in this scope"
                            } else {
                                "No matching resources"
                            };

                            view! {
                                <div class="access-tools">
                                    <input type="search" aria-label="Filter access review resources"
                                        placeholder="Filter by kind, group, or category"
                                        prop:value=move || query.get()
                                        on:input=move |event| query.set(event_target_value(&event)) />
                                    <button type="button" class="act access-filter" class:active=move || restrictions_only.get()
                                        aria-pressed=move || restrictions_only.get().to_string()
                                        on:click=move |_| restrictions_only.update(|only| *only = !*only)>
                                        "Only restrictions"
                                    </button>
                                    <span class="access-result-count">{format!("{shown} of {total} resources")}</span>
                                    <button type="button" class="act" on:click=move |_| {
                                        expanded.set(categories.get().into_iter().collect());
                                    }>"Expand all"</button>
                                    <button type="button" class="act" on:click=move |_| expanded.set(HashSet::new())>
                                        "Collapse all"
                                    </button>
                                </div>
                                <div class="access-table-frame">
                                    <div class="access-table-inner">
                                        <table class="access-table access-table-head">
                                            {access_colgroup()}
                                            <thead>
                                                <tr>
                                                    <th>"Resource"</th>
                                                    {ACCESS_REVIEW_OPERATIONS.iter().map(|v| view! { <th>{*v}</th> }).collect_view()}
                                                </tr>
                                            </thead>
                                        </table>
                                        <div class="access-table-scroll">
                                            <table class="access-table">
                                                {access_colgroup()}
                                                {groups.is_empty().then(|| view! {
                                                    <tbody><tr><td class="access-empty" colspan=ACCESS_REVIEW_OPERATIONS.len() + 1>{empty_message}</td></tr></tbody>
                                                })}
                                                {groups.into_iter().map(|(category, rows)| {
                                                    let count = rows.len();
                                                    let is_expanded = searching || open_categories.contains(&category);
                                                    let toggle_category = category.clone();
                                                    view! {
                                                        <tbody>
                                                            <tr class="access-group-row">
                                                                <th colspan=ACCESS_REVIEW_OPERATIONS.len() + 1 scope="rowgroup">
                                                                    <button type="button" class="access-group-toggle"
                                                                        disabled=searching aria-expanded=is_expanded.to_string()
                                                                        on:click=move |_| expanded.update(|open| {
                                                                            if !open.remove(&toggle_category) {
                                                                                open.insert(toggle_category.clone());
                                                                            }
                                                                        })>
                                                                        <span class="access-group-label">
                                                                            <span class="access-group-name"><span class="access-group-caret"></span>{category.label()}</span>
                                                                            <small>{format!("{count} resources")}</small>
                                                                        </span>
                                                                    </button>
                                                                </th>
                                                            </tr>
                                                            {is_expanded.then(|| rows.into_iter().map(|row| {
                                                                let title = if row.group.is_empty() { row.kind.clone() } else { format!("{}.{}", row.kind, row.group) };
                                                                view! {
                                                                    <tr>
                                                                        <td class="access-kind" title=title>
                                                                            <span class="access-kind-name">{row.kind}</span>
                                                                            {(!row.group.is_empty()).then(|| view! { <span class="access-kind-group">{row.group}</span> })}
                                                                        </td>
                                                                        {row.operations.into_iter().map(|(_, allowed)| view! {
                                                                            <td class="access-cell">
                                                                                <span class=match allowed {
                                                                                    Some(true) => "access-yes",
                                                                                    Some(false) => "access-no",
                                                                                    None => "muted",
                                                                                }>
                                                                                    {match allowed {
                                                                                        Some(true) => "✓",
                                                                                        Some(false) => "✕",
                                                                                        None => "·",
                                                                                    }}
                                                                                </span>
                                                                            </td>
                                                                        }).collect_view()}
                                                                    </tr>
                                                                }
                                                            }).collect_view())}
                                                        </tbody>
                                                    }
                                                }).collect_view()}
                                            </table>
                                        </div>
                                    </div>
                                </div>
                            }.into_any()
                        },
                    }}
                </div>
            </div>
        })}
    }
}

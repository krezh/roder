//! RBAC access review overlay: which verbs the current identity may perform
//! across every known resource kind, given OIDC passthrough.

use leptos::prelude::*;
use roder_core::{AccessRow, ACCESS_REVIEW_VERBS};

use crate::app::state::AccessReviewOpen;
use crate::data;

/// Identical `<colgroup>` for the header and body tables, since `table-layout:
/// fixed` needs matching column widths declared in both to stay aligned.
fn access_colgroup() -> impl IntoView {
    view! {
        <colgroup>
            <col style="width: 40%" />
            {ACCESS_REVIEW_VERBS.iter().map(|_| view! { <col /> }).collect_view()}
        </colgroup>
    }
}

#[component]
pub(crate) fn AccessReview() -> impl IntoView {
    let open = expect_context::<AccessReviewOpen>().0;
    let selected_ns = expect_context::<RwSignal<Option<String>>>();
    let (visible, closing, do_close) = super::use_bool_overlay(open);
    let dialog_ref = NodeRef::<leptos::html::Div>::new();
    crate::app::ui::use_dialog_focus(dialog_ref);

    let rows = RwSignal::new(None::<Result<Vec<AccessRow>, String>>);
    // Fetch fresh on every open (permissions can change) rather than eagerly
    // on mount — this is ~250 SelfSubjectAccessReview calls, best kept
    // on-demand even though `can()` caches each one for 30s.
    Effect::new(move |_| {
        if open.get() {
            let ns = selected_ns.get_untracked();
            leptos::task::spawn_local(async move {
                let url = match ns {
                    Some(ns) if !ns.is_empty() => {
                        format!("/api/access-review?namespace={}", data::percent_encode(&ns))
                    }
                    _ => "/api/access-review".to_string(),
                };
                rows.set(Some(data::fetch_json::<Vec<AccessRow>>(&url).await));
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
                        Some(Ok(rows)) => view! {
                            <table class="access-table access-table-head">
                                {access_colgroup()}
                                <thead>
                                    <tr>
                                        <th>"Kind"</th>
                                        {ACCESS_REVIEW_VERBS.iter().map(|v| view! { <th>{*v}</th> }).collect_view()}
                                    </tr>
                                </thead>
                            </table>
                            <div class="access-table-scroll">
                                <table class="access-table">
                                    {access_colgroup()}
                                    <tbody>
                                        {rows.into_iter().map(|row| {
                                            let name = if row.group.is_empty() { row.kind.clone() } else { format!("{}.{}", row.kind, row.group) };
                                            view! {
                                                <tr>
                                                    <td class="access-kind">{name}</td>
                                                    {row.verbs.into_iter().map(|(_, allowed)| view! {
                                                        <td class="access-cell">
                                                            <span class=if allowed { "access-yes" } else { "access-no" }>
                                                                {if allowed { "✓" } else { "✕" }}
                                                            </span>
                                                        </td>
                                                    }).collect_view()}
                                                </tr>
                                            }
                                        }).collect_view()}
                                    </tbody>
                                </table>
                            </div>
                        }.into_any(),
                    }}
                </div>
            </div>
        })}
    }
}

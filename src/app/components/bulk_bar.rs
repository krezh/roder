//! The desktop grid's bulk-action bar: the strip that slides up when rows are
//! selected, offering every action the kind supports across the whole
//! selection.
//!
//! Which buttons exist is decided by the kind's capabilities; whether each is
//! *enabled*, and whether it shows an `allowed/total` count, comes from a live
//! `SelfSubjectAccessReview` over the selected resources. Both live here rather
//! than in `kind_table`, so the grid itself deals only with rows.

use leptos::prelude::*;
use roder_core::{ResourceAction, ResourceKind, RowStatus};

use crate::app::controllers::detail::selection_permissions_resource;
use crate::app::events::{make_bulk_open_logs, make_do_bulk, make_do_delete, RowMap, UidSet};
use crate::app::state::LogPods;
use crate::app::table_logic;
use crate::app::ui::confirm::{ask_confirm, Confirm};
use crate::app::ui::delete::{ask_delete, DeleteRequest};
use crate::app::ui::toast::Toast;
use crate::app::util::predicate::KindKind;

#[component]
pub(crate) fn BulkBar(
    kind: ResourceKind,
    rows: RowMap,
    selected: UidSet,
    /// The rows currently passing the filter — what "Select all" selects.
    shown_uids: Memo<Vec<String>>,
) -> impl IntoView {
    let toast = expect_context::<RwSignal<Option<Toast>>>();
    let confirm = expect_context::<RwSignal<Option<Confirm>>>();
    let delete_confirm = expect_context::<RwSignal<Option<DeleteRequest>>>();
    let log_pods = expect_context::<LogPods>().0;

    let kk = KindKind::new(&kind.group, &kind.version, &kind.kind);
    let show_workload = kk.supports(ResourceAction::Restart);
    let show_job = kk.supports(ResourceAction::JobRerun);
    let show_flux_reconcile = kk.supports(ResourceAction::FluxReconcile);
    let show_flux_suspend = kk.supports(ResourceAction::FluxSuspend);
    let show_certificate = kk.supports(ResourceAction::CertificateRenew);
    let show_helmrelease = kk.supports(ResourceAction::FluxForce);
    let show_source_ref = kk.supports(ResourceAction::FluxReconcileWithSource);
    let show_logs = kk.supports(ResourceAction::Logs);
    let show_eso = kk.supports(ResourceAction::ExternalSecretsRefresh);
    let show_cronjob = kk.supports(ResourceAction::CronJobTrigger);
    let show_kopiur = kk.supports(ResourceAction::KopiurSnapshotNow);

    let is_pod_kind = kind.group.is_empty() && kind.kind == "Pod";
    let key_sv = StoredValue::new(kind.key.clone());

    let permissions = selection_permissions_resource(move || {
        let key = key_sv.get_value();
        let uids = selected.get();
        rows.with(|rows| table_logic::bulk_targets(&key, rows, &uids))
    });
    let allowed = move |action| {
        permissions
            .get()
            .is_some_and(|permissions| permissions.allows_all(action))
    };
    // Where RBAC permits the action on only part of the selection, say so on
    // the button rather than silently acting on a subset.
    let label = move |action, label: &'static str| {
        let Some(permissions) = permissions.get() else {
            return label.to_string();
        };
        let (allowed, total) = permissions.count(action);
        if total > 0 && allowed < total {
            format!("{label} {allowed}/{total}")
        } else {
            label.to_string()
        }
    };
    let can_rerun_selected_jobs = move || {
        let selected = selected.get();
        !selected.is_empty()
            && rows.with(|rows| {
                selected.iter().all(|uid| {
                    rows.get(uid)
                        .is_some_and(|row| matches!(row.status, RowStatus::Ok | RowStatus::Error))
                })
            })
    };
    // Suspend and Resume are offered only when the selection agrees, so a mixed
    // selection can't be flipped half one way and half the other.
    let suspend_state = move || -> Option<bool> {
        let uids = selected.get();
        rows.with(|rm| {
            let mut states = uids.iter().filter_map(|u| rm.get(u)).map(|r| r.suspended);
            let first = states.next()?;
            states.all(|s| s == first).then_some(first)
        })
    };
    let show_suspend = move || suspend_state() != Some(true);
    let show_resume = move || suspend_state() != Some(false);

    let reset_selection = move || selected.set(std::collections::BTreeSet::new());
    let do_bulk = make_do_bulk(toast, key_sv, rows, selected, reset_selection);
    let do_delete = make_do_delete(toast, key_sv, rows, selected, reset_selection);
    let do_logs = make_bulk_open_logs(
        log_pods,
        key_sv,
        rows,
        selected,
        is_pod_kind,
        reset_selection,
    );

    view! {
        <div class="bulkbar-wrap" class:open=move || !selected.get().is_empty()>
            <div class="bulkbar">
                <span class="bulk-count">{move || format!("{} selected", selected.get().len())}</span>
                <button class="act" on:click=move |_| selected.set(shown_uids.get().into_iter().collect())>"Select all"</button>
                <button class="act" on:click=move |_| reset_selection()>"Clear"</button>
                {show_logs.then(|| view! {
                    <button class="act" disabled=move || !allowed(ResourceAction::Logs) on:click=move |_| do_logs()>{move || label(ResourceAction::Logs, "Logs")}</button>
                })}
                {show_workload.then(|| view! {
                    <button class="act" disabled=move || !allowed(ResourceAction::Restart) on:click=move |_| do_bulk("restart")>{move || label(ResourceAction::Restart, "Restart")}</button>
                })}
                {show_job.then(|| view! {
                    <button class="act" disabled=move || !can_rerun_selected_jobs() || !allowed(ResourceAction::JobRerun)
                        data-tip="Only completed or failed Jobs can be re-run"
                        on:click=move |_| do_bulk("job-rerun")>{move || label(ResourceAction::JobRerun, "Re-run")}</button>
                })}
                {show_flux_reconcile.then(|| view! {
                    <button class="act" disabled=move || !allowed(ResourceAction::FluxReconcile) on:click=move |_| do_bulk("flux-reconcile")>{move || label(ResourceAction::FluxReconcile, "Reconcile")}</button>
                    {show_source_ref.then(|| view! {
                        <button class="act" disabled=move || !allowed(ResourceAction::FluxReconcileWithSource) on:click=move |_| do_bulk("flux-reconcile-with-source")>{move || label(ResourceAction::FluxReconcileWithSource, "Reconcile w/ source")}</button>
                    })}
                    {show_helmrelease.then(|| view! {
                        <button class="act" disabled=move || !allowed(ResourceAction::FluxForce) on:click=move |_| do_bulk("flux-force")>{move || label(ResourceAction::FluxForce, "Force")}</button>
                        <button class="act" disabled=move || !allowed(ResourceAction::FluxReset) on:click=move |_| do_bulk("flux-reset")>{move || label(ResourceAction::FluxReset, "Reset")}</button>
                    })}
                })}
                {show_flux_suspend.then(|| view! {
                    {move || show_suspend().then(|| view! {
                        <button class="act" disabled=move || !allowed(ResourceAction::FluxSuspend) on:click=move |_| do_bulk("flux-suspend")>{move || label(ResourceAction::FluxSuspend, "Suspend")}</button>
                    })}
                    {move || show_resume().then(|| view! {
                        <button class="act" disabled=move || !allowed(ResourceAction::FluxSuspend) on:click=move |_| do_bulk("flux-resume")>{move || label(ResourceAction::FluxSuspend, "Resume")}</button>
                    })}
                })}
                {show_certificate.then(|| view! {
                    <button class="act" disabled=move || !allowed(ResourceAction::CertificateRenew) on:click=move |_| {
                        let n = selected.get_untracked().len();
                        ask_confirm(
                            confirm,
                            format!("Force renewal of {n} Certificates?"),
                            "Renew",
                            move || do_bulk("certificate-renew"),
                        );
                    }>{move || label(ResourceAction::CertificateRenew, "Force renew")}</button>
                })}
                {show_eso.then(|| view! {
                    <button class="act" disabled=move || !allowed(ResourceAction::ExternalSecretsRefresh) on:click=move |_| do_bulk("eso-refresh")>{move || label(ResourceAction::ExternalSecretsRefresh, "Refresh")}</button>
                })}
                {show_cronjob.then(|| view! {
                    <button class="act" disabled=move || !allowed(ResourceAction::CronJobTrigger) on:click=move |_| do_bulk("cronjob-trigger")>{move || label(ResourceAction::CronJobTrigger, "Trigger")}</button>
                })}
                {show_kopiur.then(|| view! {
                    <button class="act" disabled=move || !allowed(ResourceAction::KopiurSnapshotNow) on:click=move |_| do_bulk("kopiur-snapshot-now")>{move || label(ResourceAction::KopiurSnapshotNow, "Snapshot now")}</button>
                })}
                <button class="act danger" disabled=move || !allowed(ResourceAction::Delete) on:click=move |_| {
                    let n = selected.get_untracked().len();
                    ask_delete(delete_confirm, format!("Delete {n} resources?"), do_delete);
                }>{move || label(ResourceAction::Delete, "Delete")}</button>
            </div>
        </div>
    }
}

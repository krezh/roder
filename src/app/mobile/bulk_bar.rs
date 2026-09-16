//! The mobile bulk-action bar shown while one or more cards are selected —
//! shared by every mobile list (single-kind, search, workspace pane) so the
//! same action set/markup doesn't get re-typed per list.

use leptos::prelude::*;
use roder_core::ResourceAction;

use crate::app::controllers::detail::SelectionPermissions;
use crate::app::events::UidSet;
use crate::app::ui::confirm::{ask_confirm, Confirm};
use crate::app::ui::delete::{ask_delete, DeleteRequest};

#[component]
pub(crate) fn MobileBulkBar(
    selected: UidSet,
    /// Turned off (clearing `selected`, via `use_select_mode`'s effect) by
    /// "Done" — the only way out of select mode now that hold is the only
    /// way in.
    select_mode: RwSignal<bool>,
    /// Every uid currently shown, for the "All" button.
    all_uids: impl Fn() -> Vec<String> + Copy + Send + Sync + 'static,
    /// Dispatches a bulk action by name (mirrors desktop's `do_bulk`/`fire_action`).
    do_bulk: impl Fn(&'static str) + Copy + Send + Sync + 'static,
    /// Dispatches the bulk delete with its force/propagation options (mirrors
    /// desktop's `do_delete`/`fire_action_with`).
    do_delete: impl Fn(bool, Option<roder_core::DeletePropagation>) + Copy + Send + Sync + 'static,
    permissions: LocalResource<SelectionPermissions>,
    on_logs: Callback<()>,
    #[prop(optional, into)] show_logs: Option<Signal<bool>>,
    #[prop(default = false)] bulk_workload: bool,
    #[prop(default = false)] bulk_job: bool,
    #[prop(optional, into)] can_rerun_jobs: Option<Signal<bool>>,
    #[prop(default = false)] bulk_flux_reconcile: bool,
    #[prop(default = false)] bulk_flux_suspend: bool,
    #[prop(default = false)] bulk_helmrelease: bool,
    #[prop(default = false)] bulk_has_source_ref: bool,
    #[prop(default = false)] bulk_certificate: bool,
    #[prop(default = false)] bulk_eso: bool,
    #[prop(default = false)] bulk_cronjob: bool,
    #[prop(default = false)] bulk_kopiur: bool,
    #[prop(default = false)] bulk_cnpg_backup: bool,
    #[prop(default = false)] bulk_cnpg_suspend: bool,
    #[prop(optional, into)] suspend_state: Option<Signal<Option<bool>>>,
) -> impl IntoView {
    let confirm = expect_context::<RwSignal<Option<Confirm>>>();
    let delete_confirm = expect_context::<RwSignal<Option<DeleteRequest>>>();
    let allowed = move |action| {
        permissions
            .get()
            .is_some_and(|permissions| permissions.allows_all(action))
    };
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

    view! {
        <div class="mobile-bulkbar-wrap" class:open=move || !selected.get().is_empty()>
            <div class="mobile-bulkbar">
                <span class="bulk-count">{move || format!("{} selected", selected.get().len())}</span>
                <button class="act" on:click=move |_| selected.set(all_uids().into_iter().collect())>"All"</button>
                <button class="act" on:click=move |_| select_mode.set(false)>"Done"</button>
                <Show when=move || show_logs.is_some_and(|show| show.get())>
                    <button class="act" disabled=move || !allowed(ResourceAction::Logs) on:click=move |_| on_logs.run(())>{move || label(ResourceAction::Logs, "Logs")}</button>
                </Show>
                {bulk_workload.then(|| view! {
                    <button class="act" disabled=move || !allowed(ResourceAction::Restart) on:click=move |_| do_bulk("restart")>{move || label(ResourceAction::Restart, "Restart")}</button>
                })}
                {bulk_job.then(|| view! {
                    <button class="act"
                        disabled=move || !can_rerun_jobs.is_some_and(|value| value.get()) || !allowed(ResourceAction::JobRerun)
                        data-tip="Only completed or failed Jobs can be re-run"
                        on:click=move |_| do_bulk("job-rerun")>{move || label(ResourceAction::JobRerun, "Re-run")}</button>
                })}
                {bulk_flux_reconcile.then(|| view! {
                    <button class="act" disabled=move || !allowed(ResourceAction::FluxReconcile) on:click=move |_| do_bulk("flux-reconcile")>{move || label(ResourceAction::FluxReconcile, "Reconcile")}</button>
                    {bulk_has_source_ref.then(|| view! {
                        <button class="act" disabled=move || !allowed(ResourceAction::FluxReconcileWithSource) on:click=move |_| do_bulk("flux-reconcile-with-source")>{move || label(ResourceAction::FluxReconcileWithSource, "Reconcile+src")}</button>
                    })}
                    {bulk_helmrelease.then(|| view! {
                        <button class="act" disabled=move || !allowed(ResourceAction::FluxForce) on:click=move |_| do_bulk("flux-force")>{move || label(ResourceAction::FluxForce, "Force")}</button>
                        <button class="act" disabled=move || !allowed(ResourceAction::FluxReset) on:click=move |_| do_bulk("flux-reset")>{move || label(ResourceAction::FluxReset, "Reset")}</button>
                    })}
                })}
                {bulk_flux_suspend.then(|| view! {
                    {move || (suspend_state.and_then(|signal| signal.get()) != Some(true)).then(|| view! {
                        <button class="act" disabled=move || !allowed(ResourceAction::FluxSuspend) on:click=move |_| do_bulk("flux-suspend")>{move || label(ResourceAction::FluxSuspend, "Suspend")}</button>
                    })}
                    {move || (suspend_state.and_then(|signal| signal.get()) != Some(false)).then(|| view! {
                        <button class="act" disabled=move || !allowed(ResourceAction::FluxSuspend) on:click=move |_| do_bulk("flux-resume")>{move || label(ResourceAction::FluxSuspend, "Resume")}</button>
                    })}
                })}
                {bulk_certificate.then(|| view! {
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
                {bulk_eso.then(|| view! {
                    <button class="act" disabled=move || !allowed(ResourceAction::ExternalSecretsRefresh) on:click=move |_| do_bulk("eso-refresh")>{move || label(ResourceAction::ExternalSecretsRefresh, "Refresh")}</button>
                })}
                {bulk_cronjob.then(|| view! {
                    <button class="act" disabled=move || !allowed(ResourceAction::CronJobTrigger) on:click=move |_| do_bulk("cronjob-trigger")>{move || label(ResourceAction::CronJobTrigger, "Trigger")}</button>
                })}
                {bulk_kopiur.then(|| view! {
                    <button class="act" disabled=move || !allowed(ResourceAction::KopiurSnapshotNow) on:click=move |_| do_bulk("kopiur-snapshot-now")>{move || label(ResourceAction::KopiurSnapshotNow, "Snapshot now")}</button>
                })}
                {bulk_cnpg_backup.then(|| view! {
                    <button class="act" disabled=move || !allowed(ResourceAction::CnpgBackup) on:click=move |_| {
                        let n = selected.get_untracked().len();
                        ask_confirm(
                            confirm,
                            format!("Create immediate Backups for {n} Clusters? Completion is reported separately."),
                            "Create backup",
                            move || do_bulk("cnpg-backup"),
                        );
                    }>{move || label(ResourceAction::CnpgBackup, "Create backup")}</button>
                })}
                {bulk_cnpg_suspend.then(|| view! {
                    {move || (suspend_state.and_then(|signal| signal.get()) != Some(true)).then(|| view! {
                        <button class="act" disabled=move || !allowed(ResourceAction::CnpgSuspend) on:click=move |_| do_bulk("cnpg-suspend")>{move || label(ResourceAction::CnpgSuspend, "Suspend")}</button>
                    })}
                    {move || (suspend_state.and_then(|signal| signal.get()) != Some(false)).then(|| view! {
                        <button class="act" disabled=move || !allowed(ResourceAction::CnpgSuspend) on:click=move |_| do_bulk("cnpg-resume")>{move || label(ResourceAction::CnpgSuspend, "Resume")}</button>
                    })}
                })}
                <button class="act danger" disabled=move || !allowed(ResourceAction::Delete) on:click=move |_| {
                    let n = selected.get_untracked().len();
                    ask_delete(delete_confirm, format!("Delete {n} resources?"), do_delete);
                }>{move || label(ResourceAction::Delete, "Delete")}</button>
            </div>
        </div>
    }
}

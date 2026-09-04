//! Watch-event application + the table's selection/action primitives.

use std::collections::HashMap;

use leptos::prelude::*;
use roder_core::{DeletePropagation, ResourceRow, WatchEvent};

use crate::app::overlays::delete::delete_extra;
use crate::app::overlays::toast::{
    show_toast, show_toast_detail, show_toast_full, show_toast_list, Toast, ToastKind,
};
use crate::app::search_state::MergedRow;
use crate::app::state::{open_logs, DetailTarget, LogTarget};
use crate::app::table_logic;
use crate::data;

pub(crate) type UidSet = RwSignal<std::collections::BTreeSet<String>>;
/// Live rows keyed by uid for O(1) per-row lookup (the table renders many rows that
/// each fetch themselves from this one signal).
pub(crate) type RowMap = RwSignal<HashMap<String, ResourceRow>>;

/// Past-tense verb shown in the toast for each action (the toast icon already
/// conveys success/failure, so these must not embed their own ✓/✕).
fn action_label(action: &str) -> &str {
    match action {
        "delete" => "Deleted",
        "evict" => "Evicted",
        "restart" => "Restarted",
        "scale" => "Scaled",
        "flux-reconcile" => "Reconciled",
        "flux-reconcile-with-source" => "Reconciled (with source)",
        "flux-force" => "Forced",
        "flux-reset" => "Reset",
        "flux-suspend" => "Suspended",
        "flux-resume" => "Resumed",
        "cordon" => "Cordoned",
        "uncordon" => "Uncordoned",
        "eso-refresh" => "Refreshed",
        "certificate-renew" => "Renewal requested for",
        "cronjob-trigger" => "Triggered",
        "job-rerun" => "Re-ran",
        "kopiur-snapshot-now" => "Snapshot triggered",
        "talos-etcd-defrag" => "Defragmented etcd on",
        other => other,
    }
}

/// Summarizes a target count for the toast title; the individual names (when
/// there's more than one) are rendered separately as a list, not joined inline.
fn describe_names(names: &[String]) -> String {
    match names {
        [] => "0 resources".to_string(),
        [n] => n.clone(),
        _ => format!("{} resources", names.len()),
    }
}

/// Fire a mutation against every target concurrently, then report one aggregated
/// toast once they've all landed (rather than one per row on bulk actions).
pub(crate) fn fire_action(
    toast: RwSignal<Option<Toast>>,
    action: &'static str,
    targets: &[DetailTarget],
) {
    fire_action_with(toast, action, targets, serde_json::Value::Null);
}

/// Like [`fire_action`] but merges extra fields into each request body (e.g. `{"replicas": 3}`).
pub(crate) fn fire_action_with(
    toast: RwSignal<Option<Toast>>,
    action: &'static str,
    targets: &[DetailTarget],
    extra: serde_json::Value,
) {
    let total = targets.len();
    if total == 0 {
        return;
    }
    let all_names: Vec<String> = targets.iter().map(|t| t.name.clone()).collect();
    let replicas = extra.get("replicas").and_then(|v| v.as_i64());

    // Shared by every target's task (wasm is single-threaded, so Rc/RefCell is fine)
    // so the last one to finish can report a single aggregated toast.
    let remaining = std::rc::Rc::new(std::cell::Cell::new(total));
    let failed_names = std::rc::Rc::new(std::cell::RefCell::new(Vec::<String>::new()));
    let last_err = std::rc::Rc::new(std::cell::RefCell::new(String::new()));

    for t in targets {
        let mut body = serde_json::json!({
            "action": action, "key": t.key, "namespace": t.namespace, "name": t.name,
        });
        if let (Some(o), Some(ex)) = (body.as_object_mut(), extra.as_object()) {
            for (k, v) in ex {
                o.insert(k.clone(), v.clone());
            }
        }
        let name = t.name.clone();
        let remaining = remaining.clone();
        let failed_names = failed_names.clone();
        let last_err = last_err.clone();
        let all_names = all_names.clone();
        leptos::task::spawn_local(async move {
            if let Err(e) = data::post_action(&body).await {
                failed_names.borrow_mut().push(name);
                *last_err.borrow_mut() = e;
            }
            remaining.set(remaining.get() - 1);
            if remaining.get() == 0 {
                let label = action_label(action);
                let failed = failed_names.borrow();
                if failed.is_empty() {
                    let target_desc = describe_names(&all_names);
                    let msg = match replicas {
                        Some(n) => format!("{label} {target_desc} to {n}"),
                        None => format!("{label} {target_desc}"),
                    };
                    if all_names.len() > 1 {
                        show_toast_list(toast, msg, all_names.clone(), ToastKind::Ok);
                    } else {
                        show_toast(toast, msg, ToastKind::Ok);
                    }
                } else if failed.len() == total {
                    show_toast_detail(
                        toast,
                        format!("{label} failed"),
                        Some(last_err.borrow().clone()),
                        ToastKind::Err,
                    );
                } else {
                    let title = format!("{label} failed for {}", describe_names(&failed));
                    let msg = last_err.borrow().clone();
                    if failed.len() > 1 {
                        show_toast_full(toast, title, failed.clone(), Some(msg), ToastKind::Err);
                    } else {
                        show_toast_detail(toast, title, Some(msg), ToastKind::Err);
                    }
                }
            }
        });
    }
}

/// Build a `do_bulk`-style dispatcher for a single-kind table/list: resolve
/// the current selection into targets via `table_logic::bulk_targets`, fire
/// `action`, then run `reset` (e.g. clearing `selected` or `select_mode`).
/// Shared by `KindTable` and the mobile single-kind resource/workspace lists.
pub(crate) fn make_do_bulk(
    toast: RwSignal<Option<Toast>>,
    key_sv: StoredValue<String>,
    rows: RowMap,
    selected: UidSet,
    reset: impl Fn() + Copy + Send + Sync + 'static,
) -> impl Fn(&'static str) + Copy + Send + Sync + 'static {
    move |action: &'static str| {
        let key = key_sv.get_value();
        let uids = selected.get_untracked();
        let targets = rows.with_untracked(|v| table_logic::bulk_targets(&key, v, &uids));
        fire_action(toast, action, &targets);
        reset();
    }
}

/// Same shape as [`make_do_bulk`] but for delete's force/propagation options.
pub(crate) fn make_do_delete(
    toast: RwSignal<Option<Toast>>,
    key_sv: StoredValue<String>,
    rows: RowMap,
    selected: UidSet,
    reset: impl Fn() + Copy + Send + Sync + 'static,
) -> impl Fn(bool, Option<DeletePropagation>) + Copy + Send + Sync + 'static {
    move |force, propagation| {
        let key = key_sv.get_value();
        let uids = selected.get_untracked();
        let targets = rows.with_untracked(|v| table_logic::bulk_targets(&key, v, &uids));
        fire_action_with(toast, "delete", &targets, delete_extra(force, propagation));
        reset();
    }
}

/// [`make_do_delete`]'s counterpart for the mixed-kind search views, whose
/// rows live in a `MergedRow` map keyed by uid rather than a single-kind
/// `RowMap` sharing one `key`.
pub(crate) fn make_do_delete_multi(
    toast: RwSignal<Option<Toast>>,
    merged_rows: RwSignal<HashMap<String, MergedRow>>,
    selected: UidSet,
    reset: impl Fn() + Copy + Send + Sync + 'static,
) -> impl Fn(bool, Option<DeletePropagation>) + Copy + Send + Sync + 'static {
    move |force, propagation| {
        let uids = selected.get_untracked();
        let targets: Vec<DetailTarget> = merged_rows.with_untracked(|m| {
            uids.iter()
                .filter_map(|uid| {
                    m.get(uid).map(|mr| DetailTarget {
                        key: mr.kind.key.clone(),
                        namespace: mr.row.namespace.clone(),
                        name: mr.row.name.clone(),
                    })
                })
                .collect()
        });
        fire_action_with(toast, "delete", &targets, delete_extra(force, propagation));
        reset();
    }
}

/// Open logs for the current bulk selection of a single-kind list, then run
/// `reset`. Shared by `KindTable` and the mobile single-kind lists.
pub(crate) fn make_bulk_open_logs(
    log_pods: RwSignal<Vec<LogTarget>>,
    key_sv: StoredValue<String>,
    rows: RowMap,
    selected: UidSet,
    is_pod_kind: bool,
    reset: impl Fn() + Copy + Send + Sync + 'static,
) -> impl Fn() + Copy + Send + Sync + 'static {
    move || {
        let uids = selected.get_untracked();
        let key = key_sv.get_value();
        let agg = !is_pod_kind;
        rows.with_untracked(|v| {
            for r in v.values().filter(|r| uids.contains(&r.uid)) {
                open_logs(log_pods, LogTarget::from_row(&key, r, agg));
            }
        });
        reset();
    }
}

/// Shift-click range selection: select every row between the anchor (last
/// individually-toggled row) and `cur`, in the currently displayed order.
pub(crate) fn range_select(
    selected: UidSet,
    last_clicked: RwSignal<Option<String>>,
    order: Memo<Vec<String>>,
    cur: &str,
) {
    let order = order.get_untracked();
    if let Some(anchor) = last_clicked.get_untracked() {
        if let (Some(i), Some(j)) = (
            order.iter().position(|x| *x == anchor),
            order.iter().position(|x| x == cur),
        ) {
            let (lo, hi) = if i <= j { (i, j) } else { (j, i) };
            selected.update(|s| {
                for u in &order[lo..=hi] {
                    s.insert(u.clone());
                }
            });
        }
    } else {
        selected.update(|s| {
            s.insert(cur.to_string());
        });
    }
    last_clicked.set(Some(cur.to_string()));
}

/// Apply a live `WatchEvent` to the table's row buffer, tagging uids for the
/// enter/exit animations. `columns`, when provided, receives the snapshot's
/// current column headers so a table reflows its columns live (CRD changes push
/// a fresh snapshot with new headers + cells together).
pub(crate) fn apply_event(
    rows: RowMap,
    entering: UidSet,
    removing: UidSet,
    columns: Option<RwSignal<Vec<String>>>,
    toast: RwSignal<Option<Toast>>,
    ev: WatchEvent,
) {
    match ev {
        WatchEvent::Snapshot {
            columns: cols,
            rows: r,
        } => {
            if let Some(sig) = columns {
                if sig.get_untracked() != cols {
                    sig.set(cols);
                }
            }
            entering.set(std::collections::BTreeSet::new());
            removing.set(std::collections::BTreeSet::new());
            rows.set(r.into_iter().map(|row| (row.uid.clone(), row)).collect());
        }
        WatchEvent::Applied { row } => {
            let uid = row.uid.clone();
            let is_new = rows.with_untracked(|m| !m.contains_key(&uid));
            rows.update(|m| {
                m.insert(row.uid.clone(), row);
            });
            // Always clear from removing: the object exists again regardless of
            // whether it's truly new. This handles delete-then-recreate with the
            // same uid (e.g. objects whose uid falls back to namespace/name) so
            // the safety-net timeout below doesn't evict the newly-created row.
            removing.update(|s| {
                s.remove(&uid);
            });
            // New rows animate in (briefly tagged, then untagged so the flash works again).
            if is_new {
                entering.update(|s| {
                    s.insert(uid.clone());
                });
                set_timeout(
                    move || {
                        let _ = entering.try_update(|s| {
                            s.remove(&uid);
                        });
                    },
                    std::time::Duration::from_millis(280),
                );
            }
        }
        WatchEvent::Deleted { uid } => {
            // Tag for the collapse transition. The actual unmount is driven by the
            // row's `transitionend` handler (see the table view) once the CSS
            // collapse has *visually* finished — so it can never be cut off early.
            removing.update(|s| {
                s.insert(uid.clone());
            });
            // Safety net: if `transitionend` never fires (row was reordered, never
            // laid out, etc.), force the removal after the transition would have
            // completed so rows can't get stuck collapsed.
            // Only act if uid is still in removing — an Applied event for a
            // same-uid recreated object clears it, preventing this timeout from
            // silently deleting the new row.
            let uid2 = uid.clone();
            set_timeout(
                move || {
                    if removing
                        .try_with_untracked(|s| s.contains(&uid))
                        .unwrap_or(false)
                    {
                        let _ = rows.try_update(|m| {
                            m.remove(&uid);
                        });
                        let _ = removing.try_update(|s| {
                            s.remove(&uid2);
                        });
                    }
                },
                std::time::Duration::from_millis(500),
            );
        }
        WatchEvent::Forbidden { message } => {
            // Forbidden streams do not reconnect, so retain the last snapshot
            // and leave a diagnostic in the browser console.
            leptos::logging::warn!("watch forbidden: {message}");
        }
        WatchEvent::Error { message } => {
            leptos::logging::error!("watch failed: {message}");
            show_toast_detail(
                toast,
                "Resource watch failed",
                Some(message),
                ToastKind::Err,
            );
        }
    }
}

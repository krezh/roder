//! Watch-event application + the table's selection/action primitives.

use std::collections::HashMap;

use leptos::prelude::*;
use roder_core::{ResourceRow, WatchEvent};

use crate::app::state::DetailTarget;
use crate::data;

pub(crate) type UidSet = RwSignal<std::collections::BTreeSet<String>>;
/// Live rows keyed by uid for O(1) per-row lookup (the table renders many rows that
/// each fetch themselves from this one signal).
pub(crate) type RowMap = RwSignal<HashMap<String, ResourceRow>>;

/// Fire-and-forget mutation (context menu / bulk actions).
pub(crate) fn fire_action(action: &'static str, t: &DetailTarget) {
    let body = serde_json::json!({
        "action": action, "key": t.key, "namespace": t.namespace, "name": t.name,
    });
    leptos::task::spawn_local(async move {
        let _ = data::post_action(&body).await;
    });
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
/// enter/exit animations.
pub(crate) fn apply_event(rows: RowMap, entering: UidSet, removing: UidSet, ev: WatchEvent) {
    match ev {
        WatchEvent::Snapshot { rows: r } => {
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
                        entering.update(|s| {
                            s.remove(&uid);
                        })
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
                    if removing.with_untracked(|s| s.contains(&uid)) {
                        rows.update(|m| {
                            m.remove(&uid);
                        });
                        removing.update(|s| {
                            s.remove(&uid2);
                        });
                    }
                },
                std::time::Duration::from_millis(500),
            );
        }
    }
}

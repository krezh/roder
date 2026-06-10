//! Shared UI state: the detail/context targets, the table sort key, and the
//! newtype-wrapped context signals (so multiple same-typed signals can coexist in
//! Leptos context).

use leptos::prelude::*;
use roder_core::ResourceKind;
use serde::{Deserialize, Serialize};

/// The object currently shown in the detail drawer.
#[derive(Clone, PartialEq, Eq)]
pub struct DetailTarget {
    pub key: String,
    pub namespace: Option<String>,
    pub name: String,
}

/// Right-click context menu state: where to draw it and what it acts on.
#[derive(Clone)]
pub(crate) struct CtxMenu {
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) target: DetailTarget,
    pub(crate) node: Option<String>,
}

/// Which column the resource table is sorted by.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum SortKey {
    Namespace,
    Name,
    Cell(usize),
    Age,
}

/// Increments once a second so relative ages can recompute live.
#[derive(Clone, Copy)]
pub(crate) struct Tick(pub(crate) RwSignal<u32>);
/// Quick filter (k9s ⌃Z): show only rows in a Warn/Error state.
#[derive(Clone, Copy)]
pub(crate) struct OnlyProblems(pub(crate) RwSignal<bool>);
#[derive(Clone, Copy)]
pub(crate) struct NavOpen(pub(crate) RwSignal<bool>);
#[derive(Clone, Copy)]
pub(crate) struct PaletteOpen(pub(crate) RwSignal<bool>);
#[derive(Clone, Copy)]
pub(crate) struct Catalog(pub(crate) RwSignal<Vec<ResourceKind>>);
/// A pod to show in the centered pod-info modal (opened from a workload's Pods tab).
#[derive(Clone, Copy)]
pub(crate) struct PodModalTarget(pub(crate) RwSignal<Option<DetailTarget>>);

/// Whether the keyboard shortcuts help overlay is open.
#[derive(Clone, Copy)]
pub(crate) struct ShortcutsOpen(pub(crate) RwSignal<bool>);

/// Whether the SSE data stream is currently live (`true`) or reconnecting (`false`).
#[derive(Clone, Copy)]
pub(crate) struct ConnectionState(pub(crate) RwSignal<bool>);

/// Global text filter for resource search (set by command palette).
#[derive(Clone)]
pub(crate) struct ResourceFilter(pub(crate) RwSignal<String>);

/// Multi-kind search query (stored in session storage for the search results view).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MultiKindSearch {
    pub kinds: Vec<String>,
    pub namespaces: Vec<String>,
    pub text: String,
}

/// A source open in the log sidebar: a single pod, or a workload whose pods'
/// logs are merged into one panel.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct LogTarget {
    pub(crate) key: String,
    pub(crate) namespace: String,
    pub(crate) name: String,
    /// true → aggregate every pod of this workload into one stream.
    pub(crate) aggregate: bool,
}
impl LogTarget {
    pub(crate) fn url(&self) -> String {
        let ns = crate::data::percent_encode(&self.namespace);
        let name = crate::data::percent_encode(&self.name);
        if self.aggregate {
            format!("/api/logs?key={}&namespace={}&name={}", self.key, ns, name)
        } else {
            format!("/api/logs?namespace={}&pod={}", ns, name)
        }
    }
}

/// Sources whose logs are open in the right-hand log sidebar (one pane each).
#[derive(Clone, Copy)]
pub(crate) struct LogPods(pub(crate) RwSignal<Vec<LogTarget>>);

/// Add a source to the log sidebar (no duplicates).
pub(crate) fn open_logs(log_pods: RwSignal<Vec<LogTarget>>, t: LogTarget) {
    log_pods.update(|v| {
        if !v.iter().any(|x| x == &t) {
            v.push(t);
        }
    });
}

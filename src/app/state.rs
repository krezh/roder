//! Shared UI state: the detail/context targets, the table sort key, and the
//! newtype-wrapped context signals (so multiple same-typed signals can coexist in
//! Leptos context).

use std::collections::{BTreeSet, HashMap, HashSet};

use leptos::prelude::*;
use roder_core::{FiringAlert, ResourceKind, ResourceRow};
use serde::{Deserialize, Serialize};

/// The object currently shown in the detail drawer.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    /// uid of the right-clicked row — used to detect multi-select context.
    pub(crate) uid: String,
}

/// Provided at App level; `ResourceView` writes the actual signal into it on
/// mount so that `ContextMenu` (a sibling, not a child) can read multi-select
/// state for bulk context-menu actions.
#[derive(Clone, Copy)]
pub(crate) struct TableSelected(pub(crate) StoredValue<Option<RwSignal<BTreeSet<String>>>>);

#[derive(Clone, Copy)]
pub(crate) struct TableRows(pub(crate) StoredValue<Option<RwSignal<HashMap<String, ResourceRow>>>>);

/// Per-row action targets for tables where rows can have different kinds.
#[derive(Clone, Copy)]
pub(crate) struct TableTargets(
    pub(crate) StoredValue<Option<RwSignal<HashMap<String, DetailTarget>>>>,
);

/// Which column the resource table is sorted by.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum SortKey {
    Namespace,
    Name,
    Cell(usize),
    Age,
}

/// Increments once a second so relative ages can recompute live.
#[derive(Clone, Copy)]
pub(crate) struct Tick(pub(crate) RwSignal<u32>);
/// Quick filter (⌃Z): show only rows in a Warn/Error state.
#[derive(Clone, Copy)]
pub(crate) struct OnlyProblems(pub(crate) RwSignal<bool>);
#[derive(Clone, Copy)]
pub(crate) struct NavOpen(pub(crate) RwSignal<bool>);
#[derive(Clone, Copy)]
pub(crate) struct NavigationRestored(pub(crate) RwSignal<bool>);
#[derive(Clone, Copy)]
pub(crate) struct PaletteOpen(pub(crate) RwSignal<bool>);
#[derive(Clone, Copy)]
pub(crate) struct NsPaletteOpen(pub(crate) RwSignal<bool>);
#[derive(Clone, Copy)]
pub(crate) struct Catalog(pub(crate) RwSignal<Vec<ResourceKind>>);

/// Kind keys the user has pinned, shown as the sidebar's "Favorites" section
/// and reachable with `Ctrl+1`..`Ctrl+0`.
///
/// Lives at App level rather than inside `Sidebar` because the key dispatcher
/// needs the same set; [`pinned_in_catalog_order`] resolves it to the order the
/// sidebar actually renders, which is what the number keys have to agree with.
#[derive(Clone, Copy)]
pub(crate) struct PinnedKinds(pub(crate) RwSignal<HashSet<String>>);

/// The pinned kinds in the order the sidebar lists them.
///
/// The set is persisted sorted by key, but rendered by walking the catalog and
/// keeping pinned entries — so catalog order, not key order, is what the user
/// sees and therefore what `Ctrl+<n>` must count.
pub(crate) fn pinned_in_catalog_order(
    catalog: &[ResourceKind],
    pinned: &HashSet<String>,
) -> Vec<ResourceKind> {
    catalog
        .iter()
        .filter(|k| pinned.contains(&k.key))
        .cloned()
        .collect()
}
/// A pod to show in the centered pod-info modal (opened from a workload's Pods tab).
#[derive(Clone, Copy)]
pub(crate) struct PodModalTarget(pub(crate) RwSignal<Option<DetailTarget>>);

/// Target for the exec/shell overlay: which pod (and optionally container) to exec into.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ExecTarget {
    pub(crate) namespace: String,
    pub(crate) pod: String,
    pub(crate) container: Option<String>,
    /// True while an ephemeral debug container is being injected and waited on.
    pub(crate) pending: bool,
    /// True for a node-shell session: `pod` is a throw-away privileged pod
    /// entered via `nsenter`, torn down when the session ends.
    pub(crate) node_shell: bool,
    /// Resolved debug-image reference (for the loading text). Empty when a
    /// session already existed (e.g. exec into a real container).
    pub(crate) image: String,
}

#[derive(Clone, Copy)]
pub(crate) struct ExecOpen(pub(crate) RwSignal<Option<ExecTarget>>);

#[derive(Clone, Copy)]
pub(crate) struct FileBrowserOpen(pub(crate) RwSignal<Option<DetailTarget>>);

/// Resolved debug-image reference, populated from `/api/features` at startup
/// so the exec overlay can show the actual image during the spinner.
#[derive(Clone, Copy)]
pub(crate) struct DebugImage(pub(crate) RwSignal<String>);

/// Talos permissions returned by `/api/features` for the current user.
#[derive(Clone, Copy)]
pub(crate) struct TalosFeatures(pub(crate) RwSignal<roder_core::TalosCapabilities>);

/// Which Kustomization/HelmRelease the resource-tree overlay is open for.
#[derive(Clone, Copy)]
pub(crate) struct TreeOpen(pub(crate) RwSignal<Option<DetailTarget>>);

/// Target for the drain overlay: a node to drain, optionally as the first
/// phase of a Talos power action (`power`), in which case `control_plane`
/// drives the etcd-quorum warning shown in the dialog.
#[derive(Clone, PartialEq)]
pub(crate) struct DrainTarget {
    /// Node kind key (used to build the plain `"drain"` action payload;
    /// unused when `power` is set — the server resolves the node itself).
    pub(crate) key: String,
    pub(crate) name: String,
    /// "reboot" | "shutdown" when this drain is the first phase of a Talos
    /// power action; `None` for a bare drain (context menu).
    pub(crate) power: Option<String>,
    pub(crate) control_plane: bool,
    /// Existing job to reopen after a browser refresh; `None` starts at options.
    pub(crate) job: Option<roder_core::DrainJobRef>,
}

#[derive(Clone, Copy)]
pub(crate) struct DrainOpen(pub(crate) RwSignal<Option<DrainTarget>>);

/// Whether the keyboard shortcuts help overlay is open.
#[derive(Clone, Copy)]
pub(crate) struct ShortcutsOpen(pub(crate) RwSignal<bool>);

/// Whether the alerts panel overlay is open.
#[derive(Clone, Copy)]
pub(crate) struct AlertsOpen(pub(crate) RwSignal<bool>);

/// Whether the RBAC access-review overlay is open.
#[derive(Clone, Copy)]
pub(crate) struct AccessReviewOpen(pub(crate) RwSignal<bool>);

/// Cached list of firing alerts from AlertManager (None = not yet fetched).
#[derive(Clone, Copy)]
pub(crate) struct AlertsData(pub(crate) RwSignal<Option<Vec<FiringAlert>>>);

/// Browser time of the last successful alerts API response, in milliseconds.
#[derive(Clone, Copy)]
pub(crate) struct AlertsLastRefresh(pub(crate) RwSignal<Option<f64>>);

#[derive(Clone, Copy)]
pub(crate) struct AlertSilencesEnabled(pub(crate) RwSignal<bool>);

/// `None` = SSE stream is live. `Some(msg)` = disconnected, with the HTTP status
/// or network error that caused it (e.g. "401 Unauthorized", "Network error").
#[derive(Clone, PartialEq, Eq)]
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) enum Connectivity {
    Checking,
    Connected,
    Offline,
    Error(String),
}

impl Connectivity {
    pub(crate) fn message(&self) -> &str {
        match self {
            Self::Checking => "Checking cluster connection...",
            Self::Connected => "Cluster connected",
            Self::Offline => "Browser is offline",
            Self::Error(message) => message,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct ConnectionState(pub(crate) RwSignal<Connectivity>);

/// Global text filter for resource search (used by search view).
#[derive(Clone)]
pub(crate) struct ResourceFilter(pub(crate) RwSignal<String>);

/// Incrementing token; pressing `/` bumps this so KindTable can focus its filter input.
#[derive(Clone, Copy)]
pub(crate) struct FilterFocus(pub(crate) RwSignal<u32>);

/// Multi-kind search query (stored in session storage for the search results view).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MultiKindSearch {
    pub kinds: Vec<String>,
    pub namespaces: Vec<String>,
    pub text: String,
    /// Pre-computed Kubernetes label selector (e.g. `"app=notifier,env=prod"`).
    /// Passed to each SSE watch URL so the API server filters server-side.
    #[serde(default)]
    pub selector: Option<String>,
}

/// A single live-watching pane in the workspace view.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneConfig {
    pub kind_key: String,
    pub namespace: Option<String>,
    /// Kubernetes label selector applied server-side via the SSE watch URL.
    pub selector: Option<String>,
}

/// Persistent set of live-watching panes shown on the `/workspace` route.
#[derive(Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct WorkspaceConfig {
    pub panes: Vec<PaneConfig>,
}

/// App-level signal for the workspace configuration (persisted to localStorage).
#[derive(Clone, Copy)]
pub(crate) struct WorkspaceConf(pub(crate) RwSignal<WorkspaceConfig>);

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
    pub(crate) fn from_row(key: &str, row: &ResourceRow, aggregate: bool) -> Self {
        Self {
            key: key.to_string(),
            namespace: row.namespace.clone().unwrap_or_default(),
            name: row.name.clone(),
            aggregate,
        }
    }

    pub(crate) fn from_detail(target: &DetailTarget, aggregate: bool) -> Self {
        Self {
            key: target.key.clone(),
            namespace: target.namespace.clone().unwrap_or_default(),
            name: target.name.clone(),
            aggregate,
        }
    }

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

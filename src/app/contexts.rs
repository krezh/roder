//! The signal bundles `App` owns and hands to the tree.
//!
//! These exist so `App` doesn't have to spell out four dozen
//! `let x = RwSignal::new(..); provide_context(Wrapper(x));` pairs, and — more
//! importantly — so the facts that span *all* of them have one home. Adding an
//! overlay means adding a field to [`Overlays`]; [`Overlays::any_open`] then
//! accounts for it automatically, instead of the keyboard layer silently
//! ignoring an overlay somebody forgot to add to a hand-written `||` chain.

use leptos::prelude::*;

use crate::app::state::{
    AccessReviewOpen, AlertSilencesEnabled, AlertsData, AlertsLastRefresh, AlertsOpen,
    DetailTarget, DrainOpen, DrainTarget, ExecOpen, ExecTarget, FileBrowserOpen, NsPaletteOpen,
    PaletteOpen, PodModalTarget, RecommendData, RecommendEnabled, RecommendError, RecommendOpen,
    RecommendScanning, ShortcutsOpen, TableRows, TableSelected, TableTargets, TreeOpen,
};
use crate::app::ui::confirm::Confirm;
use crate::app::ui::delete::DeleteRequest;
use crate::app::ui::sweep::SweepRequest;

/// Every overlay that takes the screen, and therefore the keyboard.
///
/// Membership here is exactly the set that puts the key dispatcher into
/// `Layer::Overlay`: if opening it should stop the table from acting on
/// keystrokes, it belongs in this struct.
#[derive(Clone, Copy)]
pub(crate) struct Overlays {
    pub(crate) palette: RwSignal<bool>,
    pub(crate) ns_palette: RwSignal<bool>,
    pub(crate) shortcuts: RwSignal<bool>,
    pub(crate) alerts: RwSignal<bool>,
    pub(crate) recommend: RwSignal<bool>,
    pub(crate) access_review: RwSignal<bool>,
    pub(crate) exec: RwSignal<Option<ExecTarget>>,
    pub(crate) file_browser: RwSignal<Option<DetailTarget>>,
    pub(crate) tree: RwSignal<Option<DetailTarget>>,
    pub(crate) drain: RwSignal<Option<DrainTarget>>,
    pub(crate) pod_modal: RwSignal<Option<DetailTarget>>,
    pub(crate) confirm: RwSignal<Option<Confirm>>,
    pub(crate) delete: RwSignal<Option<DeleteRequest>>,
    pub(crate) sweep: RwSignal<Option<SweepRequest>>,
}

impl Overlays {
    pub(crate) fn new() -> Self {
        Self {
            palette: RwSignal::new(false),
            ns_palette: RwSignal::new(false),
            shortcuts: RwSignal::new(false),
            alerts: RwSignal::new(false),
            recommend: RwSignal::new(false),
            access_review: RwSignal::new(false),
            exec: RwSignal::new(None),
            file_browser: RwSignal::new(None),
            tree: RwSignal::new(None),
            drain: RwSignal::new(None),
            pod_modal: RwSignal::new(None),
            confirm: RwSignal::new(None),
            delete: RwSignal::new(None),
            sweep: RwSignal::new(None),
        }
    }

    /// Whether anything currently has the screen. Reactive — every field is
    /// read, so the keyboard layer re-derives when any overlay opens or closes.
    pub(crate) fn any_open(&self) -> bool {
        self.palette.get()
            || self.ns_palette.get()
            || self.shortcuts.get()
            || self.alerts.get()
            || self.recommend.get()
            || self.access_review.get()
            || self.exec.with(Option::is_some)
            || self.file_browser.with(Option::is_some)
            || self.tree.with(Option::is_some)
            || self.drain.with(Option::is_some)
            || self.pod_modal.with(Option::is_some)
            || self.confirm.with(Option::is_some)
            || self.delete.with(Option::is_some)
            || self.sweep.with(Option::is_some)
    }

    /// Publish each signal under the newtype its consumers expect.
    pub(crate) fn provide(self) {
        provide_context(PaletteOpen(self.palette));
        provide_context(NsPaletteOpen(self.ns_palette));
        provide_context(ShortcutsOpen(self.shortcuts));
        provide_context(AlertsOpen(self.alerts));
        provide_context(RecommendOpen(self.recommend));
        provide_context(AccessReviewOpen(self.access_review));
        provide_context(ExecOpen(self.exec));
        provide_context(FileBrowserOpen(self.file_browser));
        provide_context(TreeOpen(self.tree));
        provide_context(DrainOpen(self.drain));
        provide_context(PodModalTarget(self.pod_modal));
        // These three are read as bare signals rather than newtypes, because
        // their payload type already identifies them unambiguously.
        provide_context(self.confirm);
        provide_context(self.delete);
        provide_context(self.sweep);
    }
}

/// The alert panel's shared state: the last fetch, when it happened, and what
/// this deployment's Alertmanager integration permits.
#[derive(Clone, Copy)]
pub(crate) struct Alerts {
    pub(crate) data: RwSignal<Option<Vec<roder_core::FiringAlert>>>,
    pub(crate) last_refresh: RwSignal<Option<f64>>,
    pub(crate) silences_enabled: RwSignal<bool>,
    /// Whether the server has Alertmanager configured at all. Read by `App`'s
    /// own poll loop to decide whether to fetch, and deliberately not provided
    /// as a context — no child needs it. That poll loop is wasm-only, so on the
    /// SSR build the field is genuinely never read.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    pub(crate) enabled: RwSignal<bool>,
}

impl Alerts {
    pub(crate) fn new() -> Self {
        Self {
            data: RwSignal::new(None),
            last_refresh: RwSignal::new(None),
            silences_enabled: RwSignal::new(false),
            enabled: RwSignal::new(false),
        }
    }

    pub(crate) fn provide(self) {
        provide_context(AlertsData(self.data));
        provide_context(AlertsLastRefresh(self.last_refresh));
        provide_context(AlertSilencesEnabled(self.silences_enabled));
    }
}

/// The resource scan's shared state: the last report, whether one is running,
/// and whether this deployment has Prometheus at all.
#[derive(Clone, Copy)]
pub(crate) struct Recommendations {
    pub(crate) data: RwSignal<Option<roder_core::ResourceScan>>,
    pub(crate) scanning: RwSignal<bool>,
    pub(crate) error: RwSignal<Option<String>>,
    /// Whether the server has Prometheus configured. Gates the topbar button,
    /// the same way `Alerts::enabled` gates the alerts one.
    pub(crate) enabled: RwSignal<bool>,
}

impl Recommendations {
    pub(crate) fn new() -> Self {
        Self {
            data: RwSignal::new(None),
            scanning: RwSignal::new(false),
            error: RwSignal::new(None),
            enabled: RwSignal::new(false),
        }
    }

    pub(crate) fn provide(self) {
        provide_context(RecommendData(self.data));
        provide_context(RecommendScanning(self.scanning));
        provide_context(RecommendError(self.error));
        provide_context(RecommendEnabled(self.enabled));
    }
}

/// Slots the currently-mounted table publishes itself into, so its siblings —
/// the context menu and the key dispatcher — can reach the live selection.
/// Empty until a table registers, and cleared again when it unmounts.
pub(crate) fn provide_table_handles() {
    provide_context(TableSelected(StoredValue::new(None)));
    provide_context(TableRows(StoredValue::new(None)));
    provide_context(TableTargets(StoredValue::new(None)));
    provide_context(crate::app::keys::TableKeys(StoredValue::new(None)));
}

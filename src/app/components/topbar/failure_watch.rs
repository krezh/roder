//! One app-owned stream for pod and Flux failure badges.

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::app::events::{apply_event, RowMap, UidSet};
use crate::app::overlays::toast::Toast;
use crate::app::state::{Catalog, ConnectionState, Connectivity};
use crate::data;

#[derive(Clone, Copy)]
pub(crate) struct FailureWatchRows {
    pub(crate) pods: RowMap,
    pub(crate) kustomizations: RowMap,
    pub(crate) helm_releases: RowMap,
}

pub(crate) fn use_failure_watch() {
    let catalog = expect_context::<Catalog>().0;
    let connection = expect_context::<ConnectionState>().0;
    let toast = expect_context::<RwSignal<Option<Toast>>>();
    let rows = FailureWatchRows {
        pods: RwSignal::new(Default::default()),
        kustomizations: RwSignal::new(Default::default()),
        helm_releases: RwSignal::new(Default::default()),
    };
    provide_context(rows);

    let pod_entering: UidSet = RwSignal::new(Default::default());
    let pod_removing: UidSet = RwSignal::new(Default::default());
    let ks_entering: UidSet = RwSignal::new(Default::default());
    let ks_removing: UidSet = RwSignal::new(Default::default());
    let hr_entering: UidSet = RwSignal::new(Default::default());
    let hr_removing: UidSet = RwSignal::new(Default::default());
    let reconnect = RwSignal::new(0u32);

    Effect::new(move |_previous: Option<Option<data::SseHandle>>| {
        reconnect.track();
        let kinds = catalog.get();
        let pod_key = kinds
            .iter()
            .find(|kind| kind.group.is_empty() && kind.kind == "Pod")
            .map(|kind| kind.key.clone());
        let ks_key = kinds
            .iter()
            .find(|kind| kind.group.ends_with("fluxcd.io") && kind.kind == "Kustomization")
            .map(|kind| kind.key.clone());
        let hr_key = kinds
            .iter()
            .find(|kind| kind.group.ends_with("fluxcd.io") && kind.kind == "HelmRelease")
            .map(|kind| kind.key.clone());
        let panes: Vec<_> = [pod_key.as_deref(), ks_key.as_deref(), hr_key.as_deref()]
            .into_iter()
            .flatten()
            .map(|key| (key, None))
            .collect();
        if panes.is_empty() {
            return None;
        }

        rows.pods.set(Default::default());
        rows.kustomizations.set(Default::default());
        rows.helm_releases.set(Default::default());
        let url = data::watch_multi_url(&panes);
        let probe_url = url.clone();
        data::subscribe_multi(
            &url,
            move |key, event| {
                if pod_key.as_deref() == Some(&key) {
                    apply_event(rows.pods, pod_entering, pod_removing, None, toast, event);
                } else if ks_key.as_deref() == Some(&key) {
                    apply_event(
                        rows.kustomizations,
                        ks_entering,
                        ks_removing,
                        None,
                        toast,
                        event,
                    );
                } else if hr_key.as_deref() == Some(&key) {
                    apply_event(
                        rows.helm_releases,
                        hr_entering,
                        hr_removing,
                        None,
                        toast,
                        event,
                    );
                }
            },
            move || {
                let url = probe_url.clone();
                spawn_local(async move {
                    connection.set(Connectivity::Error(data::probe_error(url).await));
                });
                set_timeout(
                    move || {
                        let _ = reconnect.try_update(|attempt| *attempt = attempt.wrapping_add(1));
                    },
                    data::reconnect_delay(),
                );
            },
        )
    });
}

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::app::events::{apply_event, RowMap};
use crate::app::state::{Catalog, ConnectionState, Connectivity};
use crate::app::ui::Toast;
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
    let entering = [
        RwSignal::new(Default::default()),
        RwSignal::new(Default::default()),
        RwSignal::new(Default::default()),
    ];
    let removing = [
        RwSignal::new(Default::default()),
        RwSignal::new(Default::default()),
        RwSignal::new(Default::default()),
    ];
    let reconnect = RwSignal::new(0u32);
    Effect::new(move |_previous: Option<Option<data::SseHandle>>| {
        reconnect.track();
        let catalog = catalog.get();
        let keys = [
            catalog
                .iter()
                .find(|kind| kind.group.is_empty() && kind.kind == "Pod")
                .map(|kind| kind.key.clone()),
            catalog
                .iter()
                .find(|kind| kind.group.ends_with("fluxcd.io") && kind.kind == "Kustomization")
                .map(|kind| kind.key.clone()),
            catalog
                .iter()
                .find(|kind| kind.group.ends_with("fluxcd.io") && kind.kind == "HelmRelease")
                .map(|kind| kind.key.clone()),
        ];
        let panes: Vec<_> = keys
            .iter()
            .flatten()
            .map(|key| (key.as_str(), None))
            .collect();
        if panes.is_empty() {
            return None;
        }
        rows.pods.set(Default::default());
        rows.kustomizations.set(Default::default());
        rows.helm_releases.set(Default::default());
        let url = data::watch_multi_url(&panes);
        let probe = url.clone();
        data::subscribe_multi(
            &url,
            move |key, event| {
                let target = if keys[0].as_deref() == Some(&key) {
                    Some((rows.pods, entering[0], removing[0]))
                } else if keys[1].as_deref() == Some(&key) {
                    Some((rows.kustomizations, entering[1], removing[1]))
                } else if keys[2].as_deref() == Some(&key) {
                    Some((rows.helm_releases, entering[2], removing[2]))
                } else {
                    None
                };
                if let Some((rows, entering, removing)) = target {
                    apply_event(rows, entering, removing, None, toast, event);
                }
            },
            move || {
                let url = probe.clone();
                spawn_local(async move {
                    connection.set(Connectivity::Error(data::probe_error(url).await));
                });
                set_timeout(
                    move || {
                        let _ = reconnect.try_update(|value| *value = value.wrapping_add(1));
                    },
                    data::reconnect_delay(),
                );
            },
        )
    });
}

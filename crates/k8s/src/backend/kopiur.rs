//! kopiur-specific mutations: triggering a manual snapshot from a
//! `SnapshotPolicy` (`kopiur.home-operations.com`).

use kube::api::{DynamicObject, PostParams};
use roder_core::ResourceKind;
use serde_json::json;

use super::{api_err, Backend};
use crate::client::K8sError;

/// kopiur's manual-snapshot labels, matching what a `SnapshotSchedule` stamps
/// on its own children and what `kubectl kopiur snapshot now` creates.
const ORIGIN_LABEL: &str = "kopiur.home-operations.com/origin";
const CONFIG_LABEL: &str = "kopiur.home-operations.com/config";

impl Backend {
    /// `kubectl kopiur snapshot now`: run a `SnapshotPolicy` immediately by
    /// creating a manual `Snapshot` CR referencing it, the same thing the
    /// kopiur CLI does. The `Snapshot` kind lives in the same API group/version
    /// as the `SnapshotPolicy` (`key`), so it's derived from the policy's own
    /// catalog entry rather than looked up by kind name alone — "Snapshot" is
    /// not unique across a cluster's installed CRDs.
    pub async fn kopiur_snapshot_now(
        &self,
        key: &str,
        ns: Option<&str>,
        name: &str,
    ) -> Result<(), K8sError> {
        let policy = self.entry(key)?;
        let snapshot_key =
            ResourceKind::make_key(&policy.kind.group, &policy.kind.version, "Snapshot");
        let ts = time::OffsetDateTime::now_utc().unix_timestamp();
        let snapshot_name = format!("{name}-manual-{ts}");

        let snapshot = json!({
            "apiVersion": format!("{}/{}", policy.kind.group, policy.kind.version),
            "kind": "Snapshot",
            "metadata": {
                "name": snapshot_name,
                "namespace": ns,
                "labels": {
                    ORIGIN_LABEL: "manual",
                    CONFIG_LABEL: name,
                }
            },
            "spec": {
                "policyRef": { "name": name }
            }
        });
        let obj: DynamicObject = serde_json::from_value(snapshot).map_err(api_err)?;
        self.dyn_api(&snapshot_key, ns)?
            .create(&PostParams::default(), &obj)
            .await
            .map_err(api_err)?;
        Ok(())
    }
}

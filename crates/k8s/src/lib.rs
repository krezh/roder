//! Server-side Kubernetes layer for roder.
//!
//! [`ClusterAccess`] owns the token-passthrough client, while [`Backend`]
//! combines discovery, shared informers, row projection, metrics, object
//! detail, and mutation executors.

pub mod alertmanager;
mod backend;
mod client;
mod coordination;
mod discovery;
mod informers;
mod metrics;
mod project;
mod shared;
#[allow(dead_code)]
mod table;

#[cfg(test)]
mod test_alloc;

pub use alertmanager::{alertmanager_url, AlertsCache, SilenceError};
pub use backend::{normalize_file_path, Backend, DrainSession};
pub use client::{ClusterAccess, K8sError};
pub use coordination::{
    AcquireError, HeldNodeLease, NodeCoordinator, RoderPod, TLS_FINGERPRINT_ANNOTATION,
};
pub use informers::WatchHandle;
pub use kube::api::{AttachedProcess, TerminalSize};
pub use shared::SharedCluster;

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
mod printer_columns;
mod project;
mod shared;

#[cfg(test)]
mod test_alloc;

pub use alertmanager::{discover_alertmanager, AlertsCache};
pub use backend::{Backend, DrainSession};
pub use client::{ClusterAccess, K8sError};
pub use coordination::{AcquireError, HeldNodeLease, NodeCoordinator, RoderPod};
pub use informers::WatchHandle;
pub use kube::api::{AttachedProcess, TerminalSize};
pub use shared::SharedCluster;

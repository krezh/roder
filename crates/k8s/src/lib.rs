//! Server-side Kubernetes layer for roder.
//!
//! - M2: [`ClusterAccess`] — the token-passthrough `kube::Client` holder.
//! - M3 (here): [`Backend`] — discovery catalog + shared-informer registry +
//!   generic row projection + object detail. One watch per type → etcd-kind.
//! - M4: metrics-server reads + cache-derived cluster summary.
//! - M6: mutation executors (apply/edit/delete/scale/rollout, Flux, ESO).

mod backend;
mod client;
mod discovery;
mod informers;
mod metrics;
mod printer_columns;
mod project;

#[cfg(test)]
mod test_alloc;

pub use backend::Backend;
pub use client::{ClusterAccess, K8sError};
pub use informers::WatchHandle;

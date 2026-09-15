//! Node drain options, progress events and job bookkeeping.

use serde::{Deserialize, Serialize};

/// Result of a node drain operation. `skipped` counts pods that did not require
/// eviction: DaemonSet-owned, mirror (static), terminal, and already-deleting pods.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct DrainSummary {
    pub evicted: usize,
    pub skipped: usize,
    pub failed: Vec<String>,
}

/// Minimum overall drain timeout accepted by [`DrainOptions::validate`].
/// Zero disables the timeout, matching `kubectl drain`.
pub const DRAIN_TIMEOUT_MIN_SECS: u64 = 0;
/// Maximum overall drain timeout accepted by [`DrainOptions::validate`].
pub const DRAIN_TIMEOUT_MAX_SECS: u64 = 3_600;
/// Maximum pod termination grace period accepted by [`DrainOptions::validate`].
pub const DRAIN_GRACE_PERIOD_MAX_SECS: u32 = 86_400;

/// Options for a node drain, mirroring `kubectl drain`'s flags.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct DrainOptions {
    /// Evict pods not managed by a controller (`--force`).
    pub force: bool,
    /// Evict pods using emptyDir volumes (`--delete-emptydir-data`).
    pub delete_emptydir_data: bool,
    /// Proceed while leaving DaemonSet pods in place (`--ignore-daemonsets`).
    /// When false, DaemonSet pods block the drain, matching kubectl's default.
    pub ignore_daemonsets: bool,
    /// Delete pods directly instead of the eviction API, bypassing
    /// PodDisruptionBudgets (`--disable-eviction`).
    pub disable_eviction: bool,
    /// Per-pod termination grace period override in seconds (`--grace-period`).
    pub grace_period: Option<u32>,
    /// Overall wall-clock budget for eviction + termination (`--timeout`).
    /// Zero waits indefinitely.
    pub timeout_secs: u64,
}

impl DrainOptions {
    /// Validate user-controlled duration fields against the shared drain bounds.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.timeout_secs > DRAIN_TIMEOUT_MAX_SECS {
            return Err("timeout must not exceed 3600 seconds");
        }
        if self
            .grace_period
            .is_some_and(|seconds| seconds > DRAIN_GRACE_PERIOD_MAX_SECS)
        {
            return Err("grace period must not exceed 86400 seconds");
        }
        Ok(())
    }
}

/// Cascade behavior for a delete request, mirroring `kubectl delete --cascade`.
/// `None` (the default) leaves it up to the API server's own default for the
/// resource type rather than forcing a specific policy.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum DeletePropagation {
    /// Delete the object now, leaving its dependents in place (`--cascade=orphan`).
    Orphan,
    /// Delete dependents in the background, after the owner is removed (`--cascade=background`).
    Background,
    /// Delete dependents first, then the owner, once they're gone (`--cascade=foreground`).
    Foreground,
}

/// One pod blocking a drain, and which [`DrainOptions`] field would clear it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DrainBlocker {
    pub pod: String,
    pub reason: String,
    /// `"force"`, `"delete_emptydir_data"`, or `"ignore_daemonsets"`.
    pub clearable_by: String,
}

/// One progress event from a drain job. `seq` increases monotonically from 0
/// so a resubscribing client can de-duplicate replayed events.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DrainEvent {
    pub seq: u64,
    #[serde(flatten)]
    pub kind: DrainEventKind,
}

/// Metadata needed by a refreshed client to reopen an unfinished drain job.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActiveDrainJob {
    pub job: u64,
    /// Replica currently executing this process-local job.
    #[serde(default)]
    pub executor: Option<String>,
    #[serde(default)]
    pub started_at: u64,
    pub key: String,
    pub name: String,
    pub power: Option<String>,
}

/// Process-local drain id paired with the replica that owns it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DrainJobRef {
    pub job: u64,
    #[serde(default)]
    pub executor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DrainEventKind {
    Started {
        total: usize,
    },
    Cordoned,
    Evicted {
        pod: String,
        done: usize,
        total: usize,
    },
    EvictFailed {
        pod: String,
        reason: String,
    },
    Blocked {
        blockers: Vec<DrainBlocker>,
    },
    WaitingTermination {
        pods: Vec<String>,
    },
    PowerRequested {
        action: String,
    },
    NodeReady,
    Done {
        summary: DrainSummary,
    },
    Error {
        message: String,
    },
    Cancelled,
}

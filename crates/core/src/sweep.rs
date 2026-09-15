//! Dead pod/job sweep options and results.

use serde::{Deserialize, Serialize};

/// Result of a sweep/sanitize operation.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct CleanupSummary {
    pub pods_deleted: usize,
    pub jobs_deleted: usize,
    pub forbidden: Vec<String>,
    pub failed: Vec<String>,
}

/// Resources currently matched by sweep options.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SweepCounts {
    pub pods: usize,
    pub jobs: usize,
}

/// Resource categories included in a sweep.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct SweepOptions {
    pub terminal_pods: bool,
    pub stuck_pods: bool,
    pub restarted_pods: bool,
    pub completed_jobs: bool,
    pub failed_jobs: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SanitizeAction {
    Preview,
    Execute,
}

impl Default for SweepOptions {
    fn default() -> Self {
        Self {
            terminal_pods: true,
            stuck_pods: true,
            restarted_pods: false,
            completed_jobs: true,
            failed_jobs: true,
        }
    }
}

impl SweepOptions {
    pub fn is_empty(self) -> bool {
        !self.terminal_pods
            && !self.stuck_pods
            && !self.restarted_pods
            && !self.completed_jobs
            && !self.failed_jobs
    }

    pub fn includes_pods(self) -> bool {
        self.terminal_pods || self.stuck_pods || self.restarted_pods
    }

    pub fn includes_jobs(self) -> bool {
        self.completed_jobs || self.failed_jobs
    }
}

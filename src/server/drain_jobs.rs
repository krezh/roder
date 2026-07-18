//! Registry of in-flight drain jobs: buffered events for lossless SSE replay,
//! a broadcast channel for live subscribers, and a shared cancel flag.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use roder_core::{ActiveDrainJob, DrainEvent, DrainEventKind};
use tokio::sync::broadcast;

/// How long a finished job's buffer stays subscribable.
const EXPIRY: std::time::Duration = std::time::Duration::from_secs(300);

struct Entry {
    owner: String,
    target: String,
    key: String,
    power: Option<String>,
    events: Vec<DrainEvent>,
    tx: broadcast::Sender<DrainEvent>,
    cancel: Arc<AtomicBool>,
    cancellable: bool,
    done: bool,
}

#[derive(Default)]
pub struct DrainJobs {
    entries: Mutex<HashMap<u64, Entry>>,
    next_id: AtomicU64,
}

pub struct JobHandle {
    pub id: u64,
    jobs: Arc<DrainJobs>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum CreateError {
    TargetBusy,
}

#[derive(Debug, PartialEq, Eq)]
pub enum CancelResult {
    Accepted,
    NotFound,
    NotCancellable,
}

impl DrainJobs {
    pub fn create(
        self: &Arc<Self>,
        owner: String,
        target: String,
    ) -> Result<JobHandle, CreateError> {
        self.create_for(owner, String::new(), target, None)
    }

    pub fn create_for(
        self: &Arc<Self>,
        owner: String,
        key: String,
        target: String,
        power: Option<String>,
    ) -> Result<JobHandle, CreateError> {
        let mut entries = self.entries.lock().unwrap();
        if entries
            .values()
            .any(|entry| !entry.done && entry.target == target)
        {
            return Err(CreateError::TargetBusy);
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, _) = broadcast::channel(256);
        entries.insert(
            id,
            Entry {
                owner,
                target,
                key,
                power,
                events: Vec::new(),
                tx,
                cancel: Arc::new(AtomicBool::new(false)),
                cancellable: true,
                done: false,
            },
        );
        Ok(JobHandle {
            id,
            jobs: Arc::clone(self),
        })
    }

    /// Newest unfinished job owned by this caller, for browser refresh recovery.
    pub fn active(&self, owner: &str) -> Option<ActiveDrainJob> {
        self.entries
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, entry)| entry.owner == owner && !entry.done)
            .max_by_key(|(id, _)| *id)
            .map(|(id, entry)| ActiveDrainJob {
                job: *id,
                key: entry.key.clone(),
                name: entry.target.clone(),
                power: entry.power.clone(),
            })
    }

    /// Snapshot + subscribe under one lock so no event can fall between them.
    pub fn subscribe(
        &self,
        owner: &str,
        id: u64,
    ) -> Option<(Vec<DrainEvent>, broadcast::Receiver<DrainEvent>, bool)> {
        let entries = self.entries.lock().unwrap();
        let e = entries.get(&id)?;
        if e.owner != owner {
            return None;
        }
        Some((e.events.clone(), e.tx.subscribe(), e.done))
    }

    /// Replay buffered events newer than `after`, scoped to the job owner.
    pub fn replay_after(
        &self,
        owner: &str,
        id: u64,
        after: Option<u64>,
    ) -> Option<Vec<DrainEvent>> {
        let entries = self.entries.lock().unwrap();
        let e = entries.get(&id)?;
        if e.owner != owner {
            return None;
        }
        Some(
            e.events
                .iter()
                .filter(|event| after.is_none_or(|seq| event.seq > seq))
                .cloned()
                .collect(),
        )
    }

    pub fn cancel(&self, owner: &str, id: u64) -> CancelResult {
        let entries = self.entries.lock().unwrap();
        let Some(e) = entries.get(&id) else {
            return CancelResult::NotFound;
        };
        if e.owner != owner {
            return CancelResult::NotFound;
        }
        if !e.cancellable {
            return CancelResult::NotCancellable;
        }
        e.cancel.store(true, Ordering::Relaxed);
        CancelResult::Accepted
    }
}

impl JobHandle {
    pub fn cancel_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(
            &self
                .jobs
                .entries
                .lock()
                .unwrap()
                .get(&self.id)
                .expect("job handle must reference a registered job")
                .cancel,
        )
    }

    /// Atomically close cancellation after drain work ends and before cleanup,
    /// power work, or the terminal event. Returns false if cancellation won.
    pub fn begin_non_cancellable(&self) -> bool {
        let mut entries = self.jobs.entries.lock().unwrap();
        let Some(e) = entries.get_mut(&self.id) else {
            return false;
        };
        if e.done || !e.cancellable || e.cancel.load(Ordering::Relaxed) {
            return false;
        }
        e.cancellable = false;
        true
    }

    pub fn emit(&self, kind: DrainEventKind) {
        let mut entries = self.jobs.entries.lock().unwrap();
        let Some(e) = entries.get_mut(&self.id) else {
            return;
        };
        if e.done {
            return;
        }
        let ev = DrainEvent {
            seq: e.events.len() as u64,
            kind,
        };
        e.events.push(ev.clone());
        let _ = e.tx.send(ev);
    }

    /// Atomically append the terminal event and close the job. Subscribers can
    /// therefore never observe the terminal replay with `done == false`.
    pub fn finish(&self, kind: DrainEventKind) {
        debug_assert!(matches!(
            kind,
            DrainEventKind::Done { .. } | DrainEventKind::Error { .. } | DrainEventKind::Cancelled
        ));
        {
            let mut entries = self.jobs.entries.lock().unwrap();
            let Some(e) = entries.get_mut(&self.id) else {
                return;
            };
            if e.done {
                return;
            }
            let event = DrainEvent {
                seq: e.events.len() as u64,
                kind,
            };
            e.events.push(event.clone());
            e.cancellable = false;
            e.done = true;
            let _ = e.tx.send(event);
        }
        let jobs = Arc::clone(&self.jobs);
        let id = self.id;
        tokio::spawn(async move {
            tokio::time::sleep(EXPIRY).await;
            jobs.entries.lock().unwrap().remove(&id);
        });
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;
    use std::sync::Arc;

    use roder_core::DrainEventKind;

    use super::*;

    #[tokio::test]
    async fn replay_then_live_has_no_gaps_or_dupes() {
        let jobs = Arc::new(DrainJobs::default());
        let h = jobs.create("alice".into(), "node-a".into()).unwrap();
        h.emit(DrainEventKind::Cordoned);
        h.emit(DrainEventKind::Started { total: 2 });

        let (replay, mut rx, done) = jobs.subscribe("alice", h.id).unwrap();
        assert_eq!(replay.iter().map(|e| e.seq).collect::<Vec<_>>(), vec![0, 1]);
        assert!(!done);

        h.emit(DrainEventKind::NodeReady);
        let live = rx.recv().await.unwrap();
        assert_eq!(live.seq, 2);
    }

    #[tokio::test]
    async fn cancel_is_owner_scoped_and_lifecycle_aware() {
        let jobs = Arc::new(DrainJobs::default());
        let h = jobs.create("alice".into(), "node-a".into()).unwrap();
        let flag = h.cancel_flag();
        assert!(!flag.load(Ordering::Relaxed));
        assert_eq!(jobs.cancel("bob", h.id), CancelResult::NotFound);
        assert_eq!(jobs.cancel("alice", 9999), CancelResult::NotFound);
        assert_eq!(jobs.cancel("alice", h.id), CancelResult::Accepted);
        assert!(flag.load(Ordering::Relaxed));
        assert!(!h.begin_non_cancellable());
    }

    #[tokio::test]
    async fn accepted_cancellation_wins_the_error_transition() {
        let jobs = Arc::new(DrainJobs::default());
        let h = jobs.create("alice".into(), "node-a".into()).unwrap();

        assert_eq!(jobs.cancel("alice", h.id), CancelResult::Accepted);
        assert!(!h.begin_non_cancellable());
        h.finish(DrainEventKind::Cancelled);
        let (replay, _, done) = jobs.subscribe("alice", h.id).unwrap();
        assert!(done);
        assert!(matches!(
            replay.last().map(|event| &event.kind),
            Some(DrainEventKind::Cancelled)
        ));
    }

    #[tokio::test]
    async fn finish_atomically_exposes_terminal_replay_and_done() {
        let jobs = Arc::new(DrainJobs::default());
        let h = jobs.create("alice".into(), "node-a".into()).unwrap();
        h.finish(DrainEventKind::Done {
            summary: Default::default(),
        });
        let (replay, _, done) = jobs.subscribe("alice", h.id).unwrap();
        assert!(done);
        assert!(matches!(
            replay.as_slice(),
            [DrainEvent {
                kind: DrainEventKind::Done { .. },
                ..
            }]
        ));
        assert_eq!(jobs.cancel("alice", h.id), CancelResult::NotCancellable);
    }

    #[tokio::test]
    async fn duplicate_target_is_released_when_the_first_job_finishes() {
        let jobs = Arc::new(DrainJobs::default());
        let first = jobs.create("alice".into(), "node-a".into()).unwrap();
        assert!(matches!(
            jobs.create("bob".into(), "node-a".into()),
            Err(CreateError::TargetBusy)
        ));

        first.finish(DrainEventKind::Cancelled);
        assert!(jobs.create("bob".into(), "node-a".into()).is_ok());
    }

    #[tokio::test]
    async fn owner_scopes_subscribe_and_replay() {
        let jobs = Arc::new(DrainJobs::default());
        let h = jobs.create("alice".into(), "node-a".into()).unwrap();
        h.emit(DrainEventKind::Cordoned);

        assert!(jobs.subscribe("bob", h.id).is_none());
        assert!(jobs.replay_after("bob", h.id, None).is_none());
        assert_eq!(jobs.replay_after("alice", h.id, None).unwrap().len(), 1);
    }

    #[tokio::test]
    async fn power_transition_closes_the_cancel_race() {
        let jobs = Arc::new(DrainJobs::default());
        let h = jobs.create("alice".into(), "node-a".into()).unwrap();

        assert!(h.begin_non_cancellable());
        assert_eq!(jobs.cancel("alice", h.id), CancelResult::NotCancellable);
    }

    #[tokio::test]
    async fn active_returns_only_the_owners_newest_unfinished_job() {
        let jobs = Arc::new(DrainJobs::default());
        let old = jobs
            .create_for("alice".into(), "nodes".into(), "node-a".into(), None)
            .unwrap();
        old.finish(DrainEventKind::Cancelled);
        let active = jobs
            .create_for(
                "alice".into(),
                "nodes".into(),
                "node-b".into(),
                Some("reboot".into()),
            )
            .unwrap();
        jobs.create_for("bob".into(), "nodes".into(), "node-c".into(), None)
            .unwrap();

        assert_eq!(
            jobs.active("alice"),
            Some(ActiveDrainJob {
                job: active.id,
                key: "nodes".into(),
                name: "node-b".into(),
                power: Some("reboot".into()),
            })
        );
        assert_eq!(jobs.active("bob").unwrap().name, "node-c");
        assert!(jobs.active("charlie").is_none());
    }
}

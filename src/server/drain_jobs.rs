//! Registry of in-flight drain jobs: buffered events for lossless SSE replay,
//! a broadcast channel for live subscribers, and a shared cancel flag.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use roder_core::{DrainEvent, DrainEventKind};
use tokio::sync::broadcast;

/// How long a finished job's buffer stays subscribable.
const EXPIRY: std::time::Duration = std::time::Duration::from_secs(300);

struct Entry {
    events: Vec<DrainEvent>,
    tx: broadcast::Sender<DrainEvent>,
    cancel: Arc<AtomicBool>,
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

impl DrainJobs {
    pub fn create(self: &Arc<Self>) -> JobHandle {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, _) = broadcast::channel(256);
        self.entries.lock().unwrap().insert(
            id,
            Entry {
                events: Vec::new(),
                tx,
                cancel: Arc::new(AtomicBool::new(false)),
                done: false,
            },
        );
        JobHandle {
            id,
            jobs: Arc::clone(self),
        }
    }

    /// Snapshot + subscribe under one lock so no event can fall between them.
    pub fn subscribe(
        &self,
        id: u64,
    ) -> Option<(Vec<DrainEvent>, broadcast::Receiver<DrainEvent>, bool)> {
        let entries = self.entries.lock().unwrap();
        let e = entries.get(&id)?;
        Some((e.events.clone(), e.tx.subscribe(), e.done))
    }

    pub fn cancel(&self, id: u64) -> bool {
        let entries = self.entries.lock().unwrap();
        let Some(e) = entries.get(&id) else {
            return false;
        };
        e.cancel.store(true, Ordering::Relaxed);
        true
    }

    pub fn cancel_flag(&self, id: u64) -> Option<Arc<AtomicBool>> {
        Some(Arc::clone(&self.entries.lock().unwrap().get(&id)?.cancel))
    }
}

impl JobHandle {
    pub fn emit(&self, kind: DrainEventKind) {
        let mut entries = self.jobs.entries.lock().unwrap();
        let Some(e) = entries.get_mut(&self.id) else {
            return;
        };
        let ev = DrainEvent {
            seq: e.events.len() as u64,
            kind,
        };
        e.events.push(ev.clone());
        let _ = e.tx.send(ev);
    }

    pub fn finish(&self) {
        if let Some(e) = self.jobs.entries.lock().unwrap().get_mut(&self.id) {
            e.done = true;
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
        let h = jobs.create();
        h.emit(DrainEventKind::Cordoned);
        h.emit(DrainEventKind::Started { total: 2 });

        let (replay, mut rx, done) = jobs.subscribe(h.id).unwrap();
        assert_eq!(replay.iter().map(|e| e.seq).collect::<Vec<_>>(), vec![0, 1]);
        assert!(!done);

        h.emit(DrainEventKind::NodeReady);
        let live = rx.recv().await.unwrap();
        assert_eq!(live.seq, 2);
    }

    #[tokio::test]
    async fn cancel_trips_the_shared_flag() {
        let jobs = Arc::new(DrainJobs::default());
        let h = jobs.create();
        let flag = jobs.cancel_flag(h.id).unwrap();
        assert!(!flag.load(Ordering::Relaxed));
        assert!(jobs.cancel(h.id));
        assert!(flag.load(Ordering::Relaxed));
        assert!(!jobs.cancel(9999));
    }

    #[tokio::test]
    async fn finished_jobs_report_done_on_subscribe() {
        let jobs = Arc::new(DrainJobs::default());
        let h = jobs.create();
        h.emit(DrainEventKind::Done {
            summary: Default::default(),
        });
        h.finish();
        let (_, _, done) = jobs.subscribe(h.id).unwrap();
        assert!(done);
    }
}

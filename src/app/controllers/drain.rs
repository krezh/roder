use roder_core::{
    DrainBlocker, DrainEvent, DrainEventKind, DrainJobRef, DrainOptions,
    DRAIN_GRACE_PERIOD_MAX_SECS, DRAIN_TIMEOUT_MAX_SECS, DRAIN_TIMEOUT_MIN_SECS,
};

use crate::app::state::DrainTarget;

#[derive(Clone, PartialEq)]
pub(crate) enum DrainPhase {
    Options,
    Running(DrainJobRef),
}

#[derive(Clone, Default)]
pub(crate) struct DrainProgressState {
    pub(crate) log: Vec<String>,
    pub(crate) evicted: usize,
    pub(crate) total: usize,
    pub(crate) finished: bool,
    pub(crate) successful: bool,
    pub(crate) power_requested: bool,
    pub(crate) node_ready: bool,
    pub(crate) retry_options: Vec<String>,
}

impl DrainProgressState {
    pub(crate) fn apply(&mut self, event: DrainEventKind) {
        match event {
            DrainEventKind::Cordoned => self.log.push("node cordoned".into()),
            DrainEventKind::Started { total } => {
                self.total = total;
                self.log.push(format!("evicting {total} pods"));
            }
            DrainEventKind::Evicted { pod, done, .. } => {
                self.evicted = done;
                self.log.push(format!("evicted {pod}"));
            }
            DrainEventKind::EvictFailed { pod, reason } => {
                self.log.push(format!("FAILED {pod}: {reason}"));
            }
            DrainEventKind::Blocked { blockers } => {
                for blocker in &blockers {
                    self.log
                        .push(format!("blocked by {}: {}", blocker.pod, blocker.reason));
                }
                self.retry_options = distinct_clearable_by(&blockers);
                self.finished = true;
            }
            DrainEventKind::WaitingTermination { pods } => {
                self.log.push(waiting_message(&pods));
            }
            DrainEventKind::PowerRequested { action } => {
                self.power_requested = true;
                self.log.push(format!("{action} requested"));
            }
            DrainEventKind::NodeReady => {
                self.node_ready = true;
                self.log.push("node returned Ready".into());
            }
            DrainEventKind::Done { summary } => {
                self.successful = summary.failed.is_empty();
                self.finished = true;
                self.log.push(format!(
                    "done: evicted {}, skipped {}, {} failed",
                    summary.evicted,
                    summary.skipped,
                    summary.failed.len()
                ));
                self.log.extend(
                    summary
                        .failed
                        .into_iter()
                        .map(|reason| format!("FAILED: {reason}")),
                );
            }
            DrainEventKind::Error { message } => {
                self.log.push(format!("ERROR: {message}"));
                self.finished = true;
            }
            DrainEventKind::Cancelled => {
                self.log.push("cancelled - node remains cordoned".into());
                self.finished = true;
            }
        }
    }
}

pub(crate) fn parse_options(
    force: bool,
    delete_emptydir_data: bool,
    ignore_daemonsets: bool,
    disable_eviction: bool,
    grace: &str,
    timeout: &str,
) -> DrainOptions {
    DrainOptions {
        force,
        delete_emptydir_data,
        ignore_daemonsets,
        disable_eviction,
        grace_period: grace
            .trim()
            .parse::<u32>()
            .ok()
            .map(|value| value.min(DRAIN_GRACE_PERIOD_MAX_SECS)),
        timeout_secs: timeout
            .trim()
            .parse::<u64>()
            .unwrap_or(DrainOptions::default().timeout_secs)
            .clamp(DRAIN_TIMEOUT_MIN_SECS, DRAIN_TIMEOUT_MAX_SECS),
    }
}

pub(crate) async fn start(
    target: &DrainTarget,
    options: &DrainOptions,
) -> Result<DrainJobRef, String> {
    let payload = match &target.power {
        None => serde_json::json!({
            "action": "drain", "key": target.key, "name": target.name, "options": options,
        }),
        Some(power) => serde_json::json!({
            "action": format!("talos-{power}"), "name": target.name, "drain": true, "options": options,
        }),
    };
    let body = crate::data::post_action(&payload).await?;
    serde_json::from_str(&body).map_err(|_| format!("unexpected response: {body}"))
}

pub(crate) async fn cancel(job: &DrainJobRef) -> Result<String, String> {
    crate::data::post_action(&serde_json::json!({
        "action": "drain-cancel", "job": job.job, "executor": job.executor,
    }))
    .await
}

pub(crate) fn subscribe(
    job: &DrainJobRef,
    on_event: impl Fn(DrainEvent) + 'static,
) -> Option<crate::data::SseHandle> {
    let last_seq = std::cell::Cell::new(None);
    crate::data::subscribe_lines(&progress_url(job), move |line| {
        let Ok(event) = serde_json::from_str::<DrainEvent>(&line) else {
            return;
        };
        if last_seq.get().is_some_and(|sequence| event.seq <= sequence) {
            return;
        }
        last_seq.set(Some(event.seq));
        on_event(event);
    })
}

pub(crate) fn progress_percent(state: &DrainProgressState, power: Option<&str>) -> usize {
    if state.successful {
        return 100;
    }
    let milestones = match power {
        Some("reboot") => 3,
        Some("shutdown") => 2,
        _ => 0,
    };
    let complete = state.evicted.min(state.total)
        + usize::from(state.power_requested)
        + usize::from(state.node_ready);
    complete.saturating_mul(100) / state.total.saturating_add(milestones).max(1)
}

pub(crate) fn option_label(option: &str) -> &'static str {
    match option {
        "force" => "Force",
        "delete_emptydir_data" => "Delete emptyDir data",
        "ignore_daemonsets" => "Ignore DaemonSets",
        _ => "Unknown option",
    }
}

fn distinct_clearable_by(blockers: &[DrainBlocker]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    blockers
        .iter()
        .map(|blocker| blocker.clearable_by.clone())
        .filter(|option| seen.insert(option.clone()))
        .collect()
}

fn waiting_message(pods: &[String]) -> String {
    let noun = if pods.len() == 1 { "pod" } else { "pods" };
    format!(
        "waiting for {} {noun} to terminate: {}",
        pods.len(),
        pods.join(", ")
    )
}

fn progress_url(job: &DrainJobRef) -> String {
    match job.executor.as_deref() {
        Some(executor) => format!("/api/drain-progress?id={}&executor={executor}", job.job),
        None => format!("/api/drain-progress?id={}", job.job),
    }
}

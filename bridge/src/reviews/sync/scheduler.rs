//! Task identity coalescing and retry deadlines, independent of wall clocks.
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

const QUIET: Duration = Duration::from_millis(500);
const MAX_WAIT: Duration = Duration::from_secs(2);

#[derive(Default)]
pub(super) struct Scheduler {
    tasks: BTreeMap<String, Pending>,
}
struct Pending {
    first: Instant,
    due: Instant,
    retry_after: Instant,
    failures: u32,
    queued: bool,
}
impl Scheduler {
    pub(super) fn enqueue(&mut self, task: String, now: Instant, immediate: bool) {
        let entry = self.tasks.entry(task).or_insert(Pending {
            first: now,
            due: now,
            retry_after: now,
            failures: 0,
            queued: false,
        });
        if !entry.queued {
            entry.first = now;
        }
        entry.queued = true;
        entry.due = if immediate {
            now
        } else {
            (now + QUIET).min(entry.first + MAX_WAIT)
        };
    }
    pub(super) fn ready(&mut self, now: Instant, limit: usize) -> Vec<String> {
        let mut ready = Vec::new();
        for (task, entry) in &mut self.tasks {
            if ready.len() == limit {
                break;
            }
            if entry.queued && now >= entry.due && now >= entry.retry_after {
                entry.queued = false;
                ready.push(task.clone());
            }
        }
        ready
    }
    pub(super) fn settle(&mut self, task: &str, now: Instant, retry: bool) {
        let Some(entry) = self.tasks.get_mut(task) else {
            return;
        };
        if retry {
            entry.failures = entry.failures.saturating_add(1);
            entry.retry_after = now + Duration::from_secs((1u64 << entry.failures.min(5)).min(30));
            entry.due = entry.retry_after;
            entry.queued = true;
        } else {
            entry.failures = 0;
            entry.retry_after = now;
        }
    }
    pub(super) fn retain(&mut self, active: &BTreeSet<String>) {
        self.tasks.retain(|task, _| active.contains(task));
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quiet_window_has_a_hard_cap_and_tasks_coalesce() {
        let mut scheduler = Scheduler::default();
        let start = Instant::now();
        for milliseconds in [0, 400, 800, 1200, 1600, 1900] {
            scheduler.enqueue(
                "task".into(),
                start + Duration::from_millis(milliseconds),
                false,
            );
        }
        assert!(scheduler
            .ready(start + Duration::from_millis(1999), 8)
            .is_empty());
        assert_eq!(scheduler.ready(start + MAX_WAIT, 8), ["task"]);
        assert!(scheduler.ready(start + MAX_WAIT, 8).is_empty());
    }
    #[test]
    fn polling_and_events_cannot_bypass_task_backoff() {
        let mut scheduler = Scheduler::default();
        let start = Instant::now();
        scheduler.enqueue("bad".into(), start, true);
        scheduler.enqueue("good".into(), start, true);
        assert_eq!(scheduler.ready(start, 1), ["bad"]);
        scheduler.settle("bad", start, true);
        scheduler.enqueue("bad".into(), start, true);
        assert_eq!(scheduler.ready(start, 8), ["good"]);
        assert_eq!(scheduler.ready(start + Duration::from_secs(2), 8), ["bad"]);
    }
}

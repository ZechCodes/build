//! The size walks the Workspaces tab asks for (#273).
//!
//! A workspace's size on disk is only ever read in that tab, so it is walked
//! when the tab asks rather than on every sweep. This is the queue between the
//! asking and the walking: the request names workspaces, the service thread
//! takes them one at a time. A workspace already waiting, or being walked, is
//! not queued twice. A size measured within [`SIZE_REUSE_WINDOW`] is reused.
//!
//! No app state here. The service that walks and writes the size is
//! `app/workspaces/sizes.rs`.

use super::LifecycleRecord;
use std::collections::{HashSet, VecDeque};
use std::sync::{Condvar, Mutex};
use std::time::Duration;

/// How long a measured size stands before a request walks the workspace again.
pub const SIZE_REUSE_WINDOW: Duration = Duration::from_secs(5 * 60);

/// Whether `record` holds a size measured within [`SIZE_REUSE_WINDOW`] of
/// `now_ms`.
pub fn size_is_fresh(record: Option<&LifecycleRecord>, now_ms: i64) -> bool {
    let window = i64::try_from(SIZE_REUSE_WINDOW.as_millis()).unwrap_or(i64::MAX);
    record
        .filter(|record| record.size_bytes.is_some())
        .and_then(|record| record.size_measured_at_ms)
        .is_some_and(|at| now_ms.saturating_sub(at) < window)
}

/// The workspaces waiting for a size walk, and the one being walked.
#[derive(Default)]
struct Pending {
    waiting: VecDeque<String>,
    /// Waiting or being walked: what a request does not queue again.
    held: HashSet<String>,
}

/// The queue the request fills and the service thread drains.
#[derive(Default)]
pub struct SizeRequests {
    pending: Mutex<Pending>,
    arrived: Condvar,
}

impl SizeRequests {
    /// Queue each workspace not already waiting or being walked, and wake the
    /// walker. Answers the ones this call queued.
    pub fn request(&self, workspace_ids: impl IntoIterator<Item = String>) -> Vec<String> {
        let mut pending = self.pending.lock().unwrap();
        let queued: Vec<String> = workspace_ids
            .into_iter()
            .filter(|id| pending.held.insert(id.clone()))
            .collect();
        pending.waiting.extend(queued.iter().cloned());
        if !queued.is_empty() {
            self.arrived.notify_all();
        }
        queued
    }

    /// The next workspace to walk, if one is waiting. It stays held until
    /// [`SizeRequests::done`].
    pub fn next(&self) -> Option<String> {
        self.pending.lock().unwrap().waiting.pop_front()
    }

    /// The walk of this workspace is over, however it ended: a later request
    /// may queue it again.
    pub fn done(&self, workspace_id: &str) {
        self.pending.lock().unwrap().held.remove(workspace_id);
    }

    /// Wait up to `timeout` for something to walk.
    pub fn wait(&self, timeout: Duration) {
        let pending = self.pending.lock().unwrap();
        if pending.waiting.is_empty() {
            let _ = self.arrived.wait_timeout(pending, timeout);
        }
    }

    /// Wake a waiting walker, so it sees the daemon stopping.
    pub fn wake(&self) {
        self.arrived.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    #[test]
    fn a_workspace_is_queued_once_until_its_walk_is_done() {
        let requests = SizeRequests::default();
        assert_eq!(requests.request(ids(&["a", "b", "a"])), ids(&["a", "b"]));
        assert_eq!(requests.request(ids(&["a"])), ids(&[]), "still waiting");

        assert_eq!(requests.next().as_deref(), Some("a"));
        assert_eq!(requests.request(ids(&["a"])), ids(&[]), "being walked");
        requests.done("a");
        assert_eq!(requests.request(ids(&["a"])), ids(&["a"]), "walked");

        assert_eq!(requests.next().as_deref(), Some("b"));
        assert_eq!(requests.next().as_deref(), Some("a"));
        assert_eq!(requests.next(), None);
    }

    #[test]
    fn a_size_is_fresh_only_inside_the_window() {
        let window = i64::try_from(SIZE_REUSE_WINDOW.as_millis()).unwrap();
        let measured = |size_bytes, at| LifecycleRecord {
            size_bytes,
            size_measured_at_ms: at,
            ..LifecycleRecord::default()
        };
        let now = 10 * window;
        assert!(size_is_fresh(Some(&measured(Some(1), Some(now - 1))), now));
        assert!(!size_is_fresh(
            Some(&measured(Some(1), Some(now - window))),
            now
        ));
        assert!(!size_is_fresh(Some(&measured(None, Some(now))), now));
        assert!(!size_is_fresh(Some(&measured(Some(1), None)), now));
        assert!(!size_is_fresh(None, now));
    }
}

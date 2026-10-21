//! A bounded fixture protocol with one owned, cancellable completion worker.
use crate::harness::{AgentStatus, HarnessError, SessionStatusSnapshot};
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tokio::sync::watch;

struct Pending {
    updates: Option<watch::Sender<SessionStatusSnapshot>>,
    completions: VecDeque<Instant>,
    stopping: bool,
}

impl Pending {
    fn transition(&self, status: AgentStatus) {
        if let Some(updates) = &self.updates {
            updates.send_if_modified(|snapshot| match snapshot.transition(status) {
                Some(next) => {
                    *snapshot = next;
                    true
                }
                None => false,
            });
        }
    }
}

struct Queue {
    pending: Mutex<Pending>,
    changed: Condvar,
}

pub(super) struct FiniteTurns {
    queue: Arc<Queue>,
    worker: Mutex<Option<JoinHandle<()>>>,
    delay: Duration,
}

impl FiniteTurns {
    pub(super) fn start(delay: Duration) -> (Self, watch::Receiver<SessionStatusSnapshot>) {
        let (updates, watched) = watch::channel(SessionStatusSnapshot::new(AgentStatus::Waiting));
        let queue = Arc::new(Queue {
            pending: Mutex::new(Pending {
                updates: Some(updates),
                completions: VecDeque::new(),
                stopping: false,
            }),
            changed: Condvar::new(),
        });
        let working = Arc::clone(&queue);
        let worker = std::thread::spawn(move || complete_turns(&working));
        (
            Self {
                queue,
                worker: Mutex::new(Some(worker)),
                delay,
            },
            watched,
        )
    }

    pub(super) fn sent(&self) -> Result<(), HarnessError> {
        let mut pending = self.queue.pending.lock().unwrap();
        if pending.stopping {
            return Err(HarnessError::Session("finite session has ended".into()));
        }
        pending.completions.push_back(Instant::now() + self.delay);
        pending.transition(AgentStatus::Working);
        self.queue.changed.notify_one();
        Ok(())
    }

    pub(super) fn end(&self) {
        // Serialize concurrent end calls through the join itself. Every caller
        // returns only after the owned worker and status producer have ended.
        let mut worker = self.worker.lock().unwrap();
        {
            let mut pending = self.queue.pending.lock().unwrap();
            pending.stopping = true;
            self.queue.changed.notify_one();
        }
        if let Some(worker) = worker.take() {
            worker
                .join()
                .expect("finite turn worker must stop without panicking");
        }
    }
}

impl Drop for FiniteTurns {
    fn drop(&mut self) {
        self.end();
    }
}

fn complete_turns(queue: &Queue) {
    let mut pending = queue.pending.lock().unwrap();
    while !pending.stopping {
        let Some(deadline) = pending.completions.front().copied() else {
            pending = queue.changed.wait(pending).unwrap();
            continue;
        };
        let remaining = deadline.saturating_duration_since(Instant::now());
        if !remaining.is_zero() {
            pending = queue.changed.wait_timeout(pending, remaining).unwrap().0;
            continue;
        }
        pending.completions.pop_front();
        // An earlier turn must not announce Waiting over a newer accepted
        // turn. Each deadline is bounded from its actual send, with no serial
        // delay added by earlier sends.
        if pending.completions.is_empty() {
            pending.transition(AgentStatus::Waiting);
        }
    }
    pending.completions.clear();
    pending.transition(AgentStatus::Ended { code: None });
    pending.updates.take();
}

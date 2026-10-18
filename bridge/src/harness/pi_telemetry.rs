//! Scoped Pi extension observations, separate from terminal painting.
use super::compaction::{AppliedCompactionPolicy, CompactionEnforcement};
use super::{
    ActivityReport, AgentActivity, AgentStatus, HarnessError, SessionStatusSnapshot, TurnContext,
};
use serde::Deserialize;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, watch};

#[derive(Clone, Debug)]
pub struct PiTelemetryConfig {
    pub path: PathBuf,
    pub owner: String,
    pub policy: AppliedCompactionPolicy,
}

#[cfg(test)]
mod tests;

const UNCONFIRMED: u8 = 0;
const CONFIRMED: u8 = 1;
const UNAVAILABLE: u8 = 2;

#[derive(Default)]
struct State {
    sequence: u64,
    pending: bool,
    compacting: bool,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Event {
    Ready,
    Working,
    Context {
        tokens: Option<u64>,
    },
    Settled,
    Compaction {
        completed: bool,
        #[serde(default)]
        failed: bool,
        reason: Option<String>,
    },
    Unsupported,
    #[serde(other)]
    Unknown,
}

pub(crate) struct PiTelemetry {
    config: PiTelemetryConfig,
    unconfirmed: AppliedCompactionPolicy,
    unavailable: AppliedCompactionPolicy,
    confirmation: AtomicU8,
    draining: AtomicBool,
    state: Mutex<State>,
    status: watch::Sender<SessionStatusSnapshot>,
    activity: Mutex<Option<broadcast::Sender<ActivityReport>>>,
    activity_rx: Mutex<Option<broadcast::Receiver<ActivityReport>>>,
}

impl PiTelemetry {
    pub(crate) fn new(config: PiTelemetryConfig) -> Self {
        let unconfirmed = unsupported(&config.policy, "pi_extension_unconfirmed");
        let unavailable = unsupported(&config.policy, "pi_extension_unavailable");
        let (status, _) = watch::channel(SessionStatusSnapshot::new(AgentStatus::Starting));
        let (activity, rx) = broadcast::channel(64);
        Self {
            config,
            unconfirmed,
            unavailable,
            confirmation: AtomicU8::new(UNCONFIRMED),
            draining: AtomicBool::new(false),
            state: Mutex::new(State::default()),
            status,
            activity: Mutex::new(Some(activity)),
            activity_rx: Mutex::new(Some(rx)),
        }
    }

    pub(crate) fn follow(
        self: &Arc<Self>,
        alive: super::transcript_activity::TerminalAlive,
        on_failure: impl FnOnce() + Send + 'static,
    ) {
        let telemetry = Arc::clone(self);
        std::thread::spawn(move || {
            let started = Instant::now();
            let mut cursor = super::transcript_activity::Cursor::default();
            while alive
                .upgrade()
                .is_some_and(|live| live.lock().unwrap().is_some())
            {
                cursor = super::transcript_activity::read_complete_lines(
                    &telemetry.config.path,
                    cursor,
                    |value| telemetry.observe(value),
                );
                if started.elapsed() >= Duration::from_secs(10)
                    && telemetry.confirmation.load(Ordering::Acquire) == UNCONFIRMED
                {
                    telemetry.fail();
                }
                if telemetry.failed() {
                    on_failure();
                    break;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            telemetry.finish(cursor);
        });
    }

    fn finish(&self, mut cursor: super::transcript_activity::Cursor) {
        // The terminal can close before the last sidecar poll. Consume its
        // complete records, but never publish a fresh idle boundary at EOF.
        self.draining.store(true, Ordering::Release);
        loop {
            let mut observed = false;
            cursor = super::transcript_activity::read_complete_lines(
                &self.config.path,
                cursor,
                |value| {
                    observed = true;
                    self.observe(value);
                },
            );
            if !observed {
                break;
            }
        }
        if !self.confirmed() {
            self.fail();
        }
        self.transition(AgentStatus::Ended { code: None });
        // A dead session remains in the registry for its terminal/history.
        // Its activity stream must still close after the final status exists.
        self.activity.lock().unwrap().take();
    }

    pub(crate) fn snapshot(&self) -> SessionStatusSnapshot {
        self.status.borrow().clone()
    }
    pub(crate) fn subscribe(&self) -> watch::Receiver<SessionStatusSnapshot> {
        self.status.subscribe()
    }
    pub(crate) fn activity(&self) -> Option<broadcast::Receiver<ActivityReport>> {
        self.activity_rx.lock().unwrap().take()
    }
    pub(crate) fn path(&self) -> &std::path::Path {
        &self.config.path
    }
    pub(crate) fn confirmed(&self) -> bool {
        self.confirmation.load(Ordering::Acquire) == CONFIRMED
    }
    pub(crate) fn failed(&self) -> bool {
        self.confirmation.load(Ordering::Acquire) == UNAVAILABLE
    }
    pub(crate) fn policy(&self) -> &AppliedCompactionPolicy {
        match self.confirmation.load(Ordering::Acquire) {
            CONFIRMED => &self.config.policy,
            UNAVAILABLE => &self.unavailable,
            _ => &self.unconfirmed,
        }
    }

    pub(crate) fn automatic_due(&self, tokens: Option<u64>) -> bool {
        let snapshot = self.snapshot();
        snapshot.status == AgentStatus::Waiting
            && snapshot.context.map(|context| context.context_tokens) == tokens
            && self.policy().automatic_due(tokens)
    }

    pub(crate) fn sent(&self, compaction: bool) -> Result<(), HarnessError> {
        let mut state = self.state.lock().unwrap();
        if self.draining.load(Ordering::Acquire) {
            return Err(HarnessError::Session("Pi session has ended".into()));
        }
        if self.failed() {
            return Err(HarnessError::Unsupported(
                "Pi compaction extension is unavailable".into(),
            ));
        }
        if compaction && (!self.confirmed() || self.snapshot().status != AgentStatus::Waiting) {
            return Err(HarnessError::Unsupported(
                "Pi compaction requires a confirmed idle boundary".into(),
            ));
        }
        state.pending = true;
        self.transition(AgentStatus::Working);
        Ok(())
    }

    pub(crate) fn fail(&self) {
        self.confirmation.store(UNAVAILABLE, Ordering::Release);
        self.clear_context();
    }

    pub(crate) fn observe(&self, value: &Value) {
        if !self.matches_identity(value) || self.confirmation.load(Ordering::Acquire) == UNAVAILABLE
        {
            return;
        }
        let mut state = self.state.lock().unwrap();
        let Some(sequence) = value["sequence"].as_u64() else {
            self.fail();
            return;
        };
        if sequence <= state.sequence {
            return;
        }
        if sequence != state.sequence + 1 {
            self.fail();
            return;
        }
        state.sequence = sequence;
        match serde_json::from_value::<Event>(value.clone()) {
            Ok(event) => self.apply(event, &mut state),
            Err(_) => self.fail(),
        }
    }

    fn matches_identity(&self, value: &Value) -> bool {
        let identity = &self.config.policy.launch_identity;
        value["owner"] == self.config.owner
            && value["thread_id"] == identity.thread_id
            && value["launch_id"] == identity.id
    }

    fn apply(&self, event: Event, state: &mut State) {
        match event {
            Event::Ready => {
                if self
                    .confirmation
                    .compare_exchange(UNCONFIRMED, CONFIRMED, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
                    && !state.pending
                {
                    self.transition(AgentStatus::Waiting);
                }
            }
            Event::Working => {
                state.pending = true;
                self.transition(AgentStatus::Working);
            }
            Event::Context { tokens } => self.context(tokens),
            Event::Settled if self.confirmed() && !state.compacting => {
                state.pending = false;
                self.transition(AgentStatus::Waiting);
            }
            Event::Compaction {
                completed,
                failed,
                reason,
            } => self.compaction(state, completed, failed, reason.as_deref()),
            Event::Unsupported => self.fail(),
            _ => {}
        }
    }

    fn compaction(&self, state: &mut State, completed: bool, failed: bool, reason: Option<&str>) {
        if failed {
            state.compacting = false;
            return;
        }
        if completed && !state.compacting {
            return;
        }
        if !completed && state.compacting {
            return;
        }
        state.compacting = !completed;
        if completed {
            self.clear_context();
        } else {
            self.transition(AgentStatus::Working);
            if reason == Some("overflow") {
                self.publish_activity(AgentActivity::TaskUpdate {
                    summary: "Pi is recovering from context overflow".into(),
                });
            }
        }
        self.publish_activity(AgentActivity::Compaction { completed });
    }

    fn publish_activity(&self, activity: AgentActivity) {
        if let Some(sender) = self.activity.lock().unwrap().as_ref() {
            let _ = sender.send(ActivityReport::own_work(activity));
        }
    }

    fn context(&self, tokens: Option<u64>) {
        match tokens {
            Some(tokens) if tokens <= crate::compaction::MAX_SAFE_TOKENS => {
                super::session::publish_context(
                    &self.status,
                    TurnContext {
                        context_tokens: tokens,
                        cache_read_tokens: 0,
                    },
                );
            }
            None => self.clear_context(),
            _ => self.fail(),
        }
    }

    fn clear_context(&self) {
        self.status.send_modify(|snapshot| {
            snapshot.context = None;
            snapshot.context_reset_count = snapshot.context_reset_count.saturating_add(1);
        });
    }

    fn transition(&self, status: AgentStatus) {
        if self.draining.load(Ordering::Acquire) && !matches!(status, AgentStatus::Ended { .. }) {
            return;
        }
        self.status.send_if_modified(|snapshot| {
            if let Some(next) = snapshot.transition(status) {
                *snapshot = next;
                true
            } else {
                false
            }
        });
    }
}

fn unsupported(policy: &AppliedCompactionPolicy, reason: &str) -> AppliedCompactionPolicy {
    let mut policy = policy.clone();
    policy.enforcement = CompactionEnforcement::Unsupported;
    policy.reason = Some(reason.into());
    policy.capabilities.between_turn_compact = false;
    policy.capabilities.unsupported_reason = Some(reason.into());
    policy
}

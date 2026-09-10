//! The application layer: an orchestrator-backed RPC carried over the E2EE relay.
//!
//! This is what the browser actually talks to. A decrypted request frame carries
//! `{ "method": "task.dispatch", "id": "...", "params": {...} }`; the handler runs
//! the corresponding [`Orchestrator`] operation and returns
//! `{ "id": "...", "ok": true, "result": {...} }` (or `ok: false` + `error`).
//!
//! For local QA without a real LLM, a deterministic *scripted agent* stands in for
//! the harness: on dispatch/approval it writes the plan/code files an agent would
//! and reports `done`, so the whole lifecycle (plan → build → review → merge) runs
//! end to end. In production the harness is a real CLI in a PTY reporting `done`
//! over MCP; the orchestrator code path is identical.

mod board;
mod captures;
mod config;
mod fs;
mod git;
mod issues;
mod mcp;
mod projects;
mod rpc;
mod rtc;
mod runs;
mod runtime;
mod streams;
mod transactions;
mod worktrees;

#[cfg(test)]
pub(in crate::app) use self::board::cache::EXTERNAL_SCAN_INTERVAL;
pub(in crate::app) use self::board::cache::{
    scan_may_yet_show_it, DiffCacheEntry, DiffCacheKey, DiffComputeObserver, ExternalScanCache,
    ExternalWorktreeRows, PRIMARY_SUMMARY_TTL,
};
pub(in crate::app) use self::board::views::{working_time_json, EntitylessRow};
#[cfg(test)]
pub(in crate::app) use self::fs::FS_READ_MAX_BYTES;
pub(in crate::app) use self::git::deferred::{DeferredGit, GitCallScope, ScopedGitCall};
pub(in crate::app) use self::git::diff_file_rows;
pub(in crate::app) use self::issues::documents::{attach_plan_operation_turn, comment_json};
pub(in crate::app) use self::issues::scheduler::scheduler_request;
pub(in crate::app) use self::issues::sessions::PlanDraftingStarted;
pub use self::issues::sessions::{PlanWorkspaceOpened, PlanWorkspaceRefused};
pub(in crate::app) use self::issues::views::{
    dispatchable_next_run_stage, next_unsettled_stage, plan_stage_json, plan_state_str,
    stage_doc_state_str,
};
pub(in crate::app) use self::mcp::AddressedSession;
#[cfg(test)]
pub(in crate::app) use self::mcp::MCP_CONTROL_METHOD;
#[cfg(test)]
pub(in crate::app) use self::mcp::{
    authenticated_mcp_owner, bind_done_listener, serve_done_listener,
};
#[cfg(test)]
pub(in crate::app) use self::rpc::dispatch_frame;
pub(in crate::app) use self::rpc::{
    entity_ids_of, err, optional_nonempty_string, require_array, require_str, require_value,
};
pub(in crate::app) use self::rtc::{rtc_close, rtc_ice, rtc_offer};
pub(in crate::app) use self::runs::lifecycle::PlannedRunFinish;
pub use self::runs::lifecycle::{
    ImplementationAdopted, ImplementationOpened, ImplementationRefused, RunAdopted,
    RunAdoptionSettled,
};
pub(in crate::app) use self::runs::reporting::{
    abandoned_branch_summary, append_plan_stage_announcements, close_abandoned_run_conversations,
    open_session_id, record_current_stage_started, record_idle_in_thread, record_report_in_thread,
    recovery_agent_prompt, HarnessExit,
};
#[cfg(test)]
pub(in crate::app) use self::runs::reporting::{out_of_phase_log, run_outcome_mirrors_to_issue};
#[cfg(test)]
pub(in crate::app) use self::runs::review::{merge_cleanup_from, MergeCleanup};
pub(in crate::app) use self::runs::views::{
    diff_file_edited_at, diff_json, run_state_str, worktree_diff_json,
};
#[cfg(test)]
pub(in crate::app) use self::runtime::agents::endpoints::agent_is_working;
pub(in crate::app) use self::runtime::agents::endpoints::{
    activity_event_kind, agent_attach, agent_start, has_agent_choice, model_choice_from,
    named_agent_id, AgentSpawnRequest, DigestScope,
};
pub(in crate::app) use self::runtime::agents::records::{
    record_activity, PumpWake, SelfReport, NO_ANSWER_SESSION_ENDED, SESSION_DIED_SUMMARY,
};
#[cfg(test)]
pub use self::runtime::deferred::OffLockGate;
#[cfg(test)]
pub(in crate::app) use self::runtime::deferred::OffLockGateHandle;
pub(in crate::app) use self::runtime::deferred::{
    DeferredRead, DeferredWork, OffLockJob, ProjectListRow, ReadSubject,
};
pub(in crate::app) use self::runtime::delivery::preflight::{
    chosen_option_id, deliver, NEW_THREAD_MESSAGES_PROMPT, WORKING_INDICATOR_NOTICE,
};
pub(in crate::app) use self::runtime::delivery::runner::{DeliveryRunner, AGENT_DELIVERY_METHOD};
pub(in crate::app) use self::runtime::delivery::types::{
    DeliveryOutcome, DeliveryPreflight, ImplementationTarget, PendingAgentTurn, PendingTurns,
    TurnText, AGENT_START_DECLINED_SESSION_OVER, NO_TERMINAL_LEFT, SPAWN_NEVER_OPENED,
    TAB_CLOSED_UNDER_A_TURN,
};
pub use self::runtime::lifecycle::{DiscardSettlement, ImplementationCaller, PlanSessionOpening};
pub(in crate::app) use self::runtime::pumps::{
    capture_conversation_names, spawn_tab_pumps, still_pumping_instance,
};
#[cfg(test)]
pub(in crate::app) use self::runtime::pumps::{
    end_of_session, spawn_activity_pump, spawn_status_pump,
};
pub use self::runtime::recovery::RestoredCheckout;
pub(in crate::app) use self::runtime::recovery::{
    archived_worktree_json, load_stored_tasks, merge_archived_worktree_facts,
    reconcile_missing_run_worktree, record_session_death_in_thread, StagePublicationQuery,
    StagePublications,
};
#[cfg(test)]
pub(in crate::app) use self::runtime::recovery::{
    classify_stage_publication, unregistered_restore_for,
};
pub(in crate::app) use self::runtime::sessions::{
    agent_tab_id, build_agent, default_resume_id_probe, default_session_locator_factory,
    IdleObservation, LifecycleDiagnostic, McpTokenLease, SessionRegistry, SpawnAvailability,
    SpawnClaimToken, Tab, TabKey, TabPumps, TabRole,
};
#[cfg(test)]
pub(in crate::app) use self::runtime::spawning::nudge_live_agent_tab;
#[cfg(test)]
pub(in crate::app) use self::runtime::spawning::{
    agent_open_request, inherit_waiting_clients, reserve_agent_spawn, SpawnClaim, AGENT_SPAWN_WAIT,
};
pub(in crate::app) use self::runtime::spawning::{
    ensure_agent_tab, issue_session, open_session_lineage, SettlingHandle, Spawned,
};
pub(in crate::app) use self::runtime::terminals::{
    attach_to_tab, attach_view, no_terminal_here, session_hello, term_ack, term_attach,
    term_create, term_input, term_resize, TabFacts, TermScope,
};
#[cfg(test)]
pub(in crate::app) use self::runtime::terminals::{
    require_shell_kind, shell_harness_spec, terminal_size, MAX_USER_TERMINALS,
};
pub(in crate::app) use self::streams::{sha256_hex, stream_start, StreamState};
pub use self::worktrees::dispatch::{BranchDispatched, BranchJoined};
#[cfg(test)]
pub(in crate::app) use self::worktrees::finish::run_finish_git_steps;
pub(in crate::app) use self::worktrees::finish::{
    finish_git_steps_are_complete, parse_worktree_finish_action, BranchFinishEpilogue,
    FinishEpilogue, FinishKind, FinishRequirement, PlannedFinish, RunFinishEpilogue,
    WorktreeFinishJob, WorktreeFinishOutcome,
};
pub use self::worktrees::WorktreeCreated;

#[cfg(test)]
use crate::orchestrator::Orchestrator;
#[cfg(test)]
use crate::templates::Templates;
use captures::RouteRecorded;
pub use captures::RoutedCapture;
#[cfg(test)]
use captures::{capture_after_routing, RoutedIssueDrafting};
pub use config::ConfigError;
pub(crate) use config::{announce_isolation_downgrade, expand_tilde};
use config::{default_state_root, DEFAULT_HARNESS};
#[cfg(test)]
use config::{read_config, ConfigPersistStep};
use projects::{default_projects_dir, Project, ProjectRegistry};
pub use projects::{ProjectAdded, ProjectRemoteSet};

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
#[cfg(test)]
use std::time::Duration;

#[cfg(test)]
use portable_pty::PtySize;
#[cfg(test)]
use serde_json::json;
use serde_json::Value;
#[cfg(test)]
use tokio::sync::broadcast;

#[cfg(test)]
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

use crate::agent_modes::AgentModes;
#[cfg(test)]
use crate::carrier::{FrameHandler, SessionSender};
use crate::changes::{ChangeBus, DEFAULT_COALESCE_WINDOW};
#[cfg(test)]
use crate::encoding::b64decode;
use crate::harness::HarnessContext;
#[cfg(test)]
use crate::harness::{
    harness_for, AgentStatus, SessionOpenRequest, SessionOutput, TerminalOpenOptions,
    TurnChoiceSupport,
};
use crate::isolation::Isolation;
#[cfg(test)]
use crate::lifecycle::BranchDispatchStep;
#[cfg(test)]
use crate::lifecycle::WorktreeLifecycleJob;
#[cfg(test)]
use crate::mcp::{BridgeAction, DoneOutputs, DonePhase, DoneReport, DoneStatus};
use crate::models::{AgentProvider, ModelChoice};
use crate::notify::{Notifier, NotifyThrottle};
use crate::orchestrator::{ActivePlan, ActiveRun, Agent, ResumeIdProbe, SessionLocatorFactory};
#[cfg(test)]
use crate::orchestrator::{
    AgentTurn, ImplementableIssue, PreparedAgentLaunch, RunSource, SpawnOptions,
};
#[cfg(test)]
use crate::plan::{ImplementationActivity, PlanId, PlanState};
#[cfg(test)]
use crate::pty::HarnessSpec;
use crate::rtc::{NoPeerFactory, SessionPeers};
#[cfg(test)]
use crate::run::ValidationReport;
#[cfg(test)]
use crate::run::{
    PublicationAttempt, RunId, RunState, StageProgress, StageProgressState, StagePublication,
};
#[cfg(test)]
use crate::screen::ScreenHandle;
#[cfg(test)]
use crate::screen::TERM_FLUSH_MS;
#[cfg(test)]
use crate::store::{
    now_rfc3339, PersistedPlan, PersistedRun, WorktreeFinishAction, WorktreeFinishStatus,
};
use crate::store::{PersistedArchivedWorktree, Store};
pub use crate::terminal_environment::{capture_login_path, resolve_term_shell};
#[cfg(test)]
use crate::thread::{SessionInstance, ThreadDetail};
use crate::timing::FrameClock;
#[cfg(test)]
use crate::timing::FrameTimer;
#[cfg(test)]
use crate::transport::{self, Frame};
#[cfg(test)]
use crate::worktree::{git_remote_origin, git_stdout, WorktreeManager};
pub(crate) use crate::{encoding::b64encode, fs_scope::fenced_scope_path};

mod conversations;
mod qa;
pub use conversations::ATTACHMENT_MAX_BYTES;
use conversations::{
    append_user_thread_messages, apply_thread_action, locate_conversations, media_mime_hint,
    mime_hint, parse_thread_inputs, thread_cursor, thread_detail, view_thread_detail,
    with_post_receipt, ReadReport,
};

/// Shared application state behind the relay handler.
pub struct AppState {
    /// Registered projects and the entity bindings that route work to them.
    projects: ProjectRegistry,
    worktrees_root: std::path::PathBuf,
    /// Where cloned repos land and the directory browser starts; user-configurable.
    projects_dir: std::path::PathBuf,
    /// The harness a new agent is created on when nobody names one. An agent is
    /// locked to its harness for life, so this is asked once on the Account
    /// page and spent at creation — never re-read to move an agent that
    /// already exists.
    default_harness: AgentProvider,
    /// Independent launch presentation for each agent family.
    agent_modes: AgentModes,
    /// How a new checkout is isolated from the project it comes from, for
    /// every project that names no isolation of its own. Spent at creation,
    /// like `default_harness`: an existing checkout says what it is itself.
    isolation: Isolation,
    /// Whether completed diffs automatically receive a review-prioritization pass.
    /// Missing from older configs means off, so upgrading never starts new agent work.
    triage_enabled: bool,
    /// Where to persist the projects + settings, if persistence is enabled.
    config_path: Option<std::path::PathBuf>,
    #[cfg(test)]
    config_persist_failure: Option<ConfigPersistStep>,
    agent: Agent,
    harness: String,
    /// Project-scoped plans, keyed by `plan_id`.
    plans: HashMap<String, ActivePlan>,
    /// Worktree-scoped runs, keyed by `run_id`.
    runs: HashMap<String, ActiveRun>,
    /// Durable plan/run records under the bridge state dir, if persistence is
    /// enabled.
    store: Option<Store>,
    /// What the user said, before anything decided where it goes, keyed by
    /// capture id. Durable from the moment it is taken — the router runs after
    /// the write, never instead of it.
    captures: HashMap<String, crate::capture::Capture>,
    /// The routing decisions in flight, keyed by the capture each one is about.
    /// Single-flight per capture: one router at a time decides where one thing
    /// the user said goes, however many times something asks for it to.
    router_sessions: HashMap<String, crate::router::RouterSession>,
    /// Build's own state directory, fixed at construction. Router scratch is
    /// cut here, beside the store; attaching a store validates its parent and
    /// never changes this root.
    state_root: std::path::PathBuf,
    /// The canonical executable fact supplied by [`HarnessContext`]. Every
    /// project orchestrator receives this same path for MCP scaffolding.
    bridge_exe: std::path::PathBuf,
    /// The provider/model routing runs on when the config file names one.
    /// `None` is the account default at low effort.
    router_choice: Option<ModelChoice>,
    /// Finished external worktrees keyed by their stable path-derived id.
    /// Loaded from the store at boot; project association is resolved by the
    /// canonical project path because project ids are re-minted.
    archived_worktrees: HashMap<String, PersistedArchivedWorktree>,
    /// entity id → its RFC 3339 creation time, carried across saves (and restarts).
    entity_created_at: HashMap<String, String>,
    /// entity id → its RFC 3339 last-mutation time (stamped on every mutation).
    entity_updated_at: HashMap<String, String>,
    /// entity id → the RFC 3339 time of its last *state transition* (vs
    /// `entity_updated_at`, which moves on every mutation).
    entity_state_changed_at: HashMap<String, String>,
    /// When the human last touched each entity, and whether they have seen where
    /// it got to — the rail's ordering and colour. Keyed by run id, plan id, or
    /// worktree id alike (a bare worktree has no record of its own).
    attention: HashMap<String, crate::attention::Attention>,
    /// thread id → how far that conversation had got when a mutation tail last
    /// looked at it. A push fires when an attention-class item lands past it.
    /// Keyed by conversation rather than by entity because a planned run and
    /// its Issue share one thread, and one piece of news is one notification.
    conversation_attention_sequence: HashMap<String, u64>,
    /// entity id → the wire state string last seen by a mutation tail, so
    /// `entity_state_changed_at` only moves on real transitions.
    entity_last_state: HashMap<String, String>,
    /// run id → cached `board.list` diffstat, so the poll surface never runs
    /// per-run git work more than once per TTL window.
    run_stat_cache: HashMap<String, (std::time::Instant, Value)>,
    /// run id → when this run's files were last seen to change (RFC 3339).
    ///
    /// The diff cache above IS the watcher: its numbers are recomputed from the
    /// checkout on a cadence, and two consecutive computes disagreeing means
    /// work landed on disk. Stamping it there costs one comparison of values
    /// already in hand — a real per-checkout watcher would cost a file handle
    /// per worktree and a thread to drain it. Derived, so it is not persisted:
    /// after a restart a run dates itself by its HEAD commit and its
    /// conversation until the next change is observed.
    run_files_changed_at: HashMap<String, String>,
    /// Diff-cache entries with a refresh running right now. Single-flight: a
    /// poll that finds one of these stale serves the value it has and adds no
    /// second worktree scan to the disk.
    diff_refreshes_in_flight: std::collections::HashSet<DiffCacheKey>,
    /// Of those, the ones a mutation has overtaken — see
    /// [`supersede_diff_refresh`](AppState::supersede_diff_refresh). The claim
    /// is also the right to publish, and these have lost it: what they compute
    /// describes the tree as it was before the mutation, and is dropped rather
    /// than put back on the board.
    diff_refreshes_superseded: std::collections::HashSet<DiffCacheKey>,
    /// Test seam: see [`DiffComputeObserver`]. `None` in production.
    diff_compute_observer: Option<DiffComputeObserver>,
    /// Whether a [`sweep_vanished_runs`](AppState::sweep_vanished_runs) is
    /// deciding right now. Single-flight, for the same reason a diff refresh
    /// is: the board polls faster than a fetch per stage returns.
    vanished_run_sweep_in_flight: bool,
    /// Test seam: see [`OffLockGate`]. `None` in production.
    #[cfg(test)]
    off_lock_gate: Option<OffLockGate>,
    /// Project-list-only gate, kept separate so a lifecycle test's global git
    /// gate does not also stop the unrelated project.list probe it uses to
    /// prove the mutex is free.
    #[cfg(test)]
    off_lock_project_list_gate: Option<OffLockGate>,
    /// The git work a verb handed to the drain, to run with this mutex
    /// released. Set by exactly one verb per dispatch and taken by the drain
    /// in the same breath, so the `Ok` the verb returned meanwhile is a
    /// placeholder no client ever sees.
    ///
    /// DELIBERATE, and the same split as [`AppState::delivery_queue`]:
    /// under the lock a verb DECIDES (validates, claims the checkout,
    /// snapshots the paths), and the drain — [`dispatch_frame`], or
    /// [`AppState::dispatch`] itself where there is no `Arc` to release
    /// through — DOES the git with the lock free. A `git worktree remove` of a
    /// six-gigabyte checkout takes minutes and a `git status` there takes
    /// seconds; every other frame, every terminal pump and the relay's own
    /// read loop need this mutex while they run.
    deferred_work: Option<DeferredWork>,
    /// Rows a lifecycle verb has claimed and not yet settled: the board's
    /// carrier for a checkout being cut or discarded right now, and the claim
    /// that keeps a second verb off the same name, branch or checkout while its
    /// git runs. Never persisted — everything one leaves behind on a crash is
    /// re-derived by the scan (see `Bridge Concurrency Primitives.md` §5).
    pending_rows: Vec<Arc<crate::lifecycle::PendingRow>>,
    /// Checkouts whose finish is running right now with the mutex released.
    /// A finish is the one verb whose git work outlives its lock hold, so the
    /// checkout it acts on is claimed here for the duration: a second finish
    /// of the same checkout refuses cleanly instead of racing the first one's
    /// branch delete and worktree removal.
    finishing_worktrees: std::collections::HashSet<String>,
    /// Tests only: read every diff cache as aged out, so a stale-poll test does
    /// not have to sleep out a ten-second TTL.
    #[cfg(test)]
    force_stale_diff_caches: bool,
    /// Tests only: see [`BranchDispatchStep`]. `None` everywhere else.
    #[cfg(test)]
    dispatch_fault: Option<BranchDispatchStep>,
    /// The shell user terminals spawn (resolved once; see [`resolve_term_shell`]).
    term_shell: String,
    streams: HashMap<String, StreamState>,
    /// Every live PTY the daemon owns — the human's shells and each worktree's
    /// one agent alike — keyed by (canonical worktree root, tab id). One
    /// registry over one id space: there is no second place a terminal can be,
    /// so no verb has to ask which kind of thing an id names before serving it.
    session_registry: SessionRegistry,
    /// Turns queued by the verbs running under the state lock, drained by
    /// [`dispatch_frame`] once that lock is free. The synchronous test entry
    /// point ([`AppState::handle`]) has no `Arc` to deliver over, so it leaves
    /// the queue for the test to inspect instead.
    ///
    /// DELIBERATE DIVERGENCE — do not "fix" this back into an inline `deliver`
    /// at each verb. A verb runs holding this lock; a delivery spawns a harness
    /// and waits seconds on its readiness, and every terminal pump needs the
    /// same lock to make progress, so delivering inline deadlocks the daemon
    /// for as long as the spawn takes. The split is the contract: under the
    /// lock a verb RECORDS what to say (a `PendingAgentTurn`), and
    /// [`DeliveryRunner`] SAYS it on a thread of its own, after the frame that
    /// queued it has answered. The queue's in-flight counters let the idle
    /// sweep distinguish an agent on its way from one that never arrived.
    delivery_queue: self::runtime::delivery::queue::DeliveryQueue,
    /// Operation receipts cached only for Store-free execution, plus the one
    /// acceptance awaiting its canonical owner persistence. SQLite remains
    /// authoritative whenever configured.
    operation_ledger: self::conversations::operation_ledger::OperationLedger,
    /// Weak self-handle set once at [`AppState::shared`] time, so `&mut self`
    /// hooks can spawn pump tasks that need the `Arc`. Dispatch paths that run
    /// in tests without an Arc simply skip pump spawning (they assert on
    /// state, not pushes).
    self_handle: Option<std::sync::Weak<Mutex<AppState>>>,
    next_stream: u64,
    /// When true, simulate the agent deterministically (local QA, no LLM).
    qa_agent: bool,
    /// Builds the watcher that names the conversation a spawning session is
    /// having — consulted once per agent-tab spawn, before the child exists.
    /// Never builds one in QA mode: the scripted harness has no transcript
    /// tree to watch.
    session_locator_factory: SessionLocatorFactory,
    /// Whether a recorded resume id still names a conversation the provider
    /// holds — consulted at every spawn that has one to spend, so a dead one is
    /// cleared where it is read instead of costing a session to find out.
    resume_id_probe: ResumeIdProbe,
    /// Web-push notifier for attention transitions, if configured. Content-free
    /// by contract — it only ever says "a task needs you".
    notifier: Option<Notifier>,
    /// At most one push per task-state change.
    notify_throttle: NotifyThrottle,
    /// Which peer connection each E2EE session has (spec §Signaling), and the
    /// factory that builds them. The bridge is always the answerer, so there is
    /// nothing here until a browser offers; a bridge with no peer transport
    /// built in refuses every offer and its clients keep working over the relay
    /// carrier.
    peers: Arc<SessionPeers>,
    /// Push invalidation: every browser session that asked to be told when
    /// state moves, and the changes waiting to reach them.
    ///
    /// Held as an `Arc` behind its own leaf mutexes rather than as plain
    /// fields, because the two halves run in opposite places: mutations NOTE
    /// changes holding this state's mutex, and the flusher SENDS them holding
    /// no lock at all. See [`crate::changes`].
    changes: Arc<ChangeBus>,
    /// What every frame's four durations are recorded against.
    ///
    /// It lives on the state rather than beside it because the state is what
    /// every path that takes this mutex can already reach: the relay's frame
    /// handler, the MCP done socket, and the delivery path both of them share.
    /// A clock built per handler would leave the socket's frames, which have
    /// no handler behind them, counted nowhere.
    /// Answering `bridge.stats` never goes through here — a frame parked on
    /// this mutex is exactly when the counters are needed, so the frame reads
    /// them off the clock its own timer holds.
    frame_clock: Arc<FrameClock>,
}

impl AppState {
    pub fn new(
        repo_path: impl Into<std::path::PathBuf>,
        worktrees_root: impl Into<std::path::PathBuf>,
        base_branch: impl Into<String>,
        qa_agent: bool,
        mcp_socket: impl Into<String>,
    ) -> Self {
        let context = HarnessContext::resolved(mcp_socket.into().into(), default_state_root())
            .expect("resolve the default harness context");
        Self::new_with_context(
            Some(repo_path.into()),
            worktrees_root.into(),
            base_branch.into(),
            qa_agent,
            context,
        )
    }

    pub fn new_configured(
        repo_path: impl Into<std::path::PathBuf>,
        worktrees_root: impl Into<std::path::PathBuf>,
        base_branch: impl Into<String>,
        qa_agent: bool,
        context: HarnessContext,
    ) -> Self {
        Self::new_with_context(
            Some(repo_path.into()),
            worktrees_root.into(),
            base_branch.into(),
            qa_agent,
            context,
        )
    }

    fn new_with_context(
        repo_path: Option<std::path::PathBuf>,
        worktrees_root: std::path::PathBuf,
        base_branch: String,
        qa_agent: bool,
        context: HarnessContext,
    ) -> Self {
        let harness = if qa_agent { "QA agent" } else { "Claude Code" }.to_string();
        let session_locator_factory: SessionLocatorFactory = if qa_agent {
            Arc::new(|_, _| None)
        } else {
            default_session_locator_factory()
        };
        let state_root = context.state_root.clone();
        let bridge_exe = context.bridge_exe.clone();
        let agent = build_agent(qa_agent, context);
        let mut state = AppState {
            projects: ProjectRegistry::new(),
            worktrees_root,
            projects_dir: default_projects_dir(),
            default_harness: DEFAULT_HARNESS,
            agent_modes: AgentModes::from_legacy_default(DEFAULT_HARNESS),
            isolation: Isolation::default(),
            triage_enabled: false,
            config_path: None,
            #[cfg(test)]
            config_persist_failure: None,
            agent,
            harness,
            plans: HashMap::new(),
            runs: HashMap::new(),
            store: None,
            captures: HashMap::new(),
            router_sessions: HashMap::new(),
            state_root,
            bridge_exe,
            router_choice: None,
            archived_worktrees: HashMap::new(),
            entity_created_at: HashMap::new(),
            entity_updated_at: HashMap::new(),
            entity_state_changed_at: HashMap::new(),
            attention: HashMap::new(),
            conversation_attention_sequence: HashMap::new(),
            entity_last_state: HashMap::new(),
            run_stat_cache: HashMap::new(),
            run_files_changed_at: HashMap::new(),
            diff_refreshes_in_flight: std::collections::HashSet::new(),
            diff_refreshes_superseded: std::collections::HashSet::new(),
            vanished_run_sweep_in_flight: false,
            diff_compute_observer: None,
            #[cfg(test)]
            off_lock_gate: None,
            #[cfg(test)]
            off_lock_project_list_gate: None,
            deferred_work: None,
            pending_rows: Vec::new(),
            finishing_worktrees: std::collections::HashSet::new(),
            #[cfg(test)]
            force_stale_diff_caches: false,
            #[cfg(test)]
            dispatch_fault: None,
            term_shell: resolve_term_shell(),
            streams: HashMap::new(),
            session_registry: SessionRegistry::new(),
            delivery_queue: Default::default(),
            operation_ledger: Default::default(),
            self_handle: None,
            next_stream: 1,
            qa_agent,
            session_locator_factory,
            resume_id_probe: default_resume_id_probe(),
            notifier: None,
            notify_throttle: NotifyThrottle::default(),
            peers: SessionPeers::with_factory(Arc::new(NoPeerFactory)),
            changes: ChangeBus::new(DEFAULT_COALESCE_WINDOW),
            frame_clock: FrameClock::new(),
        };
        if let Some(repo_path) = repo_path {
            state.add_project(repo_path, base_branch);
        }
        state
    }

    /// Like [`AppState::new`] but with no default project: projects arrive only
    /// through the UI (`project.add`/`project.clone`) and persisted config. This
    /// is the end-user path — a default like `/repo` would register a phantom
    /// project on machines where that path never existed.
    pub fn new_unrooted(
        worktrees_root: impl Into<std::path::PathBuf>,
        base_branch: impl Into<String>,
        qa_agent: bool,
        mcp_socket: impl Into<String>,
    ) -> Self {
        let context = HarnessContext::resolved(mcp_socket.into().into(), default_state_root())
            .expect("resolve the default harness context");
        Self::new_with_context(
            None,
            worktrees_root.into(),
            base_branch.into(),
            qa_agent,
            context,
        )
    }

    pub fn new_unrooted_configured(
        worktrees_root: impl Into<std::path::PathBuf>,
        base_branch: impl Into<String>,
        qa_agent: bool,
        context: HarnessContext,
    ) -> Self {
        Self::new_with_context(
            None,
            worktrees_root.into(),
            base_branch.into(),
            qa_agent,
            context,
        )
    }

    /// Enable web-push attention notifications: every task-state change into a
    /// state that needs the human fires one signed, content-free notify at the api.
    pub fn with_notifier(mut self, notifier: Notifier) -> Self {
        self.notifier = Some(notifier);
        self
    }

    /// Enable durable task persistence at `dir` and recover every stored task:
    /// re-attach tasks whose worktrees survived, surface tasks that were mid-phase
    /// when the daemon died as `interrupted`, and abandon tasks whose worktrees are
    /// gone. A corrupt task file is a hard error naming the file — boot fails
    /// rather than silently dropping a task.
    pub fn with_task_store(mut self, dir: impl Into<std::path::PathBuf>) -> Result<Self, String> {
        let dir = dir.into();
        let parent = dir
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| std::path::Path::new("."));
        let resolved_parent = std::fs::canonicalize(parent)
            .map_err(|error| format!("resolve task store parent {}: {error}", parent.display()))?;
        if resolved_parent != self.state_root {
            return Err(format!(
                "task store parent {} does not match configured state root {}",
                resolved_parent.display(),
                self.state_root.display()
            ));
        }
        let stored = load_stored_tasks(dir)?;
        self.restore_stored_tasks(stored)?;
        Ok(self)
    }

    /// Share this state so the relay handler and the done-socket listener both
    /// drive the same tasks. Stashes a weak self-handle so `&mut self` hooks
    /// can spawn pump tasks (see the `self_handle` field).
    pub fn shared(self) -> Arc<Mutex<AppState>> {
        let state = Arc::new(Mutex::new(self));
        let changes = {
            let mut app = state.lock().unwrap();
            app.self_handle = Some(Arc::downgrade(&state));
            Arc::clone(&app.changes)
        };
        // The flusher runs on a task of its own and never takes this mutex —
        // that is the whole reason the bus is not a field it would have to
        // lock. A build with no runtime under it (the synchronous unit tests)
        // gets no flusher and simply never sends.
        ChangeBus::spawn_flusher(changes);
        state
    }

    /// The push-invalidation bus — how a frame handler subscribes the session it
    /// is serving, and how the flusher finds its subscribers.
    pub fn changes(&self) -> Arc<ChangeBus> {
        Arc::clone(&self.changes)
    }

    /// Tests only: coalesce over a shorter window, so a push test does not have
    /// to sleep out the production one. Must precede [`AppState::shared`] —
    /// that is where the flusher takes its handle.
    #[cfg(test)]
    fn with_change_window(mut self, window: Duration) -> Self {
        self.changes = ChangeBus::new(window);
        self
    }
}

#[cfg(test)]
mod tests;

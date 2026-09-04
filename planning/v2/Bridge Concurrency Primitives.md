# Bridge Concurrency Primitives

Status: design, 2026-09-04. The component list for `Bridge Concurrency Spec.md`,
branch `fix/bridge-concurrent-requests`. Read with the spec open; where they
disagree the spec wins. It says what must stop happening — this names the five
components built so it stops, one per step.

## The one rule every component obeys

**The `AppState` mutex is held only for in-memory bookkeeping.** Nothing that
can block on a process, a pipe, a socket, a filesystem walk, or a sleep runs
while it is held. Every component below is one job split the same way — decide
under the lock, run with it released, apply under it again.

## Lock order, daemon-wide

The app mutex is above every other lock. `FrameClock` counters (§1) are taken
under it only to stamp the current holder; `ChangeBus` is a leaf, taken with it
released. `ScreenHandle` (§2) is absent from the order because it is never
nested: the app mutex resolves the handle, releases, and only then is the screen
locked.

## 1. `FrameClock` — the timing guard

- **Boundary** `bridge/src/timing.rs` (new), between the relay's worker pool and
  the app. The only thing that knows how long anything took, and nothing about
  what a verb means. `FrameHandler` becomes
  `Arc<dyn Fn(SessionSender, Frame, FrameTimer) -> Value + Send + Sync>`: the
  queue wait is `relay::run_job`'s, the lock wait `dispatch_frame`'s, and one
  record must carry both.
- **Interface**

  ```rust
  pub struct FrameClock;                     // shared: Arc<FrameClock>
  impl FrameClock {
      pub fn frame(self: &Arc<Self>, method: &str, queued_for: Duration) -> FrameTimer;
      pub fn stats(&self) -> Value;          // the `bridge.stats` reply
  }
  pub struct FrameTimer;                     // one frame's record, published on Drop
  impl FrameTimer { pub fn lock<'a>(&self, s: &'a Arc<Mutex<AppState>>) -> LockedFor<'a>; }
  pub struct LockedFor<'a>;                  // Deref/DerefMut to AppState
  ```

- **Hides** the clocks, the per-method p50/p95/max reservoirs, the slow-frame log
  line (over 200 ms: method, four durations, queue depth), the holder slot, the
  queue depth. Nothing else in the daemon reads an `Instant`.
- **Replaces** every bare `state.lock().unwrap()` on the dispatch path
  (`dispatch_frame`, `warm_diff_caches`, `term_*`, `agent_attach`) with
  `timer.lock(state)`; `LockedFor` derefs to `AppState`, so nothing else moves.
- **Lock discipline** `LockedFor` declares its `MutexGuard` first, so hold time
  and the histogram are recorded after release; only the holder stamp is written
  under the app mutex. `bridge.stats` answers from `FrameClock` alone — a wedged
  daemon must still say who is wedging it.
- **Tests** `a_slow_frame_logs_its_four_durations`,
  `bridge_stats_answers_while_another_frame_holds_the_app_mutex`.

## 2. `ScreenHandle` — the per-tab screen

- **Boundary** `bridge/src/screen.rs` (new; `TermScreen`, `AttachedClient` and
  the flow-control constants move out of `app.rs`): one tab's grid and its
  attached clients. The module cannot see `AppState`, which makes "the pump never
  takes the app mutex" structural rather than a habit.
- **Interface**

  ```rust
  pub struct ScreenHandle;                   // Clone; Arc<Mutex<TermScreen>> + term_id
  impl ScreenHandle {
      pub fn new(term_id: &str, cols: u16, rows: u16) -> ScreenHandle;
      pub fn attach(&self, sender: &SessionSender, cols: u16, rows: u16) -> AttachSnapshot;
      pub fn snapshot(&self) -> AttachSnapshot;   // a dead tab's last screen
      pub fn ack(&self, session_id: &str, cursor: u64);
      pub fn detach(&self, session_id: &str);
      pub fn feed(&self, chunk: &[u8]);      // the pump: parse + coalesce
      pub fn flush(&self);
      pub fn restart(&self);                 // new session: fresh parser, same cursor
      pub fn resize(&self, cols: u16, rows: u16);
      pub fn carry_clients_from(&self, waiting: &ScreenHandle);
      pub fn close(&self, reason: &str);
  }
  pub struct AttachSnapshot { snapshot: String, cursor: u64, cols: u16, rows: u16 }
  pub struct TerminalHandle { session: Arc<dyn AgentSession>, screen: ScreenHandle }
  impl TerminalHandle {      // both write with the app mutex released
      pub fn write_input(&self, bytes: &[u8]) -> Result<(), String>;
      pub fn resize(&self, cols: u16, rows: u16) -> Result<(), String>;
  }
  impl Tab { fn terminal_handle(&self) -> Result<TerminalHandle, String>; }
  ```

- **Hides** the vt100 parser, the cursor, the coalescing buffer, the unacked-byte
  budget, the flood-collapse rate limit, dead-sender pruning, every `term.*` push
  shape. `AttachSnapshot` is the one value `term.attach` and `agent.attach` reply
  from, so that JSON exists once.
- **Replaces** `Tab.screen: Option<TermScreen>` → `Option<ScreenHandle>`;
  `agent_screens_awaiting_spawn` → `HashMap<TabKey, ScreenHandle>`;
  `Tab::require_terminal_and_screen` (a borrow of `AppState`) →
  `Tab::terminal_handle` (an owned clone); `close_a_screen_with_no_terminal` →
  `ScreenHandle::close`.
- **Lock discipline** app mutex → resolve `TabKey` → clone the handle →
  **release** → lock the screen; never held together. `spawn_tab_pump` takes the
  app mutex twice in a tab's life — at start to look the handle up, at EOF for
  the death rites (`record_agent_session_end`, tab removal); every chunk and
  flush between is screen-lock only. `term.input`/`term.resize` clone a
  `TerminalHandle`, release, then write, so a pty nobody drains blocks one
  worker. The screen lock is a leaf: `SessionSender::push` is all that runs there.
- **Tests** `a_streaming_pty_never_takes_the_app_mutex`,
  `a_board_read_answers_while_three_screens_are_flooding`,
  `term_input_to_a_pty_that_is_not_draining_leaves_the_app_mutex_free`.

## 3. `DeliveryRunner` — the background delivery runner

- **Boundary** `bridge/src/delivery.rs` (new; `deliver`,
  `deliver_pending_agent_turns` and `ensure_agent_tab` move here), between a
  verb's durable state change and the agent process that hears about it.
- **Interface**

  ```rust
  pub struct PendingTurns(Vec<PendingAgentTurn>);   // what one lock acquisition took
  impl AppState { fn take_pending_turns(&mut self) -> PendingTurns; }
  impl DeliveryRunner {
      pub fn spawn(state: &Arc<Mutex<AppState>>, turns: PendingTurns);  // returns at once
      pub fn run(state: &Arc<Mutex<AppState>>, turns: PendingTurns);    // no runtime: sync tests
  }
  pub struct AgentSpawnGate { in_flight: HashSet<TabKey>, finished: Arc<Condvar> }
  impl AgentSpawnGate {                             // release notifies every waiter
      fn reserve(&mut self, key: &TabKey) -> bool;
      fn release(&mut self, key: &TabKey);
  }
  struct AgentSpawnPlan { spec: HarnessSpec, size: PtySize, carried: Option<ScreenHandle>, .. }
  impl AgentSpawnPlan { fn probe_and_scaffold(self) -> Result<ReadyToSpawn, String>; }
  ```

- **Hides** which half of a turn travels (cold/warm), the readiness wait, the
  `PROMPT_WRITE_EXIT_GRACE` exit race, the in-flight bookkeeping, the
  resume/transcript/locator order — no verb knows a harness exists.
- **Replaces** `dispatch_frame`'s inline `deliver_pending_agent_turns(state)` →
  `DeliveryRunner::spawn`, so the reply goes out as soon as the store write
  lands; `ensure_agent_tab`'s `sleep(25ms)` loop against `AGENT_SPAWN_WAIT` →
  `AgentSpawnGate`'s condvar wait, which releases the app mutex while it waits.
  `scaffold_agent_worktree`, `resume_id_probe`, `transcript_probe` and
  `session_locator_factory` leave the reservation block for `probe_and_scaffold`.
- **Lock discipline** Three acquisitions, no more. **Take**: the queue, the
  in-flight marks and the reserved tab id in one acquisition, exactly as today,
  so the idle sweep never sees a gap. **Run**: probe, scaffold, spawn,
  `send_turn` — none. **Apply**: insert the tab, release the gate,
  `record_agent_session_start` / `record_agent_delivery_failure`,
  `note_entity_changed`. The condvar wait is the one taken with a guard in hand.
  A verb that queues a turn answers with the tab id reserved under the lock; the
  outcome reaches the browser through the entity's push event, not the reply.
- **Tests** `a_message_is_answered_before_its_agent_has_spawned`,
  `a_board_read_completes_while_a_cold_spawn_waits_for_readiness`,
  `two_callers_of_one_tab_spawn_one_harness_without_spinning`.

## 4. `CachedDiff` — the non-blocking cache read

- **Boundary** `bridge/src/app.rs`, beside `DiffCacheKey`: between a poll surface
  and the git work its numbers come from. The only way a verb reads a diff cache.
- **Interface**

  ```rust
  pub enum CachedDiff { Value(DiffCacheEntry), Scanning }
  impl AppState {
      /// Serve what is cached, start the refresh it needs. Never computes.
      fn read_cached(&mut self, refresh: DiffCacheRefresh) -> CachedDiff;
      fn note_worktree_appeared(&mut self, project_id: &str, worktree: ExternalWorktree);
      fn note_worktree_gone(&mut self, project_id: &str, path: &Path);
  }
  impl DiffCacheKey { fn scanning_placeholder(&self) -> Value; }
  ```

- **Hides** staleness, the single-flight claim, the spawn, and the empty answers
  (`scanning_placeholder`: `{"worktrees": [], "scanning": true}` for a scan,
  `null` for a stat and a primary summary). Every miss and every stale entry end
  alike: an answer now, a `spawn_diff_refresh` behind it, `publish_diff_refresh`
  + `note_board_changed` when it lands.
- **Replaces** deletes `FIRST_COMPUTE_WAIT`, `wait_for_first_diff_value`,
  `DiffCacheWork::AwaitFirstValue`, `ClaimedRefresh.blocking` and the `blocking`
  parameter threaded through `claim_diff_refresh` / `diff_cache_work` /
  `stale_run_stat_work`; deletes the compute-of-last-resort in `run_stat` and
  `external_worktrees`, and with it `external_worktrees(.., force: true)` — its
  one caller (`run.adopt`) gets the fresh scan from §5's run phase instead.
  `invalidate_external_scan` → `note_worktree_appeared` / `note_worktree_gone`,
  so a create no longer empties a project's whole scan. `warm_diff_caches` still
  refreshes before the verb reads, but no branch of it waits.
- **Lock discipline** `read_cached` is pure bookkeeping: read the map, maybe
  insert a claim, hand back a value. The compute runs on `spawn_blocking` holding
  nothing; publishing takes the app mutex on its own.
- **Tests** `board_list_answers_scanning_when_nothing_has_ever_been_computed`,
  `a_first_scan_never_runs_under_the_app_mutex`,
  `a_created_worktree_joins_the_scan_cache_instead_of_clearing_it`.

## 5. `WorktreeLifecycleJob` — worktree work off the lock

- **Boundary** `bridge/src/lifecycle.rs` (new; `WorktreeFinishJob` moves here as
  one mutation), between a lifecycle verb's decision and the git that carries it
  out — for create, dispatch, implement, adopt, release, abandon, clone, finish
  and rollback alike.
- **Interface**

  ```rust
  pub struct WorktreeLifecycleJob {
      reservation: Reservation,               // the persisted Creating/Discarding record
      mutation: Box<dyn WorktreeMutation>,
      #[cfg(test)] gate: Option<OffLockGate>,
  }
  impl WorktreeLifecycleJob { pub fn run(self) -> LifecycleOutcome; }

  /// The variation: one impl per verb, chosen at the one construction point.
  pub trait WorktreeMutation: Send {
      fn perform(&self, checkouts: &WorktreeManager, base: &str) -> Result<WorktreeChange, String>;
  }
  pub struct WorktreeChange { appeared: Vec<ExternalWorktree>, gone: Vec<PathBuf>, reply: Value }
  pub struct LifecycleOutcome { reservation: Reservation, result: Result<WorktreeChange, String> }
  impl AppState {
      fn defer_lifecycle(&mut self, job: WorktreeLifecycleJob) -> Value;   // placeholder
      fn apply_lifecycle(&mut self, outcome: LifecycleOutcome) -> Result<Value, String>;
  }  ```

- **Hides** every shell-out and libgit2 call a lifecycle verb makes, the
  scaffold-after-create ordering, the rollback. `WorktreeManager` gains
  `#[derive(Clone)]` (two `PathBuf`s) so the run phase carries its own copy;
  `Orchestrator` stays behind the lock, its `adopt_run` / `abandon_run` being
  bookkeeping over `ActiveRun`.
- **Replaces** `DeferredWork::Finish` collapses into `DeferredWork::Lifecycle`;
  `WorktreeFinishJob` becomes the `Finish` mutation, `FinishEpilogue` a
  `Reservation`; `BranchDispatchCreations` + `undo_branch_dispatch` become
  `Reservation::roll_back`, one rollback for every verb. `worktree_create`,
  `cut_branch_for_dispatch`, `ensure_issue_implementation_worktree`,
  `open_implementation_run`, `plan_create`'s planning worktree, `run_adopt`,
  `run_release`, `run_abandon`, `project_add` and `project_clone` each stop
  calling git and return a job.
- **Lock discipline** decide (validate, reserve the name, persist the
  `Creating`/`Discarding` entity, build the job) under the app mutex; run (the
  git, the scan adoption needs, the scaffold) holding nothing; apply (write the
  result, `note_worktree_appeared` / `note_worktree_gone`, stamp interaction,
  `note_entity_changed`, or roll the reservation back) under it again. The drain
  in `dispatch_frame` is unchanged: it already runs `DeferredWork` between two
  acquisitions.
- **Tests** `worktree_create_runs_git_worktree_add_with_the_state_lock_free`,
  `a_creating_worktree_is_on_the_board_before_its_git_returns`,
  `a_create_that_fails_rolls_its_reservation_back_and_leaves_no_row`.

## What builds no primitive

Step 5 (SPA): the `agent_starting` state goes on the overlay `core/optimistic.js`
already owns, and the four views that read a tab id out of a reply
(`thread.post`, `agent.start`, `branch.dispatch`, `worktree.create`) read the
entity's fields from the next push instead. The spec's load test,
`bridge/tests/concurrency_load.rs`, is the only test that measures a number; the
tests named above prove a lock is free.

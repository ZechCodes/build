# Bridge Concurrency Primitives

Status: design, 2026-09-04. The component list for `Bridge Concurrency Spec.md`,
branch `fix/bridge-concurrent-requests`. Read with the spec open; where they
disagree the spec wins, except the one deviation argued in §5 (`PendingRow` is
not persisted). Five components, one per step, plus the two primitives the
rule itself costs.

## The one rule every component obeys

**The `AppState` mutex is held only for bounded in-memory bookkeeping.** Nothing
whose duration another process, another machine, or a disk decides runs while it
is held: no process wait, no pipe or socket read, no filesystem walk, no git
shell-out or libgit2 call, no sleep. Every component below is one job split the
same way — decide under the lock, run with it released, apply under it again.

Two writes stay under the lock deliberately and the spec cleared them: the
SQLite store (WAL, `synchronous = NORMAL`, sub-millisecond) and `persist`'s
single config write. A filesystem *walk* is not bounded (`transcript_stems`
lists a directory that grows with every conversation); a process wait is not
at all.

The rule costs `Orchestrator` a `#[derive(Clone)]` (two `PathBuf`s, a
`WorktreeManager` of two more, `Templates`, the `Agent` closure, `PtySize`). A
verb clones its project's orchestrator under the lock and calls it with the
lock released; the run and plan state machines stay under the lock, never the
git around them. Two methods that straddle both halves are split:

- `Orchestrator::abandon_run` (orchestrator.rs:2526) is deleted. It is
  `abandon_run_keeping_checkout` (the lifecycle verdict, pure bookkeeping) plus
  `self.worktrees.remove(..)` (worktree.rs:377: `remove_dir_all`, a libgit2
  prune, the branch delete). The verdict stays under the lock; the removal
  becomes §5's `DiscardCheckout`.
- `Orchestrator::adopt_run` (orchestrator.rs:2392) keeps its `ActiveRun`
  construction and loses `commit_all_with_message`, `scaffold_build_dir` and
  its three refusals to §5's `adopt` and `AdoptableCheckout`.

### `Retirement` — ending a tab, off the lock

`AgentSession::end` is `child.kill()` then `child.wait()` for both carriers
(`PtySession::kill_and_reap`, pty.rs:683; `AdkSession::end`, harness/adk.rs:723).
SIGKILL does not land on a child wedged in uninterruptible I/O until that I/O
returns, so `wait()` is an unbounded process wait, and today every one of its
eight callers holds the app mutex: `close_agent_tab` (app.rs:3900), `term_close`
(6013), `reap_orphaned_terminals` (6130), `retire_agent` (8328),
`abandon_router_session` (9567), `ensure_agent_tab`'s dead-tab replacement
(17950) and stale-owner sweep (17973), and `spawn_tab_pump`'s shell EOF (18436).

Seven of the eight take the tab out of the registry, tell its clients, and end
its session. The dead-tab replacement ends the session and **keeps the
screen**: `dead.screen` is `carried` into the new session (app.rs:17949-17952,
18087-18103) and the attached clients are told nothing, which is what keeps a
browser's terminal attached across an agent restart. So there are two calls,
and the signature says which half of the tab the caller keeps:

```rust
// bridge/src/reaper.rs (new)
pub struct Retirement;                    // Clone
impl Retirement {
    /// Kill and reap `session` on a std::thread of its own. Returns before
    /// either has happened. One thread per call, never a queue: a child wedged
    /// in uninterruptible I/O parks its own thread and nobody else's.
    pub fn begin(session: Arc<dyn AgentSession>) -> Retirement;
    /// True once the process is reaped. Callable only with the app mutex
    /// released — it is the wait the rule forbids, made explicit.
    pub fn wait(&self, timeout: Duration) -> bool;
}

impl AppState {
    /// Remove one tab, tell its clients `reason`, and retire its process.
    fn retire_tab(&mut self, key: &TabKey, reason: &'static str) -> Option<Retirement>;
    /// Remove one tab and retire its process, keeping its screen for the
    /// session that replaces it. The clients are told nothing and stay
    /// attached. `ensure_agent_tab`'s dead-tab replacement, and nothing else.
    fn retire_tab_keeping_screen(&mut self, key: &TabKey)
        -> Option<(Retirement, Option<ScreenHandle>)>;
    fn retire_agent_tabs(&mut self, root: &Path) -> Vec<Retirement>;   // was close_agent_tab
}
```

The close push stays in `retire_tab`, under the lock, because it is bounded:
`ScreenHandle::close` is the screen's leaf lock plus one `SessionSender::push`
per client — an encrypt and an unbounded `mpsc` send (relay.rs:135-160) —
which is what every `push_closed` under the app mutex is today. Only the kill
and the wait leave, onto a thread that holds nothing. A `std::thread` and not
a spawned task, because the synchronous unit tests run with no runtime under
them and a tab still has to end there.

Every caller but one drops the receipt: the tab is out of the registry, which
is what stops the agent being addressable. `DiscardCheckout` (§5) waits, for a
reason argued there on its own merits — it is not today's order.

With the kill asynchronous, a replaced session's EOF can arrive after the
replacement tab is in the registry. `spawn_tab_pump` carries the `Arc` of the
session it pumps and at EOF acts only when `Arc::ptr_eq` matches the registry
tab's session; today's pump (app.rs:18396) has no such check, and the
synchronous kill was what made it unnecessary.

## Lock order, daemon-wide

The app mutex is above every other lock. Under it, two leaves may be taken:
`FrameClock`'s counters (§1), to stamp the current holder, and `ChangeBus`'s
`pending` set — `note_board_changed` / `note_entity_changed` (app.rs:4337-4345)
are `AppState` methods called with the app mutex held, by `add_project`
(3644), `store_diff_entry` (4163), `apply_deferred` (5806-5808) and §5's
`apply_lifecycle`. `ChangeBus::note` (changes.rs:157) documents itself as safe
there: one leaf mutex, an insert into a set, a `notify_one`. The flusher never
takes the app mutex, so the order is app mutex → `pending`, never the reverse.

`ScreenHandle` (§2) is a leaf: the app mutex resolves the handle, releases, and
only then is the screen locked, except `retire_tab`'s close push, which locks
the screen under the app mutex as `push_closed` does today — app mutex →
screen, never the reverse. Two screens are never locked at once:
`carry_clients_from` drains the waiting screen under its own lock, releases,
then locks its own. A `Retirement`'s thread locks a child, never `AppState`.
The one lock taken *with* the app mutex in hand is §3's spawn condvar, which
hands the guard back while it waits.

## 1. `FrameClock` — the timing guard

Built. `bridge/src/timing.rs`, wired through `bridge/src/relay.rs` and
`dispatch_frame`; `bridge.stats` answers. What shipped, and where it differs
from the design above.

- **Boundary** `bridge/src/timing.rs`, between the relay's worker pool and the
  app: the queue wait is the dispatcher's, the lock wait and hold are
  `dispatch_frame`'s, and one record carries all four durations.
- **`FrameHandler` is a struct, not a function pointer.** The alias could not
  hold the clock, and the clock has to reach the dispatcher, which mints the
  queue ticket. It carries the closure and the clock together:

  ```rust
  #[derive(Clone)]
  pub struct FrameHandler { clock: Arc<FrameClock>, dispatch: Arc<dyn Fn(SessionSender, Frame, FrameTimer) -> Value + Send + Sync> }
  impl FrameHandler {
      pub fn new(clock: Arc<FrameClock>, dispatch: impl Fn(SessionSender, Frame, FrameTimer) -> Value + Send + Sync + 'static) -> FrameHandler;
      pub fn call(&self, sender: SessionSender, frame: Frame) -> Value;   // never queued
      fn run(&self, queued: QueuedFrame, sender: SessionSender, frame: Frame) -> Value;
  }
  ```

- **`AppState` owns the clock; the handler borrows it.** `AppState::new` builds
  one and `AppState::handler` clones it off the state (app.rs:2082, 4377), so
  every path that can take the app mutex can reach it: the relay's handler,
  rebuilt on each reconnect, and the MCP control socket, which has no handler at
  all. A clock built per handler would give a reconnect a second set of "since
  boot" counters and leave the socket with none.

- **Interface**

  ```rust
  pub struct FrameClock;                     // shared: Arc<FrameClock>
  impl FrameClock {
      pub fn new() -> Arc<FrameClock>;                    // slow frames to stderr
      pub fn reporting_to(sink: SlowFrameSink) -> Arc<FrameClock>;
      pub fn queued(self: &Arc<Self>) -> QueuedFrame;     // joined the dispatch queue
      pub fn frame(self: &Arc<Self>, method: &str) -> FrameTimer;   // never queued
      pub fn stats(&self) -> Value;          // the `bridge.stats` reply
  }
  pub struct QueuedFrame;                    // a place in the queue; Drop leaves it
  impl QueuedFrame { pub fn start(self, method: &str) -> FrameTimer; }
  pub struct FrameTimer;                     // one frame's record, published on Drop
  impl FrameTimer {
      pub fn lock<'a, T>(&'a self, s: &'a Arc<Mutex<T>>) -> LockedFor<'a, T>;
      pub fn clock(&self) -> &Arc<FrameClock>;   // where `bridge.stats` reads
  }
  pub struct LockedFor<'a, T>;               // Deref/DerefMut to T
  ```

  `LockedFor` is generic over what it guards rather than naming `AppState`, so
  `timing.rs` depends on nothing in the daemon and its tests measure a
  `Mutex<u32>`.
- **A frame's total starts when it arrives, not when a worker takes it.** The
  browser's 12 s timer starts when it sends, so a `board.list` that sat 300 ms
  behind seven others is a slow frame however fast its handler was. `total` is
  queue wait plus handler, the histograms record it, and the 200 ms line fires
  on it — which is what makes a backed-up daemon legible instead of reading as
  "every handler was fine".
- **Hides** the clocks, the per-method histogram, the slow-frame line (over
  200 ms: method, four durations, frames still waiting), the holder slot, the
  queue depth. Nothing on the frame path calls `Instant::now` for itself.
- **The record is fixed-size per method.** 18 shared bucket ceilings, a count
  and a max, allocated once the first time a method is seen and reached through
  an `Arc` after that: no allocation per frame. p50/p95 are the bucket ceiling
  the share falls in, capped by the largest frame actually recorded. The method
  name comes off the wire, so the map is bounded at 128 and everything past it
  is counted together under `other`.
- **The slow-frame sink is a strategy object**, `Arc<dyn Fn(&str)>`: the daemon
  passes `eprintln!`, a test passes a buffer and reads the line back. One
  formatter, `slow_frame_line`, and no `#[cfg(test)]` branch in the code that
  ships.
- **Replaces** every bare `state.lock().unwrap()` in `dispatch_frame` and
  everything it calls on the frame's own thread — `session_hello`,
  `stream_start`, `term_create`, `term_attach`, `term_ack`, `agent_attach`,
  `agent_start`, `warm_diff_caches`, and the delivery path
  (`deliver_pending_agent_turns`, `deliver`, `ensure_agent_tab`, which take the
  frame's `&FrameTimer`) — with `timer.lock(state)`. The delivery path is where
  the longest holds are, so leaving it bare would have made this step's
  before/after number for §3 read as no hold to remove, and left `lock_holder`
  saying nobody held the mutex while a spawn's probe and scaffold held it for
  seconds.
- **The MCP control socket is a frame too.** It reaches the same delivery path
  with no relay frame behind it, so it mints its own timer per socket line off
  the state's clock and records under `mcp.control` (app.rs:4454) — a name of
  its own, since it is not a wire method.
- **What is deliberately not timed.** The pump, the diff-refresh publish and the
  idle sweep are background threads: their holds belong to no frame, and a
  timer minted per chunk would count a pump as a frame served.
- **Owns frame latency, not every clock.** The timestamps that decide
  staleness keep their own `Instant`s: `TermScreen.last_flood_snapshot_at`
  (app.rs:242), `Tab.last_delivered_at` (700), `ExternalScanCache.scanned_at`
  (1324), the `(Instant, Value)` stamps on `run_stat_cache` (1923) and
  `primary_summary` (1011).
- **Lock discipline** `LockedFor` holds its `MutexGuard` in an `Option`. Its
  `Drop` clears the holder slot while the mutex is still held — clearing it
  after the release would wipe the claim of whichever frame acquired next —
  then takes the guard out, drops it, and only then records hold time; a `Drop`
  impl runs before its fields drop, so field order alone would record with the
  guard held. The histogram write is `FrameClock`'s own leaf, the one the lock
  order above permits; the holder stamp is the only write made under the app
  mutex. `bridge.stats` answers from `FrameClock` alone — a wedged daemon must
  still say who is wedging it.
- **Tests** in `timing.rs`: `a_slow_frame_logs_its_four_durations`,
  `a_quick_frame_logs_nothing`, `a_frame_is_counted_under_its_own_method`,
  `stats_name_the_method_holding_the_state_lock`,
  `a_queued_frame_counts_against_the_depth_until_it_starts`,
  `a_frames_lock_time_is_summed_across_its_acquisitions`,
  `a_flood_of_invented_methods_collapses_into_one_record`,
  `quantiles_span_the_recorded_frames`,
  `a_method_nobody_has_called_reports_nothing`. In `relay.rs`:
  `stats_count_the_frames_waiting_for_a_worker`,
  `a_frame_that_waited_for_a_worker_counts_the_wait_as_its_own`,
  `folded_reads_cost_the_queue_one_slot`. In `app.rs`:
  `bridge_stats_answers_while_another_frame_holds_the_app_mutex`,
  `bridge_stats_count_every_frame_under_its_own_method`,
  `a_frames_delivery_path_reports_its_hold_and_names_its_method`.

## 2. `ScreenHandle` — the per-tab screen

- **Boundary** `bridge/src/screen.rs` (new; `TermScreen`, `AttachedClient` and
  the flow-control constants move out of `app.rs`): one tab's grid and its
  attached clients. The module cannot see `AppState`, which makes "the pump never
  takes the app mutex" structural, and emits no reply JSON.
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
      pub fn close(&self, reason: &str);     // bounded: leaf lock + one push per client
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
  shape.
- **The attach reply exists once, in the app.** `fn attach_view(tab: TabFacts,
  screen: AttachSnapshot) -> Value`, `TabFacts { term_id, live, provider }`, is
  the only place that JSON is written. All three attach paths call it:
  `attach_to_tab` (app.rs:17863), `agent_attach`'s no-tab branch (17702,
  `TabFacts { term_id: agent_tab_id(agent), live: false, provider: None }`),
  and `term.attach`.
- **A session's last reading is taken off the lock.** The pump's EOF path still
  reads under the mutex: `note_session_self_report` (app.rs:18620) →
  `named_conversation` → `AgentSession::session_id`, which for the terminal
  carrier is `ClaudeSessionLocator::session_id` (harness/claude.rs:182) listing
  the transcript directory through `transcript_stems` whenever the name was
  never captured. `capture_conversation_names` (app.rs:18639) already has the
  right shape — take the session out, ask with the lock released, write back —
  and becomes the only shape:

  ```rust
  pub struct SelfReport { named: Option<String>, model: Option<String> }
  impl SelfReport {
      /// The filesystem read. Off the lock, always: the caller holds an Arc,
      /// not a registry borrow.
      pub fn read(session: &Arc<dyn AgentSession>) -> SelfReport;
  }
  impl AppState {
      /// Write down what moved. Pure bookkeeping, compared before written.
      fn note_self_report(&mut self, owner: &str, agent_id: &str, report: SelfReport);
  }
  ```

  `note_session_self_report`, `note_named_conversation`, `note_announced_model`,
  `named_conversation` and `announced_model` are deleted: the first three are
  `note_self_report`, and the last two read a session *through* `state.tabs`,
  the borrow that kept the read under the lock.
- **Replaces** `Tab.screen: Option<TermScreen>` → `Option<ScreenHandle>`;
  `agent_screens_awaiting_spawn` → `HashMap<TabKey, ScreenHandle>`;
  `Tab::require_terminal_and_screen` (a borrow of `AppState`) →
  `Tab::terminal_handle` (an owned clone); `close_a_screen_with_no_terminal` →
  `ScreenHandle::close`; the two hand-built attach payloads → `attach_view`; the
  shell EOF's `tab.session.end()` → `AppState::retire_tab`.
- **Lock discipline** app mutex → resolve `TabKey` → clone the handle →
  **release** → lock the screen. `spawn_tab_pump` takes the app mutex three
  times in a tab's life — at start to look the handle up, and twice at EOF with
  the reading between them: **take** the session `Arc` and the tab's role out
  (only if `Arc::ptr_eq` with the session it pumps); **release**, then
  `SelfReport::read` and, for a shell, `retire_tab`; **re-acquire** for
  `note_self_report` and `record_agent_session_end`. Every chunk and flush in
  between is screen-lock only. `term.input`/`term.resize` clone a
  `TerminalHandle`, release, then write, so a pty nobody drains blocks one
  worker. `SessionSender::push` is all that runs under the screen lock.
- **Tests** `a_streaming_pty_never_takes_the_app_mutex`,
  `a_board_read_answers_while_three_screens_are_flooding`,
  `term_input_to_a_pty_that_is_not_draining_leaves_the_app_mutex_free`,
  `carrying_clients_between_two_screens_holds_one_lock_at_a_time`,
  `an_agent_tabs_last_reading_leaves_the_app_mutex_free`,
  `killing_a_wedged_harness_never_holds_the_app_mutex`,
  `a_close_after_a_wedged_kill_still_reaches_its_clients`,
  `a_replaced_sessions_late_eof_leaves_the_replacement_tab_alone`.

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

  /// Find-or-spawn one agent tab. Idempotent per tab however many callers ask
  /// at once: it owns the single-flight claim, the wait, and the release.
  pub fn ensure_agent_tab(
      state: &Arc<Mutex<AppState>>, root: &Path, owner: &str,
      agent_id: &str, choice: &ModelChoice,
  ) -> Result<(String, Spawned), String>;
  ```

- **A turn's message is optional, so there is one delivery path.**
  `PendingAgentTurn.cold` / `.warm` become `say: Option<TurnText>`
  (`TurnText { cold: String, warm: String }`); `agent.start` (app.rs:17744)
  queues `None` when nothing is unread and stops branching between `deliver`
  and a bare `ensure_agent_tab`.
- **Hides** which half of a turn travels, the readiness wait, the
  `PROMPT_WRITE_EXIT_GRACE` exit race, the in-flight bookkeeping, the
  resume/transcript/locator order — no verb knows a harness exists.
- **Replaces** every site that drains the queue on the caller's thread:
  `dispatch_frame`'s inline `deliver_pending_agent_turns` (app.rs:17483) and
  the MCP done socket's two (4457, 4484) → `DeliveryRunner::spawn`.
  `ensure_agent_tab`'s `sleep(25 ms)` loop against `AGENT_SPAWN_WAIT` → the
  condvar wait below; its dead-tab `session.end()` →
  `retire_tab_keeping_screen`; its stale-owner sweep's → `retire_tab`.
  `scaffold_agent_worktree`, `resume_id_probe`, `transcript_probe`,
  `session_locator_factory` and `agent_harness_spec` leave the reservation
  block for `probe_and_scaffold`.
- **The spawn plan carries what builds a spec, not a spec**, because the spec's
  inputs (`continue_session`, `resume_session_id`) are the probes' outputs and
  the probes are disk reads:

  ```rust
  struct AgentSpawnPlan {
      project: Orchestrator,          // cloned under the lock; builds the spec off it
      root: PathBuf, owner: String, agent_id: String, model_choice: ModelChoice,
      recorded_resume_id: Option<String>,
      may_pick_up_a_conversation: bool,
      probes: SessionProbes,          // the three Arc closures, cloned
      session_token: String,
      carried: Option<ScreenHandle>,  // from retire_tab_keeping_screen
      claim: SpawnClaim,
  }
  impl AgentSpawnPlan { fn probe_and_scaffold(self) -> Result<ReadyToSpawn, String>; }

  /// The pick-up rule — resume an exact name, else `--continue` a transcript,
  /// else fresh — in one place.
  pub struct SessionProbes;           // Clone
  impl SessionProbes {
      fn pickup(&self, root: &Path, provider: Provider, recorded: Option<String>,
                may_pick_up: bool) -> SessionPickup;
      fn locator(&self, root: &Path, provider: Provider) -> SessionLocator;
  }
  pub struct SessionPickup {
      resume_session_id: Option<String>,
      continue_session: bool,
      /// The recorded name the provider no longer holds. The apply phase forgets
      /// it — `record_agent_resume_id(owner, agent_id, None)` is an AppState
      /// write and has no business in a probe.
      recorded_name_is_gone: bool,
  }
  struct ReadyToSpawn { spec: HarnessSpec, size: PtySize, locator: SessionLocator,
                        pickup: SessionPickup, carried: Option<ScreenHandle>,
                        claim: SpawnClaim }
  ```

- **The single-flight claim has no public surface.** `ensure_agent_tab` owns
  both ends. Under one acquisition it returns the live tab, or takes the claim
  (`AppState.agent_spawns_in_flight`, read in the same acquisition as the tab
  registry or two callers spawn two harnesses), or hands the guard to
  `AppState.agent_spawn_finished: Arc<Condvar>` and looks again when a spawn
  ends. `SpawnClaim` travels through the plan and is consumed by the acquisition
  that inserts the tab; its `Drop` releases the claim and notifies every waiter
  on any path that never got there, a panic included. `AGENT_SPAWN_WAIT` is the
  condvar's timeout.
- **Lock discipline** Three acquisitions. **Take**: the queue, the in-flight
  marks, the reserved tab id, `retire_tab_keeping_screen` for the dead tab this
  spawn replaces and `retire_tab` for the stale-owner sweep — one acquisition,
  as today, so the idle sweep never sees a gap; the receipts are dropped.
  **Run**: probe, scaffold, build the spec, spawn, `send_turn` — none.
  **Apply**: insert the tab, consume the claim,
  `record_agent_resume_id(.., None)` when `recorded_name_is_gone`,
  `record_agent_session_start` / `record_agent_delivery_failure`,
  `note_entity_changed`. A verb that queues a turn answers with the tab id
  reserved under the lock; the outcome reaches the browser through the
  entity's push event.
- **Tests** `a_message_is_answered_before_its_agent_has_spawned`,
  `agent_start_answers_with_the_reserved_tab_before_the_harness_is_up`,
  `a_board_read_completes_while_a_cold_spawn_waits_for_readiness`,
  `two_callers_of_one_tab_spawn_one_harness_without_spinning`,
  `a_dead_recorded_resume_name_is_forgotten_in_the_apply_phase`,
  `a_spawn_that_replaces_a_wedged_tab_answers_before_it_dies`,
  `a_restarted_agent_keeps_its_attached_terminal`.

## 4. The diff cache — one read, one owner

- **Boundary** `bridge/src/app.rs`, beside `DiffCacheKey`: between a poll surface
  and the git work its numbers come from. Three typed reads are the only way a
  verb touches a diff cache; each serves what is there, claims the refresh it
  needs, and never computes.
- **Interface**

  ```rust
  impl AppState {
      fn run_stat(&mut self, run_id: &str) -> Option<Value>;
      fn external_worktrees(&mut self, project_id: &str) -> ScanRead;
      fn primary_summary(&mut self, project_id: &str) -> Option<Value>;
      fn note_worktree_appeared(&mut self, project_id: &str, worktree: ExternalWorktree);
      fn note_worktree_gone(&mut self, project_id: &str, path: &Path);

      /// Claim and spawn `refresh` unless one is already running. Single-flight,
      /// non-blocking, no age test: the caller has decided it wants a scan.
      fn refresh_now(&mut self, refresh: DiffCacheRefresh);
      /// `refresh_now` unless what it would replace is younger than `ttl`. The
      /// caller passes the stamp it just answered from, so nothing here looks a
      /// timestamp up by kind. Private; the three reads above are its callers.
      fn refresh_if_stale(&mut self, computed_at: Option<Instant>, ttl: Duration,
                          refresh: DiffCacheRefresh);
  }
  pub struct ScanRead { worktrees: Vec<ExternalWorktree>, ever_scanned: bool }
  ```

- **The stamp and the TTL come from the caller, because the caller has them.**
  `run_stat_cache: HashMap<String, (Instant, Value)>` (app.rs:1923),
  `Project.external_scan.scanned_at` (1324), `Project.primary_summary:
  Option<(Instant, Value)>` (1011); `TASK_STAT_TTL` (1316),
  `EXTERNAL_SCAN_INTERVAL` (1017), `PRIMARY_SUMMARY_TTL` (1320), each declared
  beside the cache it governs. `refresh_if_stale` is compare, then
  `refresh_now`; `refresh_now` is claim, then spawn.
- **The one match left on `DiffCacheRefresh` is the one the rules allow.**
  `key()` and `compute()` (app.rs:1501, 1513) are the single construction point
  where a refresh names itself and picks its git work.
- **Hides** staleness, the single-flight claim, the spawn. Every miss and every
  stale entry end alike: an answer now, `spawn_diff_refresh` behind it,
  `publish_diff_refresh` + `note_board_changed` when it lands. No read returns
  a variant to be matched: a stat or `None`, the scan and whether one has ever
  landed, a summary or `None`.
  Emptiness is rendered, not stored: `ever_scanned == false` becomes
  `{"worktrees": [], "scanning": true}` in `external_worktrees_json`
  (app.rs:6909); `None` becomes `null` where `run_view` and the sidebar build
  their JSON.
- **Replaces** the whole second owner of staleness. Deleted: `warm_diff_caches`
  (app.rs:1725), `diff_caches_read_by`, `DiffCacheScope` (1441),
  `claim_stale_diff_refreshes` (3987), `stale_run_stat_work`,
  `stale_external_scan_work`, `stale_primary_summary_work`, `diff_cache_work`,
  `DiffCacheWork`, `ClaimedRefresh.blocking` and the `blocking` parameter,
  `DiffCacheWork::AwaitFirstValue`, `wait_for_first_diff_value` (1761),
  `FIRST_COMPUTE_WAIT` (1498). Also deleted: the compute-of-last-resort inside
  `run_stat` (14519-14540) and `external_worktrees` (4222-4270), and the
  `force` parameter with it. `trigger_diff_refresh` (4106) is `refresh_now`.
  `invalidate_external_scan` (4273) → `note_worktree_appeared` /
  `note_worktree_gone`, so a create no longer empties a project's whole scan.
- **`run.finish` / `branch.finish` decide from the truth, not a number.**
  `DiffCacheScope::Run` and `Branch` existed so they would not judge
  uncommitted work from a stale stat; the finish's own lock-free preflight
  already forces a rescan and rechecks eligibility inside
  `WorktreeFinishJob::run` (app.rs:16002), §5's run phase.
- **`force: true` has three callers.** `run_adopt` itself (app.rs:12551) →
  `AdoptExternalCheckout::perform` (§5); `bare_checkout_on_branch` (13663) →
  `DispatchCheckout::perform` (§5); `resolve_external_worktree`'s forced retry
  (4296) → deleted, below. `resolve_external_worktree` is not on `run.adopt`'s
  path: it serves the four non-lifecycle callers.
- **A cache miss elsewhere is an error, not a scan.** `resolve_external_worktree`
  keeps its four callers — `TermScope::ExternalWorktree` for `term.create` /
  `fs.tree` (app.rs:214), git scope resolution (7105), `resolve_branch_scope`
  (7377), `worktree.diff` (8451) — and becomes one cached read plus a `find`.
  Build's own creates and removals are in the cache the moment they land, so
  the only id that can miss is one for a worktree created outside Build since
  the last scan. A miss calls `refresh_now` unconditionally — a miss against a
  fresh cache must not wait out `EXTERNAL_SCAN_INTERVAL`, so the retry window
  is one scan's duration — and the error says so: `unknown worktree_id: <id>
  (a worktree created outside Build is resolvable once the scan now running
  lands)`.
- **Lock discipline** all three reads are pure bookkeeping: read the map, maybe
  insert a claim, hand back a value. The compute runs on `spawn_blocking`
  holding nothing; publishing takes the app mutex on its own.
- **Tests** `board_list_answers_scanning_when_nothing_has_ever_been_computed`,
  `a_first_scan_never_runs_under_the_app_mutex`,
  `a_created_worktree_joins_the_scan_cache_instead_of_clearing_it`,
  `run_finish_refuses_uncommitted_work_found_by_its_own_preflight`,
  `an_out_of_band_worktree_id_is_refused_and_claims_one_scan`.

## 5. `WorktreeLifecycleJob` — worktree work off the lock

- **Boundary** `bridge/src/lifecycle.rs` (new; `WorktreeFinishJob` moves here as
  one mutation), between a lifecycle verb's decision and the git that carries it
  out.
- **Interface**

  ```rust
  pub struct WorktreeLifecycleJob {
      reservation: Box<dyn Reservation>,      // what the decide phase reserved
      mutation: Box<dyn WorktreeMutation>,    // carries its own inputs, whole
      #[cfg(test)] gate: Option<OffLockGate>,
  }
  impl WorktreeLifecycleJob { pub fn run(self) -> LifecycleOutcome; }

  /// The run half. One impl per verb, chosen at the one construction point.
  pub trait WorktreeMutation: Send {
      /// Lock-free and self-contained: whatever this verb needs — a cloned
      /// `Orchestrator`, a base branch, a URL and a destination — is this impl's
      /// own field. Consumes itself into the apply half, typed.
      fn perform(self: Box<Self>) -> Result<Performed, String>;
  }

  /// The apply half. One impl per verb, built by that verb's `perform`.
  pub trait LifecycleEpilogue: Send {
      fn apply(self: Box<Self>, state: &mut AppState) -> Result<Value, String>;
  }

  pub struct Performed { change: WorktreeChange, epilogue: Box<dyn LifecycleEpilogue> }
  pub struct WorktreeChange { appeared: Vec<ExternalWorktree>, gone: Vec<PathBuf> }
  pub struct LifecycleOutcome { reservation: Box<dyn Reservation>,
                                result: Result<Performed, String> }

  impl AppState {
      fn defer_lifecycle(&mut self, job: WorktreeLifecycleJob) -> Value;   // placeholder
      fn apply_lifecycle(&mut self, outcome: LifecycleOutcome) -> Result<Value, String>;
  }
  ```

- **No fixed field the trait cannot fill.** The job holds no `project` and no
  `base_branch`; each mutation holds what it needs:

  ```rust
  struct CreateWorktree        { project: Orchestrator, base_branch: String, slug: String, .. }
  struct AdoptExternalCheckout { project: Orchestrator, base_branch: String, run_id: RunId,
                                 worktree_id: String, excluded: Vec<PathBuf>,
                                 model_choice: ModelChoice }
  struct AdoptPrimaryCheckout  { project: Orchestrator, base_branch: String, run_id: RunId,
                                 repo_path: PathBuf, model_choice: ModelChoice }
  struct DispatchCheckout      { project: Orchestrator, base_branch: String, run_id: RunId,
                                 branch: Option<String>, instruction: String, excluded: Vec<PathBuf>, .. }
  struct DiscardCheckout       { project: Orchestrator, worktree: Worktree,
                                 retirements: Vec<Retirement> }
  struct OpenRepo              { path: PathBuf, requested_base: Option<String>, minted: String }
  struct CloneRepo             { url: String, dest: PathBuf, requested_base: Option<String>,
                                 minted: String }
  ```

- **`project.add` opens; `project.clone` clones, or opens.** `project_add`
  (app.rs:6427) has no URL and no destination: it `git2::Repository::open`s an
  existing path, reads `git_default_branch` (15155, a `git rev-parse`
  shell-out) when no base was named, and `revparse_single`s the base — all
  under the lock today. That is `OpenRepo::perform`, which also canonicalizes
  the path (`add_project`, 3615, does it under the lock today). `project_clone`
  (6650) has two arms — `dest.exists()` → `git_remote_origin` (15128, a
  shell-out) must match the URL, then register the existing checkout
  (6666-6683); else `git clone`, then register — and `CloneRepo::perform` is
  those two arms followed by the same `open_repo(path, requested_base)`
  function `OpenRepo::perform` is, so both produce one `ProjectAdded { minted,
  path, base }`. A clone that fails after creating `dest` removes it in
  `perform`'s own error path. `add_project` splits: `mint_project_id` in the
  decide phase (`proj-<next_project>`) and `register_project(minted, path,
  base)` in the epilogue, which builds the `Orchestrator` (`Orchestrator::new`,
  orchestrator.rs:811, touches no disk), keeps today's idempotency — a
  canonical path already registered answers with its existing id and the
  minted one is dropped — and `persist`s.
- **`run.adopt` has two arms, so it has two mutations.** `run_adopt`
  (app.rs:12514) picks by `primary: true` at 12518, which is the construction
  point: `AdoptPrimaryCheckout::perform` runs `describe_primary_checkout`
  (worktree.rs:621 — `git worktree list --porcelain` plus libgit2 reads, under
  the lock today at app.rs:12534); `AdoptExternalCheckout::perform` runs
  `discover_external_worktrees` and finds `worktree_id` in it (the forced scan
  at 12551 today). Each canonicalizes `excluded` itself
  (`bound_worktree_paths`, 3651, canonicalizes under the lock today). Both hand
  the `ExternalWorktree` they produced to one free function:

  ```rust
  /// A checkout that passed all three refusals. Construction IS the validation,
  /// so nothing downstream can refuse a worktree it has already written to.
  pub struct AdoptableCheckout { path: PathBuf, name: String, branch: String,
                                 head_subject: String }
  impl AdoptableCheckout {
      /// `adopt_run`'s three refusals (orchestrator.rs:2400-2427): detached
      /// HEAD; the base branch checked out, for `ExternalWorktree` only; a
      /// `-`-prefixed branch name. Pure.
      pub fn judge(checkout: &ExternalWorktree, base_branch: &str, scope: AdoptionScope)
          -> Result<AdoptableCheckout, OrchestratorError>;
  }
  /// judge, checkpoint (`commit_all_with_message`), `scaffold_build_dir`, and
  /// the `RunAdopted` epilogue. Shared by both adopt mutations and by
  /// `DispatchCheckout`'s adopted arm.
  fn adopt(project: &Orchestrator, checkout: ExternalWorktree, base_branch: &str,
           scope: AdoptionScope, run_id: RunId, model_choice: ModelChoice)
      -> Result<Performed, String>;
  impl Orchestrator {
      pub fn adopt_run(&self, id: RunId, checkout: &AdoptableCheckout,
                       base_branch: &str, model_choice: ModelChoice) -> ActiveRun;
  }
  ```

  A refusal returns `Err` with nothing on disk touched and nothing persisted —
  today's order preserved — and `adopt_run` in the epilogue cannot fail on a
  verdict. `DispatchCheckout::perform` produces the same type from either arm
  (the bare checkout it adopted or the branch it just cut), so
  `BranchDispatched` has one shape. The decide phase keeps `run_adopt`'s two
  early returns (`primary_run_of`, `run_owning_worktree_id`: an owner exists,
  answer with its view) and adds one: a pending row already claiming this
  checkout answers with that row, so two `run.adopt primary:true` from two
  browsers converge on one run — the invariant 12508-12512 enforces today.
- **`Reservation` — what the decide phase reserved, and what undoes it.** Every
  verb's decide phase puts one placeholder on the board and, for some verbs,
  takes something out of the registry. The row is the same fact for all of
  them; the rest is the verb's own:

  ```rust
  /// The board's carrier for a verb in flight. New state: `Creating`/`Discarding`
  /// exist nowhere in `src/` today, and external worktrees are scan-discovered.
  pub struct PendingRow { entity_id: String, project_id: String, title: String,
                          state: PendingState,
                          /// The existing card this verb acts on, when there is
                          /// one (adopt, abandon, dispatch onto a branch): the
                          /// state is rendered on it, not as a second row.
                          checkout_id: Option<String>,
                          since: Instant }
  pub enum PendingState { Creating, Discarding }   // rendered, never branched on

  pub trait Reservation: Send {
      fn row(&self) -> &PendingRow;
      /// Undo the rest — the registry writes, not the row. Called only when the
      /// mutation failed; a mutation that succeeded is settled by its epilogue,
      /// which writes the real record where the placeholder stood.
      fn roll_back(self: Box<Self>, state: &mut AppState);
  }

  impl AppState {
      fn reserve_row(&mut self, row: PendingRow);            // decide
      fn release_row(&mut self, entity_id: &str);            // apply, both paths
      fn pending_rows_json(&self, project_id: &str) -> Vec<Value>;   // board.list
  }
  ```

  Four impls, one per thing a decide phase can hold: `ReservedName` (the slug
  — every create verb; a second create of the same slug in the same project is
  refused while the row stands), `ReservedCheckout` (the checkout id — adopt,
  dispatch onto a named branch), `TakenRun` (the `ActiveRun` `take_run`
  (app.rs:8493) removed, put back by `roll_back` — `run.abandon`),
  `MintedProject` (the minted id, dropped by `roll_back`; ids are not dense).
  None removes anything from disk: a partial clone or a half-cut branch is
  removed by the run phase that made it, in its own error path.
  `pending_rows_json` is read beside `external_worktrees_json` (app.rs:6909),
  so a `Creating` row is on the board from the decide phase's acquisition until
  the epilogue replaces it.
- **The pending row is not persisted. This deviates from spec step 4.** The
  spec says "persist that state"; here `PendingRow` lives only in `AppState`
  and the store is written by the epilogue — for `run.abandon` not before:
  `take_run` removes from the map and never touches the store, and
  `persist_run_record` (2839) runs inside `answer_run_mutation`, the
  epilogue's. What a crash between decide and apply leaves, per reservation:
  `ReservedName` — a bare worktree on disk, which the next scan discovers as
  an external card, exactly what a finished create produces; `ReservedCheckout`
  — a checkpoint commit on a checkout the store never bound, a bare card with
  one extra commit; `TakenRun` — the store still says active, so boot loads
  the run as it was, and `archive_runs_with_deleted_worktrees` (13948, run by
  every `board.list`) archives it if the removal had finished; `MintedProject`
  — a directory under `projects_dir` that `project_clone`'s register-existing
  arm (6666) adopts on retry, or refuses as not a git repo if the clone was
  cut short. Nothing is left that git and the store cannot re-derive;
  persisting the row would need a store table plus a boot reconciliation whose
  only job is to delete what the scan already ignores. `since` is for the
  board (a row pending longer than a scan interval reads as stuck), not for
  recovery.
- **Every verb's apply is a named type.** `apply_lifecycle` does the shared half
  (`release_row`, `note_worktree_appeared` / `note_worktree_gone`, stamp
  interaction, `note_entity_changed`), then `epilogue.apply(self)` or
  `reservation.roll_back(self)`:

  | verb | mutation | epilogue | what only it does |
  | --- | --- | --- | --- |
  | `worktree.create` | `CreateWorktree` | `WorktreeCreated` | record the row, reply with the worktree |
  | `branch.dispatch` | `DispatchCheckout` | `BranchDispatched` | `RunAdopted`'s work on the `AdoptableCheckout` it was handed, then mint the agent and queue its first turn |
  | `run.create` / `issue.implement_*` | `CreateWorktree` | `ImplementationOpened` | bind the run to its issue |
  | `plan.create` | `CreateWorktree` | `PlanWorkspaceOpened` | attach the docs dir to the plan |
  | `run.adopt` | `AdoptExternalCheckout` / `AdoptPrimaryCheckout` | `RunAdopted` | `adopt_run`'s record, `forget_row_dismissals`, `answer_run_mutation` / `run_view` |
  | `run.abandon` | `DiscardCheckout` | `RunAbandoned` | `abandon_run_keeping_checkout`, close the lineage, mirror to the issue |
  | `worktree.finish` | `FinishWorktree` | `WorktreeArchived` | the archive record |
  | `run.finish` | `FinishWorktree` | `RunFinished` | retire the run (`active: Box<ActiveRun>`) |
  | `branch.finish` | `FinishWorktree` | `BranchFinished` | retire the run and settle the issue |
  | `project.add` | `OpenRepo` | `ProjectAdded` | `register_project`, `persist` |
  | `project.clone` | `CloneRepo` | `ProjectAdded` | the same |

  A reply that needs `AppState` — `run.adopt`'s `run_view`, every
  `answer_run_mutation` — is built in the epilogue, which is why
  `WorktreeChange` carries no `reply`.
- **`run.release` builds no job.** Spec finding 2 lists it, but `run_release`
  (app.rs:12741) runs no git: a store delete, a map remove, `close_agent_tab`,
  `invalidate_external_scan`. Its one unbounded step was the kill, which
  `retire_agent_tabs` (receipts dropped) removes; `invalidate_external_scan`
  becomes `note_worktree_appeared` for the checkout it hands back.
- **`DiscardCheckout` waits for the agents before it removes the directory.
  That is a new order, argued here.** Today `run_abandon` (app.rs:12340-12351)
  removes first — `abandon_run` → `WorktreeManager::remove`, which opens with
  `remove_dir_all` (worktree.rs:377-389) — and kills after, and its own comment
  calls the removal best-effort. A child still writing into a directory
  `remove_dir_all` is walking fails the walk (a file created behind it leaves a
  non-empty directory), so the order that makes the removal reliable is kill,
  reap, remove. The decide phase issues the kill (`retire_agent_tabs`,
  receipts returned); `DiscardCheckout::perform` waits each receipt out with
  `CHECKOUT_REAP_WAIT` (5 s, declared beside `HARNESS_READY_GRACE` in
  orchestrator.rs) and then removes. On expiry it logs the tab that would not
  die and removes anyway — best-effort, as today, and the verdict stands. The
  cost is nothing on the normal path (a SIGKILLed harness reaps in
  milliseconds) and at most 5 s off the lock on a wedged one, which today is
  served under the app mutex.
- **`worktree.create`'s placeholder id.** `external_worktree_id`
  (worktree.rs:541) hashes the canonical path, and `worktree_create` (7538)
  canonicalizes after the directory exists. The decide phase has no directory,
  so it hashes `worktrees_root.join(project_id).join(slug)` with
  `worktrees_root` canonicalized once at boot (main.rs:144, where
  `cfg.worktrees` is read; a `create_dir_all` + `canonicalize` there, new).
  That is the epilogue's id unless `WorktreeManager::create` (worktree.rs:147)
  suffixes the slug because `name_taken` found a branch, a worktree or a
  directory in the way — a git and filesystem read the decide phase cannot
  make. Then `release_row` retires the placeholder by its own id and the
  entity push carries both ids, so the SPA overlay rekeys (`runOptimistic`'s
  `handle.rekey`, optimistic.js:138).
- **Hides** every shell-out and libgit2 call a lifecycle verb makes, the
  scaffold-after-create ordering, the fresh scan an adoption or a dispatch
  resolves against. `WorktreeManager` gains `#[derive(Clone)]` (two `PathBuf`s).
- **Replaces** `DeferredWork::Finish` collapses into `DeferredWork::Lifecycle`;
  `FinishEpilogue` and `FinishKind{Worktree,Run,Branch}` (app.rs:15959-15977)
  are deleted — their three arms become `WorktreeArchived`, `RunFinished`,
  `BranchFinished`. `BranchDispatchCreations` (1230) + `undo_branch_dispatch`
  (13717) split by phase: what the run phase cut, the run phase removes in its
  own error path; what the decide phase wrote, `Reservation::roll_back`
  removes. `bare_checkout_on_branch` (13663) folds into
  `DispatchCheckout::perform`. `worktree_create`, `cut_branch_for_dispatch`,
  `ensure_issue_implementation_worktree`, `open_implementation_run`,
  `plan_create`'s planning worktree, `run_adopt`, `run_abandon`, `project_add`
  and `project_clone` each stop calling git and return a job.
- **Lock discipline** decide (validate without disk, mint the id, reserve the
  row, take the run out, clone the orchestrator, retire the agent tabs, build
  the job) under the app mutex; run (the scan, the git, the checkpoint, the
  scaffold, the reap wait) holding nothing; apply (the shared bookkeeping, then
  the epilogue, or roll the reservation back) under it again. `perform` takes
  no argument, so a mutation cannot reach state it is not allowed to touch. The
  drain in `dispatch_frame` is unchanged: it already runs `DeferredWork`
  between two acquisitions, and a `BranchDispatched` epilogue's queued turn
  leaves it through `DeliveryRunner::spawn`.
- **Tests** `worktree_create_runs_git_worktree_add_with_the_state_lock_free`,
  `run_abandon_removes_its_checkout_with_the_state_lock_free`,
  `run_abandon_waits_for_its_agents_to_die_before_removing_the_checkout`,
  `run_abandon_removes_the_checkout_anyway_when_an_agent_will_not_die`,
  `a_creating_worktree_is_on_the_board_before_its_git_returns`,
  `a_create_that_fails_rolls_its_reservation_back_and_leaves_no_row`,
  `a_suffixed_slug_settles_the_placeholder_under_its_real_id`,
  `run_adopt_refuses_a_detached_head_before_it_writes_a_checkpoint`,
  `run_adopt_of_the_primary_checkout_reads_git_with_the_state_lock_free`,
  `two_adopts_of_one_checkout_converge_on_one_run`,
  `project_add_reads_the_default_branch_with_the_state_lock_free`,
  `project_clone_registers_its_project_from_the_landed_path`,
  `run_adopt_answers_from_its_epilogue_with_the_runs_own_view`.

## What builds no primitive

`agent.choose` (app.rs:8200) opens no tab and spawns nothing: it validates a
model choice against the agent's locked harness, writes it, and answers.

Step 5 (SPA): the `agent_starting` state goes on the overlay `core/optimistic.js`
already owns, and the four views that read a tab id out of a reply
(`thread.post`, `agent.start`, `branch.dispatch`, `worktree.create`) read the
entity's fields from the next push instead — including the `live` and `spawned`
that `agent.start` used to answer with. `createBranch`
(spa/src/core/createWork.js:87) today awaits the reply and inserts no
provisional row; step 5 gives it one, keyed by the placeholder id the reply
carries and rekeyed by the settling push, so the board's `Creating` row and
the overlay's are one row. The spec's load test,
`bridge/tests/concurrency_load.rs`, is the only one that measures a number.

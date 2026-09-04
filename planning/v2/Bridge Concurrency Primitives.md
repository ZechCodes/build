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

The rule has no exceptions, which costs one change to the code it constrains:
`Orchestrator` gains `#[derive(Clone)]` (two `PathBuf`s, a `WorktreeManager` of
two more, `Templates`, the `Agent` closure, `PtySize`). A verb clones its
project's orchestrator under the lock and calls it with the lock released. What
stays under the lock is the part of `Orchestrator` that touches no disk: the run
and plan state machines (`RunEvent::Abandon` and its siblings), never the git
around them. Two methods that today straddle both halves are split so the line
can hold:

- `Orchestrator::abandon_run` (orchestrator.rs:2526) is deleted. It is
  `abandon_run_keeping_checkout` — the lifecycle verdict, pure bookkeeping —
  plus `self.worktrees.remove(..)`, which is `git worktree remove` and
  `remove_dir_all`. The verdict stays under the lock, the removal becomes
  §5's `DiscardCheckout` mutation, and no caller can take both at once again.
- `Orchestrator::adopt_run` (orchestrator.rs:2392) keeps its validation and its
  `ActiveRun` construction and loses its two disk steps —
  `commit_all_with_message` (the adoption checkpoint) and `scaffold_build_dir`
  — to §5's `AdoptCheckout` mutation. The run id is minted in the decide phase
  and carried into the run phase, because the scaffold is written per owner.

## Lock order, daemon-wide

The app mutex is above every other lock. `FrameClock` counters (§1) are taken
under it only to stamp the current holder; `ChangeBus` is a leaf, taken with it
released. `ScreenHandle` (§2) is absent from the order because it is never
nested: the app mutex resolves the handle, releases, and only then is the screen
locked. Two screens are never locked at once either — the one carry that reads
two of them (`carry_clients_from`) drains the waiting screen under its own lock,
releases it, and only then locks its own. The one lock taken *with* the app
mutex in hand is §3's spawn condvar, which is what a condvar is: it hands the
guard back while it waits.

## 1. `FrameClock` — the timing guard

- **Boundary** `bridge/src/timing.rs` (new), between the relay's worker pool and
  the app. The only thing that knows how long a frame took, and nothing about
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
  queue depth.
- **Replaces** every bare `state.lock().unwrap()` on the dispatch path
  (`dispatch_frame`, `term_*`, `agent_attach`) with `timer.lock(state)`;
  `LockedFor` derefs to `AppState`, so nothing else moves.
- **Owns frame latency, not every clock.** The durable timestamps that decide
  staleness and rate limits keep their own `Instant`s and are none of
  `FrameClock`'s business: `TermScreen.last_flood_snapshot_at` (app.rs:242),
  `Tab.last_delivered_at` (app.rs:700), `ExternalScanCache.scanned_at`
  (app.rs:1324), and the `(Instant, Value)` stamps on `run_stat_cache` and
  `primary_summary` (app.rs:1011). Those answer "is this entry old"; `FrameClock`
  answers "how long did this frame take", and only frames flow through it.
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
  takes the app mutex" structural rather than a habit. It emits no reply JSON
  either — the wire shape of an attach is the app's, not the screen's.
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
  shape.
- **The attach reply exists once, in the app.** An attach answers with two
  things: what the screen knows (`AttachSnapshot`) and what the tab knows
  (`term_id`, `live`, `provider`). One app-side function joins them —
  `fn attach_view(tab: TabFacts, screen: AttachSnapshot) -> Value`, where
  `TabFacts` is `{ term_id, live, provider }` — and it is the only place that
  JSON is written. All three attach paths call it: `attach_to_tab`
  (app.rs:17863), `agent_attach`'s no-tab branch (app.rs:17702, which passes
  `TabFacts { term_id: agent_tab_id(agent), live: false, provider: None }`), and
  `term.attach`. The screen module never sees a `Value`.
- **Replaces** `Tab.screen: Option<TermScreen>` → `Option<ScreenHandle>`;
  `agent_screens_awaiting_spawn` → `HashMap<TabKey, ScreenHandle>`;
  `Tab::require_terminal_and_screen` (a borrow of `AppState`) →
  `Tab::terminal_handle` (an owned clone); `close_a_screen_with_no_terminal` →
  `ScreenHandle::close`; the two hand-built attach payloads → `attach_view`.
- **Lock discipline** app mutex → resolve `TabKey` → clone the handle →
  **release** → lock the screen; never held together. `carry_clients_from` locks
  two screens in sequence and never at once: it takes the waiting screen's lock,
  drains its clients and viewport out (leaving it empty), releases, then locks
  its own and registers them — so the two orders a deadlock needs cannot both
  exist. `spawn_tab_pump` takes the app mutex twice in a tab's life — at start to
  look the handle up, at EOF for the death rites (`record_agent_session_end`, tab
  removal); every chunk and flush between is screen-lock only.
  `term.input`/`term.resize` clone a `TerminalHandle`, release, then write, so a
  pty nobody drains blocks one worker. The screen lock is a leaf:
  `SessionSender::push` is all that runs there.
- **Tests** `a_streaming_pty_never_takes_the_app_mutex`,
  `a_board_read_answers_while_three_screens_are_flooding`,
  `term_input_to_a_pty_that_is_not_draining_leaves_the_app_mutex_free`,
  `carrying_clients_between_two_screens_holds_one_lock_at_a_time`.

## 3. `DeliveryRunner` — the background delivery runner

- **Boundary** `bridge/src/delivery.rs` (new; `deliver`,
  `deliver_pending_agent_turns` and `ensure_agent_tab` move here), between a
  verb's durable state change and the agent process that hears about it. Two
  public calls, and every path to a harness goes through one of them.
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

- **A turn's message is optional, so there is one delivery path, not two.**
  `PendingAgentTurn.cold` / `.warm` become
  `say: Option<TurnText>` (`TurnText { cold: String, warm: String }`). A turn
  with something to say is what `thread.post` and `branch.dispatch` queue; a
  turn with `None` is what `agent.start` queues when nothing is unread — the
  tab is opened and nothing is written to it. `agent_start` (app.rs:17744) stops
  branching between `deliver` and a bare `ensure_agent_tab` and queues one turn
  either way, so the turnless spawn has the same owner as every other.
- **Hides** which half of a turn travels (cold/warm), the readiness wait, the
  `PROMPT_WRITE_EXIT_GRACE` exit race, the in-flight bookkeeping, the
  resume/transcript/locator order — no verb knows a harness exists.
- **Replaces** every site that drains the queue on the caller's thread:
  `dispatch_frame`'s inline `deliver_pending_agent_turns` (app.rs:17483), and
  the MCP done socket's two (app.rs:4457, the router action that dispatches a
  branch, and app.rs:4484, the report that starts the next phase) →
  `DeliveryRunner::spawn`, so the reply and the socket's ack go out as soon as
  the store write lands. `ensure_agent_tab`'s `sleep(25 ms)` loop against
  `AGENT_SPAWN_WAIT` → the condvar wait below. `scaffold_agent_worktree`,
  `resume_id_probe`, `transcript_probe`, `session_locator_factory` and
  `agent_harness_spec` leave the reservation block for `probe_and_scaffold`.
- **The spawn plan carries what builds a spec, not a spec.** The spec's inputs
  (`continue_session`, `resume_session_id`) are the probes' outputs, and the
  probes are disk reads that must not run under the lock — so the plan cannot
  hold a `HarnessSpec`, and holds the means to build one instead:

  ```rust
  struct AgentSpawnPlan {
      project: Orchestrator,          // cloned under the lock; builds the spec off it
      root: PathBuf, owner: String, agent_id: String, model_choice: ModelChoice,
      recorded_resume_id: Option<String>,
      may_pick_up_a_conversation: bool,
      probes: SessionProbes,          // the three Arc closures, cloned
      session_token: String,
      carried: Option<ScreenHandle>,
      claim: SpawnClaim,
  }
  impl AgentSpawnPlan { fn probe_and_scaffold(self) -> Result<ReadyToSpawn, String>; }

  /// The pick-up rule — resume an exact name, else `--continue` a transcript,
  /// else fresh — in one place instead of inline in the reservation block.
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

- **The single-flight claim has no public surface.** There is no
  `reserve`/`release` for a caller to pair and no bare `Condvar` for a caller to
  wait on: `ensure_agent_tab` owns both ends. Under one acquisition it either
  returns the live tab, or takes the claim (`AppState.agent_spawns_in_flight`,
  which must be read in the same acquisition as the tab registry or two callers
  spawn two harnesses), or hands the guard to `AppState.agent_spawn_finished:
  Arc<Condvar>` and looks again when a spawn ends. `SpawnClaim` is the claim
  itself: it travels through the plan and is consumed by the acquisition that
  inserts the tab, and its `Drop` releases the claim and notifies every waiter
  on any path that never got there — a panic included. The waiters wake on a
  notify, not on a 25 ms timer, and `AGENT_SPAWN_WAIT` becomes the condvar's
  timeout rather than a deadline a sleep loop counts toward.
- **Lock discipline** Three acquisitions, no more. **Take**: the queue, the
  in-flight marks and the reserved tab id in one acquisition, exactly as today,
  so the idle sweep never sees a gap. **Run**: probe, scaffold, build the spec,
  spawn, `send_turn` — none. **Apply**: insert the tab, consume the claim,
  `record_agent_resume_id(.., None)` when `recorded_name_is_gone`,
  `record_agent_session_start` / `record_agent_delivery_failure`,
  `note_entity_changed`. The condvar wait is the one taken with a guard in hand.
  A verb that queues a turn answers with the tab id reserved under the lock; the
  outcome reaches the browser through the entity's push event, not the reply.
- **Tests** `a_message_is_answered_before_its_agent_has_spawned`,
  `agent_start_answers_with_the_reserved_tab_before_the_harness_is_up`,
  `a_board_read_completes_while_a_cold_spawn_waits_for_readiness`,
  `two_callers_of_one_tab_spawn_one_harness_without_spinning`,
  `a_dead_recorded_resume_name_is_forgotten_in_the_apply_phase`.

## 4. The diff cache — one read, one owner

- **Boundary** `bridge/src/app.rs`, beside `DiffCacheKey`: between a poll surface
  and the git work its numbers come from. Three typed reads are the only way a
  verb touches a diff cache, and each does the whole job — serve what is there,
  judge it, claim and start the refresh it needs, and never compute.
- **Interface**

  ```rust
  impl AppState {
      fn run_stat(&mut self, run_id: &str) -> Option<Value>;
      fn external_worktrees(&mut self, project_id: &str) -> ScanRead;
      fn primary_summary(&mut self, project_id: &str) -> Option<Value>;
      fn note_worktree_appeared(&mut self, project_id: &str, worktree: ExternalWorktree);
      fn note_worktree_gone(&mut self, project_id: &str, path: &Path);

      /// The one place staleness is judged and a refresh is claimed and spawned.
      /// Private; the three reads above are its only callers.
      fn refresh_if_stale(&mut self, refresh: DiffCacheRefresh);
  }
  pub struct ScanRead { worktrees: Vec<ExternalWorktree>, ever_scanned: bool }
  impl DiffCacheRefresh { fn ttl(&self) -> Duration; }   // the TTL per key, in one place
  ```

- **Hides** staleness, the single-flight claim, the spawn. `refresh_if_stale`
  looks the entry's timestamp up by `refresh.key()` and compares it against
  `refresh.ttl()` (`TASK_STAT_TTL`, `EXTERNAL_SCAN_INTERVAL`,
  `PRIMARY_SUMMARY_TTL`), so a caller passes one value and knows none of that.
  Every miss and every stale entry end alike: an answer now, a
  `spawn_diff_refresh` behind it, `publish_diff_refresh` + `note_board_changed`
  when it lands.
- **No cache read returns a variant to be matched.** Each read is typed to its
  own key and hands back the domain answer: a stat or `None`, the scan and
  whether one has ever landed, a summary or `None`. Nothing carries two
  impossible arms, and nothing sequences a build-then-read-then-placeholder.
- **Emptiness is rendered, not stored.** `scanning_placeholder` does not exist:
  `ever_scanned == false` becomes `{"worktrees": [], "scanning": true}` in
  `external_worktrees_json` (app.rs:6909), and `None` becomes `null` where
  `run_view` and the sidebar build their JSON. The cache key stays a domain type
  and no reply shape is written on it.
- **Replaces** the whole second owner of staleness. Deleted: `warm_diff_caches`
  (app.rs:1725), `diff_caches_read_by`, `DiffCacheScope`,
  `claim_stale_diff_refreshes`, `stale_run_stat_work`, `stale_external_scan_work`,
  `stale_primary_summary_work`, `diff_cache_work`, `DiffCacheWork`,
  `ClaimedRefresh.blocking` and the `blocking` parameter,
  `DiffCacheWork::AwaitFirstValue`, `wait_for_first_diff_value`,
  `FIRST_COMPUTE_WAIT`. Also deleted: the compute-of-last-resort inside
  `run_stat` and `external_worktrees` and the primary-summary read, and
  `external_worktrees(.., force: true)` with it. `trigger_diff_refresh` folds
  into `refresh_if_stale`. `invalidate_external_scan` →
  `note_worktree_appeared` / `note_worktree_gone`, so a create no longer empties
  a project's whole scan. With no pre-warm and no blocking read there is nothing
  left that decides an entry is stale except the read of that entry.
- **The two verbs that decided from a number keep deciding from the truth.**
  `DiffCacheScope::Run` and `Branch` existed so `run.finish` / `branch.finish`
  would not judge uncommitted work from a stale stat. They do not need a
  blocking cache read: the finish's own lock-free preflight already forces a
  rescan and rechecks eligibility inside `WorktreeFinishJob::run` (app.rs:16012),
  which is §5's run phase. The decision moves there entirely, where the numbers
  are fresh by construction.
- **Resolving a worktree id gets its fresh scan from §5.** `run.adopt` was
  `force: true`'s only caller. Its decide phase reserves against the id it was
  given; `AdoptCheckout::perform` runs `discover_external_worktrees` off the
  lock, resolves the id against that scan, and fails there if it names nothing —
  so adoption still never acts on a stale card, and `resolve_external_worktree`'s
  forced second scan goes the same way.
- **Lock discipline** all three reads are pure bookkeeping: read the map, maybe
  insert a claim, hand back a value. The compute runs on `spawn_blocking` holding
  nothing; publishing takes the app mutex on its own.
- **Tests** `board_list_answers_scanning_when_nothing_has_ever_been_computed`,
  `a_first_scan_never_runs_under_the_app_mutex`,
  `a_created_worktree_joins_the_scan_cache_instead_of_clearing_it`,
  `run_finish_refuses_uncommitted_work_found_by_its_own_preflight`.

## 5. `WorktreeLifecycleJob` — worktree work off the lock

- **Boundary** `bridge/src/lifecycle.rs` (new; `WorktreeFinishJob` moves here as
  one mutation), between a lifecycle verb's decision and the git that carries it
  out — for create, dispatch, implement, adopt, release, abandon, clone, finish
  and rollback alike.
- **Interface**

  ```rust
  pub struct WorktreeLifecycleJob {
      reservation: Reservation,               // the persisted Creating/Discarding record
      project: Orchestrator,                  // cloned under the lock, used off it
      base_branch: String,
      mutation: Box<dyn WorktreeMutation>,
      #[cfg(test)] gate: Option<OffLockGate>,
  }
  impl WorktreeLifecycleJob { pub fn run(self) -> LifecycleOutcome; }

  /// The run half. One impl per verb, chosen at the one construction point.
  pub trait WorktreeMutation: Send {
      /// Lock-free. Consumes itself into the apply half, which carries what the
      /// git made — typed, so nothing downstream matches on a kind.
      fn perform(self: Box<Self>, project: &Orchestrator, base: &str)
          -> Result<Performed, String>;
  }

  /// The apply half. One impl per verb, built by that verb's `perform`.
  pub trait LifecycleEpilogue: Send {
      fn apply(self: Box<Self>, state: &mut AppState) -> Result<Value, String>;
  }

  pub struct Performed { change: WorktreeChange, epilogue: Box<dyn LifecycleEpilogue> }
  pub struct WorktreeChange { appeared: Vec<ExternalWorktree>, gone: Vec<PathBuf> }
  pub struct LifecycleOutcome { reservation: Reservation, result: Result<Performed, String> }

  impl AppState {
      fn defer_lifecycle(&mut self, job: WorktreeLifecycleJob) -> Value;   // placeholder
      fn apply_lifecycle(&mut self, outcome: LifecycleOutcome) -> Result<Value, String>;
  }
  ```

- **Every verb's apply is a named type.** `apply_lifecycle` does the half that is
  the same for all of them — `note_worktree_appeared` / `note_worktree_gone` for
  the change, settle or roll back the reservation, stamp interaction,
  `note_entity_changed` — and then calls `epilogue.apply(self)` for the half that
  is not. The epilogues, one per verb, each carrying its own typed payload and
  building its own reply off `AppState`:

  | verb | mutation | epilogue | what only it does |
  | --- | --- | --- | --- |
  | `worktree.create` | `CreateWorktree` | `WorktreeCreated` | record the row, reply with the worktree |
  | `branch.dispatch` | `CutBranch` | `BranchDispatched` | `RunAdopted`'s work, then mint the agent and queue its first turn |
  | `run.create` / `issue.implement_*` | `CreateWorktree` | `ImplementationOpened` | bind the run to its issue |
  | `plan.create` | `CreateWorktree` | `PlanWorkspaceOpened` | attach the docs dir to the plan |
  | `run.adopt` | `AdoptCheckout` | `RunAdopted` | `Orchestrator::adopt_run`'s record, `forget_row_dismissals`, `answer_run_mutation` / `run_view` |
  | `run.release` | `ReleaseCheckout` | `RunReleased` | give the checkout back as a bare row |
  | `run.abandon` | `DiscardCheckout` | `RunAbandoned` | `abandon_run_keeping_checkout`, close the agent tab, close the lineage, mirror to the issue |
  | `worktree.finish` | `FinishWorktree` | `WorktreeArchived` | the archive record |
  | `run.finish` | `FinishWorktree` | `RunFinished` | retire the run (`active: Box<ActiveRun>`) |
  | `branch.finish` | `FinishWorktree` | `BranchFinished` | retire the run and settle the issue |
  | `project.add` / `project.clone` | `CloneRepo` | `ProjectAdded` | register the project |

  A reply that needs `AppState` — `run.adopt`'s `run_view`, every
  `answer_run_mutation` — is built in the epilogue, which has it. That is why
  `WorktreeChange` carries no `reply`: nothing off the lock can write one.
- **Hides** every shell-out and libgit2 call a lifecycle verb makes, the
  scaffold-after-create ordering, the fresh scan an adoption resolves against.
  `WorktreeManager` gains `#[derive(Clone)]` (two `PathBuf`s) so `Orchestrator`
  can derive it too, and the run phase carries its own copy of both.
- **Replaces** `DeferredWork::Finish` collapses into `DeferredWork::Lifecycle`;
  `FinishEpilogue` and `FinishKind{Worktree,Run,Branch}` (app.rs:15959-15977) are
  deleted outright — they were the kind match this trait replaces, and their
  three arms become `WorktreeArchived`, `RunFinished`, `BranchFinished`.
  `BranchDispatchCreations` + `undo_branch_dispatch` split by phase: what the run
  phase cut, the run phase removes in its own error path (the git is there, and a
  failure fails fast where it happened); what the decide phase wrote,
  `Reservation::roll_back` removes, one rollback for every verb.
  `worktree_create`, `cut_branch_for_dispatch`,
  `ensure_issue_implementation_worktree`, `open_implementation_run`,
  `plan_create`'s planning worktree, `run_adopt`, `run_release`, `run_abandon`,
  `project_add` and `project_clone` each stop calling git and return a job.
- **Lock discipline** decide (validate, mint the id, reserve the name, persist
  the `Creating`/`Discarding` entity, clone the orchestrator, build the job)
  under the app mutex; run (the git, the checkpoint, the scan an adoption
  resolves against, the scaffold) holding nothing; apply (the shared bookkeeping,
  then the epilogue, or roll the reservation back) under it again. `perform`
  takes `&Orchestrator` and no `&AppState`, so a mutation cannot reach state it
  is not allowed to touch. The drain in `dispatch_frame` is unchanged: it already
  runs `DeferredWork` between two acquisitions, and a `BranchDispatched`
  epilogue's queued turn leaves it through `DeliveryRunner::spawn` like any
  other.
- **Tests** `worktree_create_runs_git_worktree_add_with_the_state_lock_free`,
  `run_abandon_removes_its_checkout_with_the_state_lock_free`,
  `a_creating_worktree_is_on_the_board_before_its_git_returns`,
  `a_create_that_fails_rolls_its_reservation_back_and_leaves_no_row`,
  `run_adopt_answers_from_its_epilogue_with_the_runs_own_view`.

## What builds no primitive

`agent.choose` (app.rs:8200) opens no tab and spawns nothing: it validates a
model choice against the agent's locked harness, writes it, and answers with the
choice. Spec step 2 lists it beside `agent.start` as a verb whose reply embeds a
spawned tab; it has none, so there is nothing there to change. Recorded so the
absence reads as an answer rather than a gap.

Step 5 (SPA): the `agent_starting` state goes on the overlay `core/optimistic.js`
already owns, and the four views that read a tab id out of a reply
(`thread.post`, `agent.start`, `branch.dispatch`, `worktree.create`) read the
entity's fields from the next push instead — including the `live` and `spawned`
that `agent.start` used to answer with, which nothing can know at reply time once
the spawn is behind the reply. The spec's load test,
`bridge/tests/concurrency_load.rs`, is the only one that measures a number.

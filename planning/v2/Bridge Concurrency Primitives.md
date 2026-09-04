# Bridge Concurrency Primitives

Status: design, 2026-09-04. The component list for `Bridge Concurrency Spec.md`,
branch `fix/bridge-concurrent-requests`. Read with the spec open; where they
disagree the spec wins. It says what must stop happening — this names the five
components built so it stops, one per step, plus the one primitive the rule
itself costs.

## The one rule every component obeys

**The `AppState` mutex is held only for bounded in-memory bookkeeping.** Nothing
whose duration another process, another machine, or a disk decides runs while it
is held: no process wait, no pipe or socket read, no filesystem walk, no git
shell-out, no sleep. Every component below is one job split the same way —
decide under the lock, run with it released, apply under it again.

The rule is stated in terms of unbounded work because two writes stay under the
lock deliberately and the spec already cleared them: the SQLite store (WAL,
`synchronous = NORMAL`, sub-millisecond) and `persist`'s single config write.
Both are bounded by this machine's own disk with nothing to wait behind. A
filesystem *walk* is not — `transcript_stems` lists a directory that grows with
every conversation — and a process wait is the worst of all, which is why the
kill has a carrier of its own below.

So stated, the rule has no exceptions, and it costs three changes to the code it
constrains.

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
- `Orchestrator::adopt_run` (orchestrator.rs:2392) keeps its `ActiveRun`
  construction and loses both its disk steps — `commit_all_with_message` (the
  adoption checkpoint) and `scaffold_build_dir` — to §5's `AdoptCheckout`
  mutation. Its three refusals leave too, into `AdoptableCheckout` (§5), so the
  verdict still precedes the checkpoint. The run id is minted in the decide
  phase and carried into the run phase, because the scaffold is written per
  owner.

### `SessionReaper` — ending a tab, off the lock

`AgentSession::end` is `child.kill()` then `child.wait()` for both carriers
(`PtySession::kill_and_reap`, pty.rs:683; `AdkSession::end`, harness/adk.rs:723).
SIGKILL does not land on a child wedged in uninterruptible I/O until that I/O
returns, so `wait()` is an unbounded process wait — the plainest violation of
the rule there is, and today every one of its eight callers holds the app mutex:
`close_agent_tab` (app.rs:3900), `term_close` (6013), the vanished-worktree
reaper (6130), `retire_agent` (8328), `abandon_router_session` (9567),
`ensure_agent_tab`'s dead-tab replacement (17950) and stale-owner sweep (17973),
and `spawn_tab_pump`'s shell EOF (18436).

All eight do the same three things in the same order — take the tab out of the
registry, end its session, tell its clients — so they become one call, and the
two halves that are not bookkeeping move to a thread that holds nothing:

```rust
// bridge/src/reaper.rs (new)
pub struct SessionReaper;                 // one std::thread, no runtime needed
impl SessionReaper {
    /// Push the close to the tab's clients and kill and reap its process, both
    /// on the reaper's thread. Returns before either has happened.
    pub fn retire(&self, tab: RetiredTab) -> Retirement;
}
pub struct RetiredTab {
    session: Arc<dyn AgentSession>, screen: Option<ScreenHandle>,
    wire_id: String, reason: &'static str,
}
pub struct Retirement;                    // Clone
impl Retirement {
    /// True once the process is reaped. Callable only with the app mutex
    /// released — it is the wait the rule forbids, made explicit.
    pub fn wait(&self, timeout: Duration) -> bool;
}

impl AppState {
    /// Remove one tab and retire it. The only way a tab stops existing.
    fn retire_tab(&mut self, key: &TabKey, reason: &'static str) -> Option<Retirement>;
    fn retire_agent_tabs(&mut self, root: &Path) -> Vec<Retirement>;   // was close_agent_tab
}
```

An `std::thread` and not a spawned task, because the synchronous unit tests run
with no runtime under them and a tab still has to end there.

Almost every caller drops the receipt: the tab is out of the registry, which is
what stops the agent being addressable, and the process dying a moment later
changes nothing. **One caller waits**, and it is why `Retirement` exists:
`DiscardCheckout::perform` (§5) waits out the agents `run.abandon`'s decide
phase retired before it runs `git worktree remove` on the directory they had as
their cwd. That wait happens in the run phase, holding nothing — which is the
whole point, since today the same ordering is bought by killing under the lock.

## Lock order, daemon-wide

The app mutex is above every other lock. Under it, two leaves may be taken:
`FrameClock`'s counters (§1), to stamp the current holder, and `ChangeBus`'s
`pending` set — `note_board_changed` / `note_entity_changed` (app.rs:4337-4345)
are `AppState` methods called with the app mutex held throughout, by
`add_project` (3644), `store_diff_entry` (4163), `apply_deferred` (5806-5808)
and §5's `apply_lifecycle`. That is safe and deliberate, and `ChangeBus::note`
(changes.rs:157) says so in its own words: one leaf mutex, an insert into a set,
a `notify_one`, return. The flusher that sends never takes the app mutex, so the
order is app mutex → `pending`, never the reverse.

`ScreenHandle` (§2) is absent from the order because it is never nested: the app
mutex resolves the handle, releases, and only then is the screen locked. Two
screens are never locked at once either — the one carry that reads two of them
(`carry_clients_from`) drains the waiting screen under its own lock, releases
it, and only then locks its own. The reaper's thread locks a screen and a child,
never `AppState`. The one lock taken *with* the app mutex in hand is §3's spawn
condvar, which is what a condvar is: it hands the guard back while it waits.

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
  (app.rs:1324), and the `(Instant, Value)` stamps on `run_stat_cache`
  (app.rs:1923) and `primary_summary` (app.rs:1011). Those answer "is this entry
  old"; `FrameClock` answers "how long did this frame take", and only frames flow
  through it.
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
- **A session's last reading is taken off the lock, like every other.** The
  pump's EOF path is the one place a *reading* still happens under the mutex:
  `note_session_self_report` (app.rs:18620) → `named_conversation` →
  `AgentSession::session_id`, which for the terminal carrier is
  `ClaudeSessionLocator::session_id` (harness/claude.rs:182) listing the
  transcript directory through `transcript_stems` whenever the name was never
  captured. `capture_conversation_names` (app.rs:18639) already has the right
  shape — take the session out, ask with the lock released, write back — and it
  becomes the only shape, named once:

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
  which is exactly the borrow that kept the read under the lock. The sweep and
  the pump's EOF are then the same two lines in a different order, and neither
  can drift from the other.
- **Replaces** `Tab.screen: Option<TermScreen>` → `Option<ScreenHandle>`;
  `agent_screens_awaiting_spawn` → `HashMap<TabKey, ScreenHandle>`;
  `Tab::require_terminal_and_screen` (a borrow of `AppState`) →
  `Tab::terminal_handle` (an owned clone); `close_a_screen_with_no_terminal` →
  `ScreenHandle::close`; the two hand-built attach payloads → `attach_view`; the
  shell EOF's `tab.session.end()` → `AppState::retire_tab`.
- **Lock discipline** app mutex → resolve `TabKey` → clone the handle →
  **release** → lock the screen; never held together. `carry_clients_from` locks
  two screens in sequence and never at once: it takes the waiting screen's lock,
  drains its clients and viewport out (leaving it empty), releases, then locks
  its own and registers them — so the two orders a deadlock needs cannot both
  exist. `spawn_tab_pump` takes the app mutex three times in a tab's life — at
  start to look the handle up, and twice at EOF with the reading between them:
  **take** the session `Arc` and the tab's role out; **release**, then
  `SelfReport::read` (the transcript listing) and, for a shell, `retire_tab`'s
  kill on the reaper's thread; **re-acquire** for `note_self_report` and
  `record_agent_session_end`. Every chunk and flush in between is screen-lock
  only, and the screen's `flush` / `push_closed` need no app mutex at all.
  `term.input`/`term.resize` clone a `TerminalHandle`, release, then write, so a
  pty nobody drains blocks one worker. The screen lock is a leaf:
  `SessionSender::push` is all that runs there.
- **Tests** `a_streaming_pty_never_takes_the_app_mutex`,
  `a_board_read_answers_while_three_screens_are_flooding`,
  `term_input_to_a_pty_that_is_not_draining_leaves_the_app_mutex_free`,
  `carrying_clients_between_two_screens_holds_one_lock_at_a_time`,
  `an_agent_tabs_last_reading_leaves_the_app_mutex_free`,
  `killing_a_wedged_harness_never_holds_the_app_mutex`.

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
  `AGENT_SPAWN_WAIT` → the condvar wait below. Its two `session.end()` calls →
  `AppState::retire_tab`. `scaffold_agent_worktree`, `resume_id_probe`,
  `transcript_probe`, `session_locator_factory` and `agent_harness_spec` leave
  the reservation block for `probe_and_scaffold`.
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
  in-flight marks, the reserved tab id, and `retire_tab` for the dead tab this
  spawn replaces and for the stale-owner sweep — one acquisition, exactly as
  today, so the idle sweep never sees a gap. The retirements' receipts are
  dropped here: the tabs are out of the registry, and their processes die on the
  reaper's thread. **Run**: probe, scaffold, build the spec, spawn, `send_turn`
  — none. **Apply**: insert the tab, consume the claim,
  `record_agent_resume_id(.., None)` when `recorded_name_is_gone`,
  `record_agent_session_start` / `record_agent_delivery_failure`,
  `note_entity_changed`. The condvar wait is the one taken with a guard in hand.
  A verb that queues a turn answers with the tab id reserved under the lock; the
  outcome reaches the browser through the entity's push event, not the reply.
- **Tests** `a_message_is_answered_before_its_agent_has_spawned`,
  `agent_start_answers_with_the_reserved_tab_before_the_harness_is_up`,
  `a_board_read_completes_while_a_cold_spawn_waits_for_readiness`,
  `two_callers_of_one_tab_spawn_one_harness_without_spinning`,
  `a_dead_recorded_resume_name_is_forgotten_in_the_apply_phase`,
  `a_spawn_that_replaces_a_wedged_tab_answers_before_it_dies`.

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

      /// Claim and spawn `refresh` unless what it would replace is younger than
      /// `ttl`. The caller passes the stamp it just answered from, so nothing
      /// here looks a timestamp up by kind. Private; the three reads above are
      /// its only callers.
      fn refresh_if_stale(&mut self, computed_at: Option<Instant>, ttl: Duration,
                          refresh: DiffCacheRefresh);
  }
  pub struct ScanRead { worktrees: Vec<ExternalWorktree>, ever_scanned: bool }
  ```

- **The stamp comes from the caller, because the caller has it.** The three
  timestamps live in three places and stay there — `run_stat_cache:
  HashMap<String, (Instant, Value)>` (app.rs:1923),
  `Project.external_scan.scanned_at` (1324), `Project.primary_summary:
  Option<(Instant, Value)>` (1011) — and each typed read has already touched its
  own entry to answer from it. Handing that `Instant` on is one move; looking it
  up again by key would be a match on `DiffCacheKey`, which is what this design
  refuses. The TTL travels the same way: `TASK_STAT_TTL`, `EXTERNAL_SCAN_INTERVAL`
  and `PRIMARY_SUMMARY_TTL` are each declared beside the cache they govern and
  passed by the read that owns it — one fact, one place, no `ttl()` match.
  `refresh_if_stale` is then three lines with no knowledge of kinds: compare,
  claim, spawn.
- **The one match left on `DiffCacheRefresh` is the one the rules allow.**
  `key()` and `compute()` (app.rs:1513) are the single construction point where
  a refresh names itself and picks its git work. Nothing else in the path
  branches on a variant.
- **Hides** staleness, the single-flight claim, the spawn. Every miss and every
  stale entry end alike: an answer now, a `spawn_diff_refresh` behind it,
  `publish_diff_refresh` + `note_board_changed` when it lands.
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
  `run_stat` and `external_worktrees` and the primary-summary read, and the
  `force` parameter with it. `trigger_diff_refresh` folds into
  `refresh_if_stale`. `invalidate_external_scan` → `note_worktree_appeared` /
  `note_worktree_gone`, so a create no longer empties a project's whole scan.
  With no pre-warm and no blocking read there is nothing left that decides an
  entry is stale except the read of that entry.
- **The two verbs that decided from a number keep deciding from the truth.**
  `DiffCacheScope::Run` and `Branch` existed so `run.finish` / `branch.finish`
  would not judge uncommitted work from a stale stat. They do not need a
  blocking cache read: the finish's own lock-free preflight already forces a
  rescan and rechecks eligibility inside `WorktreeFinishJob::run` (app.rs:16012),
  which is §5's run phase. The decision moves there entirely, where the numbers
  are fresh by construction.
- **`force: true` has two callers, and both become run phases.**
  `resolve_external_worktree` (app.rs:4296) scans forced when its first read
  misses; `bare_checkout_on_branch` (app.rs:13663) scans forced so a dispatch
  decides against the checkouts that exist now. Both scans move off the lock into
  §5 mutations: `AdoptCheckout::perform` runs `discover_external_worktrees` and
  resolves `run.adopt`'s id against that scan, and `DispatchCheckout::perform`
  runs one scan and decides from it whether to adopt the bare checkout on the
  named branch or cut a new one. Neither verb can act on a stale card, and
  neither scans under the mutex.
- **A cache miss elsewhere is an error, not a scan.** `resolve_external_worktree`
  keeps its four non-lifecycle callers — `TermScope::ExternalWorktree` for
  `term.create` / `fs.tree` (app.rs:214), git scope resolution (7105),
  `resolve_branch_scope` (7377), `worktree.diff` (8451) — and becomes one cached
  read plus a `find`: no second scan, no compute. Build's own creates and removals
  are in the cache the moment they land (`note_worktree_appeared` /
  `note_worktree_gone`), so the only id that can miss is one for a worktree
  created outside Build since the last scan. That id is unresolvable for at most
  `EXTERNAL_SCAN_INTERVAL`: the read that missed has already claimed the refresh
  on its way past, and the error says so —
  `unknown worktree_id: <id> (a worktree created outside Build is resolvable
  after the next scan)` — so the client's retry succeeds rather than the daemon
  blocking every caller for a scan one of them asked for.
- **Lock discipline** all three reads are pure bookkeeping: read the map, maybe
  insert a claim, hand back a value. The compute runs on `spawn_blocking` holding
  nothing; publishing takes the app mutex on its own.
- **Tests** `board_list_answers_scanning_when_nothing_has_ever_been_computed`,
  `a_first_scan_never_runs_under_the_app_mutex`,
  `a_created_worktree_joins_the_scan_cache_instead_of_clearing_it`,
  `run_finish_refuses_uncommitted_work_found_by_its_own_preflight`,
  `an_out_of_band_worktree_id_is_refused_until_the_next_scan_lands`.

## 5. `WorktreeLifecycleJob` — worktree work off the lock

- **Boundary** `bridge/src/lifecycle.rs` (new; `WorktreeFinishJob` moves here as
  one mutation), between a lifecycle verb's decision and the git that carries it
  out — for create, dispatch, implement, adopt, release, abandon, clone, finish
  and rollback alike.
- **Interface**

  ```rust
  pub struct WorktreeLifecycleJob {
      reservation: Box<dyn Reservation>,      // what the decide phase persisted
      mutation: Box<dyn WorktreeMutation>,    // carries its own inputs, whole
      #[cfg(test)] gate: Option<OffLockGate>,
  }
  impl WorktreeLifecycleJob { pub fn run(self) -> LifecycleOutcome; }

  /// The run half. One impl per verb, chosen at the one construction point.
  pub trait WorktreeMutation: Send {
      /// Lock-free, and self-contained: whatever this verb needs — a cloned
      /// `Orchestrator`, a base branch, a URL and a destination — is this impl's
      /// own field, because not every verb has the same ones. Consumes itself
      /// into the apply half, typed, so nothing downstream matches on a kind.
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

- **No fixed field the trait cannot fill.** `project.add` / `project.clone` is
  why: there is no `Orchestrator` before the repo exists (`add_project`,
  app.rs:3614, constructs one from the landed path) and no base branch to pass
  (`register_clone` reads it off the clone with `git_default_branch(&dest)`,
  app.rs:6700). So the job holds no `project` and no `base_branch` and each
  mutation holds what it needs:

  ```rust
  struct CreateWorktree  { project: Orchestrator, base_branch: String, name: String, .. }
  struct AdoptCheckout   { project: Orchestrator, base_branch: String, run_id: RunId,
                           worktree_id: String, excluded: HashSet<PathBuf>,
                           scope: AdoptionScope, model_choice: ModelChoice }
  struct DispatchCheckout{ project: Orchestrator, base_branch: String, run_id: RunId,
                           branch: Option<String>, instruction: String, .. }
  struct DiscardCheckout { project: Orchestrator, worktree: Worktree,
                           retirements: Vec<Retirement> }
  struct CloneRepo       { url: String, dest: PathBuf, project_id: String,
                           requested_base: Option<String> }
  ```

  `CloneRepo::perform` clones, then reads the landed clone's default branch and
  hands it to `ProjectAdded`, which builds the `Orchestrator` under the lock —
  `Orchestrator::new` (orchestrator.rs:811) touches no disk, so that is
  bookkeeping. `add_project` splits to match: `mint_project_id` in the decide
  phase (the id the placeholder row is keyed by) and
  `register_project(id, path, base)` in the epilogue.
- **`AdoptableCheckout` — the verdict before the checkpoint.** `adopt_run`'s
  three refusals (detached HEAD, the base branch checked out, a `-`-prefixed
  branch name; orchestrator.rs:2400-2427) all read `checkout.branch`, which
  exists only once `perform` has resolved the id against its fresh scan. So they
  move out of `adopt_run` and become the thing that scan produces:

  ```rust
  /// A checkout that passed all three refusals. Construction IS the validation,
  /// so nothing downstream can refuse a worktree it has already written to.
  pub struct AdoptableCheckout { path: PathBuf, name: String, branch: String,
                                 head_subject: String }
  impl AdoptableCheckout {
      pub fn judge(checkout: &ExternalWorktree, base_branch: &str, scope: AdoptionScope)
          -> Result<AdoptableCheckout, OrchestratorError>;   // pure
  }
  impl Orchestrator {
      pub fn adopt_run(&self, id: RunId, checkout: &AdoptableCheckout,
                       base_branch: &str, model_choice: ModelChoice) -> ActiveRun;
  }
  ```

  `AdoptCheckout::perform` scans, finds the id, calls `judge` — all before it
  writes anything — and only then runs `commit_all_with_message` and
  `scaffold_build_dir`. A refusal returns `Err` with nothing on disk touched and
  nothing persisted, which is today's order preserved. `adopt_run` in the
  epilogue takes an `AdoptableCheckout` and therefore cannot fail on a verdict at
  all. `DispatchCheckout::perform` produces the same type from either arm — the
  bare checkout it adopted or the branch it just cut — so `BranchDispatched`
  has one shape to handle.
- **`Reservation` — what the decide phase persisted, and what undoes it.** Every
  verb's decide phase puts one placeholder row on the board and, for some verbs,
  takes something out of the registry. The row is the same fact for all of them,
  so it lives in one place; the rest is the verb's own, so it lives behind the
  trait:

  ```rust
  /// The board's carrier for a verb in flight. There is no such record today —
  /// `Creating`/`Discarding` exist nowhere in `src/`, and external worktrees are
  /// scan-discovered — so this is new state, and it is the whole of what makes
  /// the spec's `Creating` row visible during the run phase.
  pub struct PendingRow { entity_id: String, project_id: String, title: String,
                          state: PendingState, since: Instant }
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

  Three impls today, one per thing a decide phase can hold: `ReservedName` (the
  row plus the worktree name it reserved — every create verb), `TakenRun` (the
  row plus the `ActiveRun` `take_run` removed, whose `roll_back` puts it back —
  `run.abandon`, `run.release`), `MintedProject` (the row plus the project id
  minted for a clone, whose `roll_back` drops it; ids are not dense and nothing
  reads the gap). None of them removes anything from disk: a partial clone or a
  half-cut branch is removed by the run phase that made it, in its own error
  path.

  `pending_rows_json` is read where the board builds a project's rows, beside
  `external_worktrees_json` (app.rs:6909), so a `Creating` row is on the board
  from the decide phase's acquisition until the epilogue replaces it. Its
  `entity_id` is the id the epilogue will use, which is the id the SPA's
  optimistic overlay already keys on — the placeholder and the optimistic row are
  one row, not two.
- **Every verb's apply is a named type.** `apply_lifecycle` does the half that is
  the same for all of them — `release_row`, `note_worktree_appeared` /
  `note_worktree_gone` for the change, stamp interaction, `note_entity_changed` —
  and then either `epilogue.apply(self)` or `reservation.roll_back(self)`. The
  epilogues, one per verb, each carrying its own typed payload and building its
  own reply off `AppState`:

  | verb | mutation | epilogue | what only it does |
  | --- | --- | --- | --- |
  | `worktree.create` | `CreateWorktree` | `WorktreeCreated` | record the row, reply with the worktree |
  | `branch.dispatch` | `DispatchCheckout` | `BranchDispatched` | `RunAdopted`'s work on the `AdoptableCheckout` it was handed, then mint the agent and queue its first turn |
  | `run.create` / `issue.implement_*` | `CreateWorktree` | `ImplementationOpened` | bind the run to its issue |
  | `plan.create` | `CreateWorktree` | `PlanWorkspaceOpened` | attach the docs dir to the plan |
  | `run.adopt` | `AdoptCheckout` | `RunAdopted` | `adopt_run`'s record, `forget_row_dismissals`, `answer_run_mutation` / `run_view` |
  | `run.release` | `ReleaseCheckout` | `RunReleased` | give the checkout back as a bare row |
  | `run.abandon` | `DiscardCheckout` | `RunAbandoned` | `abandon_run_keeping_checkout`, close the lineage, mirror to the issue |
  | `worktree.finish` | `FinishWorktree` | `WorktreeArchived` | the archive record |
  | `run.finish` | `FinishWorktree` | `RunFinished` | retire the run (`active: Box<ActiveRun>`) |
  | `branch.finish` | `FinishWorktree` | `BranchFinished` | retire the run and settle the issue |
  | `project.add` / `project.clone` | `CloneRepo` | `ProjectAdded` | build the `Orchestrator` from the landed path, `register_project`, `persist` |

  A reply that needs `AppState` — `run.adopt`'s `run_view`, every
  `answer_run_mutation` — is built in the epilogue, which has it. That is why
  `WorktreeChange` carries no `reply`: nothing off the lock can write one.
- **The agents go before the directory does.** `run.abandon` and `run.release`
  retire their worktree's agent tabs in the decide phase —
  `retire_agent_tabs(root)`, pure bookkeeping, returning the receipts — and
  `run.abandon` carries them into `DiscardCheckout`, which waits each one out
  (bounded by a timeout, holding no lock) before `git worktree remove`. That is
  today's kill-then-remove ordering with the wait moved to where an unbounded
  wait is allowed to be. `run.release` drops its receipts: it hands the checkout
  back rather than deleting it, so nothing is waiting on the processes to go.
- **Hides** every shell-out and libgit2 call a lifecycle verb makes, the
  scaffold-after-create ordering, the fresh scan an adoption or a dispatch
  resolves against. `WorktreeManager` gains `#[derive(Clone)]` (two `PathBuf`s)
  so `Orchestrator` can derive it too, and each mutation carries its own copy.
- **Replaces** `DeferredWork::Finish` collapses into `DeferredWork::Lifecycle`;
  `FinishEpilogue` and `FinishKind{Worktree,Run,Branch}` (app.rs:15959-15977) are
  deleted outright — they were the kind match this trait replaces, and their
  three arms become `WorktreeArchived`, `RunFinished`, `BranchFinished`.
  `BranchDispatchCreations` + `undo_branch_dispatch` split by phase: what the run
  phase cut, the run phase removes in its own error path (the git is there, and a
  failure fails fast where it happened); what the decide phase wrote,
  `Reservation::roll_back` removes. `bare_checkout_on_branch` (app.rs:13663)
  folds into `DispatchCheckout::perform`. `worktree_create`,
  `cut_branch_for_dispatch`, `ensure_issue_implementation_worktree`,
  `open_implementation_run`, `plan_create`'s planning worktree, `run_adopt`,
  `run_release`, `run_abandon`, `project_add` and `project_clone` each stop
  calling git and return a job.
- **Lock discipline** decide (validate what can be validated without disk, mint
  the id, reserve the row, take the run out, clone the orchestrator, retire the
  agent tabs, build the job) under the app mutex; run (the scan, the git, the
  checkpoint, the scaffold, the reap wait) holding nothing; apply (the shared
  bookkeeping, then the epilogue, or roll the reservation back) under it again.
  `perform` takes no `&AppState` and no argument at all, so a mutation cannot
  reach state it is not allowed to touch. The drain in `dispatch_frame` is
  unchanged: it already runs `DeferredWork` between two acquisitions, and a
  `BranchDispatched` epilogue's queued turn leaves it through
  `DeliveryRunner::spawn` like any other.
- **Tests** `worktree_create_runs_git_worktree_add_with_the_state_lock_free`,
  `run_abandon_removes_its_checkout_with_the_state_lock_free`,
  `run_abandon_waits_for_its_agents_to_die_before_removing_the_checkout`,
  `a_creating_worktree_is_on_the_board_before_its_git_returns`,
  `a_create_that_fails_rolls_its_reservation_back_and_leaves_no_row`,
  `run_adopt_refuses_a_detached_head_before_it_writes_a_checkpoint`,
  `project_clone_registers_its_project_from_the_landed_path`,
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

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

**Shipped** with step 1 (`bridge/src/reaper.rs`), as declared. The three
`retire_*` calls, the `Arc::ptr_eq` pump check, and `Retirement::wait` — which
only §5's `DiscardCheckout` will call — are all in place. One deviation from
the sketch below: `retire_tab` takes `reason: &str`, not `&'static str`, so it
serves the reasons that are already spelled where they are pushed.

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
  one and `AppState::handler` clones it off the state (app.rs:2090, 4386), so
  every path that can take the app mutex can reach it: the relay's handler,
  cloned per reconnect (main.rs:301 builds it once, 310 clones it into every
  connection), and the MCP control socket, which has no handler at all. A clock
  built per handler would leave the socket's frames counted nowhere.

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
  `agent_start`, `warm_diff_caches` (its `wait_for_first_diff_value` included:
  the wait for another frame's first-ever compute looks at the cache under the
  mutex every 20 ms, and those acquisitions are the waiting frame's), and the
  delivery path
  (`deliver_pending_agent_turns`, `deliver`, `ensure_agent_tab`, which take the
  frame's `&FrameTimer`) — with `timer.lock(state)`. The delivery path is where
  the longest holds are, so leaving it bare would have made this step's
  before/after number for §3 read as no hold to remove, and left `lock_holder`
  saying nobody held the mutex while a spawn's probe and scaffold held it for
  seconds. The one acquisition on the frame's thread that is not timed is
  deleted instead: `spawn_activity_pump`, which `term_create` and
  `ensure_agent_tab` reach through `spawn_tab_pumps` one statement after their
  own lock block releases, took the mutex bare to read the new tab's
  `surfaces_changed()` back off the registry. `SessionOutput` now carries that
  receiver (`surfaces`, subscribed in `open_session` beside the bytes and the
  activity stream, harness/session.rs:329), so the pumps start touching no
  lock and a `term.create` or a fresh `agent.start` makes no acquisition its
  timer cannot see.
- **The MCP control socket is a frame too.** It reaches the same delivery path
  with no relay frame behind it, so it mints its own timer per socket line off
  the state's clock and records under `mcp.control` (app.rs:4462) — a name of
  its own, since it is not a wire method.
- **What is deliberately not timed.** The pump, the diff-refresh publish and the
  idle sweep are background threads: their holds belong to no frame, and a
  timer minted per chunk would count a pump as a frame served. A pump's
  *start* is on the frame's thread and is not on this list: it takes no lock.
- **Owns frame latency, not every clock.** The timestamps that decide
  staleness keep their own `Instant`s: `TermScreen.last_flood_snapshot_at`
  (app.rs:243), `Tab.last_delivered_at` (701), `ExternalScanCache.scanned_at`
  (1325), the `(Instant, Value)` stamps on `run_stat_cache` (1937) and
  `primary_summary` (1012).
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
  `a_frames_delivery_path_reports_its_hold_and_names_its_method`,
  `a_frame_waiting_for_a_first_compute_charges_its_polls_to_the_lock`,
  `a_tabs_pumps_start_while_another_frame_holds_the_app_mutex`,
  `a_done_over_the_socket_is_timed_under_its_own_method`.

## 2. `ScreenHandle` — the per-tab screen

**Shipped** — this is step 1 (`bridge/src/screen.rs`). What landed differs from
the sketch below in eight places, each because the code said so:

- `attach` takes `viewport: Option<(u16, u16)>` rather than `cols, rows`. `None`
  is a dead tab, whose retained screen is never reflowed to a browser window
  that arrived after its agent died — the condition `attach_to_tab` spelled
  inline.
- `TerminalHandle` owns the whole attach (`attach`, which sizes the child and
  then registers the client), which was the caller sequencing a pty write
  against a screen write. The carry is the one place those two halves are NOT
  owned together, because they are not on the same side of the lock:
  `ensure_agent_tab` publishes the tab under the app mutex, and the clients
  must move in that same acquisition, while the child's window-change ioctl
  goes to a process that may not answer. `inherit_waiting_clients(&mut
  AppState, key, root, tab)` is the under-lock half — it finds the waiting
  screen (including the worktree-addressed one the first agent born here
  inherits) and calls `ScreenHandle::carry_clients_from` — and it hands back an
  `InheritedViewport`, whose `tell_child` is the ioctl, made once the guard is
  down. `TerminalHandle`'s writes all keep their "with the app mutex released"
  promise.
- `close` and `session_ended` are different verbs. `close` is a screen whose
  TAB is gone: it pushes `term.closed`, and every client that arrives
  afterwards is pushed the same words instead of being registered — a client's
  attach registers with the app mutex released while a close runs under it, so
  the app mutex cannot order the two and the screen's own lock decides.
  `session_ended` is the byte pump's EOF on an agent: the clients are told and
  STAY, because the retained grid is the last thing that agent painted and the
  session that replaces it paints onto the same screen.
- `carry_clients_from` leaves the drained screen pointing at the one its
  clients went to (`superseded_by`), and `attach` follows that link before it
  registers, under the same acquisition that asks. `agent_attach` clones a
  waiting screen's handle under the lock and registers on it released, so a
  spawn can carry those clients away in between; without the link the late
  client would sit on a screen nothing feeds and nothing closes. One lock at a
  time, all the way down.
- `close_a_screen_with_no_terminal` became the reason constant
  `NO_TERMINAL_LEFT`, since with `ScreenHandle::close` the function was one call
  and a name.
- A second reason constant, `SPAWN_NEVER_OPENED`, closes the retained grid when
  `Tab::spawn` fails. `retire_tab_keeping_screen` takes the dead tab out of the
  registry and tells its clients nothing, because they are about to be handed
  over; a spawn that never opens has nobody to hand them to and leaves that
  screen in no registry, so a reaper cannot reach it either. The failure path
  says the words itself.
- `screen_epitaph` moved onto the handle as `ScreenHandle::epitaph`; `drop_session`
  keeps its acquisition (a `retain` per screen, bounded, app mutex → screen).
- `spawn_tab_pumps` takes a `TabPumps` — the session, the screen handle and the
  streams, taken off the tab before it is handed to the registry — so the byte
  pump takes the app mutex **twice in a tab's life** rather than three times:
  both at EOF, with `SelfReport::read` between them. Nothing is looked up at
  start.

`a_board_read_answers_while_three_screens_are_flooding` is not among the tests.
Every bound it could assert passes on the old code too — chunk parses are
milliseconds each, so a read never blocked measurably on one — and the property
it names is covered by two tests that do discriminate:
`a_streaming_pty_never_takes_the_app_mutex` (three flooding screens are three of
one) and `a_frame_answers_while_a_screen_lock_is_held`. The number belongs to
the spec's load test.

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
      pub fn feed(&self, chunk: &[u8]) -> bool;   // the pump: parse + coalesce; false once closed
      pub fn flush(&self) -> bool;                // the pump's tick; false once closed
      pub fn restart(&self);                 // new session: fresh parser, same cursor
      pub fn resize(&self, cols: u16, rows: u16);
      pub fn carry_clients_from(&self, waiting: &ScreenHandle) -> bool;  // false: nobody waited, no viewport adopted
      pub fn close(&self, reason: &str);     // bounded: leaf lock + one push per client; lets the clients go
      pub fn session_ended(&self, reason: &str);   // told, and the clients stay
  }
  pub struct AttachSnapshot { snapshot: String, cursor: u64, cols: u16, rows: u16 }
  pub struct TerminalHandle { session: Arc<dyn AgentSession>, screen: ScreenHandle }
  impl TerminalHandle {      // all write with the app mutex released
      pub fn write_input(&self, bytes: &[u8]) -> Result<(), String>;
      pub fn resize(&self, cols: u16, rows: u16) -> Result<(), String>;
      /// The child takes the grid the screen already stands at — a carried
      /// retained screen, or the viewport inherited clients render at. The one
      /// place `PtySize` is built and the one place a refused ioctl is judged
      /// a dying child's business.
      pub fn fit_child_to_screen(&self);
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
  **release** → lock the screen. `spawn_tab_pump` takes the app mutex twice in
  a tab's life, both at EOF with the reading between them: **take** the tab's
  role out and, in that same acquisition, tell the clients — `flush` then
  `session_ended` for an agent, `retire_tab` for a shell, each the bounded
  close every `retire_tab` already makes under the app mutex; **release**, then
  `SelfReport::read`; **re-acquire** for `note_self_report` and
  `record_agent_session_end`. Marking the tab and telling its clients cannot be
  two acquisitions: `live = false` is what makes a tab replaceable, and the
  spawn that replaces it carries this screen — clients and all — onto its own
  session without a word, so a close pushed after the release would reach
  browsers already watching the replacement, in among its opening reset. Every
  chunk and flush in between is screen-lock only. `term.input`/`term.resize` clone a `TerminalHandle`, release, then
  write, so a pty nobody drains blocks one worker. `SessionSender::push` is all
  that runs under the screen lock.
- **Every acquisition a pump makes asks whose session it is.** `still_pumping`
  is one named rule — the tab at this key still holds the `Arc` this pump was
  started for — and both of the byte pump's acquisitions and all three of the
  activity pump's ask it. The guard on the first alone is not enough: the gap
  between them is `SelfReport::read`, a transcript-tree walk, and a post
  arriving in that window replaces the dead tab and leaves the replacement
  working. What the dead session then reports would close the live turn
  (`record_agent_session_end` → `close_turn_of_dead_agent`) and overwrite the
  conversation to resume. The activity pump carries its session for the same
  reason and takes it from `TabPumps` at the one place both pumps start.
- **A closed screen ends its pump; an emptied waiting screen stays.** The
  pump never consults the registry, so the registry cannot stop it: `close`
  lets every client go and `feed`/`flush` answer `false` from then on, which
  returns the pump the moment `retire_tab` runs — not when the wedged child
  it was painting finally dies. `drop_session` detaches from a screen in
  `agent_screens_awaiting_spawn` without removing it: an attach clones that
  handle under the app mutex and registers with it released, and the client
  arriving as the last one leaves must still be where the spawn looks. Empty,
  the screen is bounded at one per agent key, carries no viewport
  (`carry_clients_from` answers `false` and the spawn keeps Build's size),
  and leaves with the spawn that inherits it, `retire_agent`, or the reaper.
- **Tests** `a_streaming_pty_never_takes_the_app_mutex`,
  `a_frame_answers_while_a_screen_lock_is_held`,
  `a_closed_tab_stops_painting_before_its_harness_dies`,
  `closing_a_screen_lets_its_clients_go_and_stops_its_pump`,
  `a_client_attaching_as_the_last_waiting_client_leaves_is_carried_onto_the_agent`,
  `carrying_from_a_screen_everyone_left_adopts_no_viewport`,
  `term_input_to_a_pty_that_is_not_draining_leaves_the_app_mutex_free`,
  `carrying_clients_between_two_screens_holds_one_lock_at_a_time`,
  `an_agent_tabs_last_reading_leaves_the_app_mutex_free`,
  `killing_a_wedged_harness_never_holds_the_app_mutex`,
  `a_close_after_a_wedged_kill_still_reaches_its_clients`,
  `a_replaced_sessions_late_eof_leaves_the_replacement_tab_alone`,
  `a_late_self_report_never_lands_on_the_session_that_replaced_it`,
  `a_replaced_sessions_late_activity_close_leaves_the_replacement_alone`,
  `a_client_attaching_to_a_tab_that_just_closed_is_told_so`,
  `a_late_attach_lands_on_the_screen_its_clients_were_carried_to`,
  `a_screen_whose_session_ended_takes_the_session_that_replaces_it`,
  `a_client_attaching_to_a_closed_screen_is_told_it_closed`,
  `telling_an_inherited_child_its_size_never_holds_the_app_mutex`,
  `a_replacement_cannot_slip_between_a_session_ending_and_its_close`,
  `a_spawn_that_fails_closes_the_grid_it_took_from_the_dead_session`. All
  sixteen were watched to fail first; two test-side waits followed
  (`process_reaped` and `SessionLog::ended` poll the retirement thread out
  rather than asking once, which is when the fact can first be observed, not a
  weaker assertion).

## 3. `DeliveryRunner` — the background delivery runner

Shipped. What it does differs from what was declared here in four places, each
noted below.

- **Boundary** `bridge/src/delivery.rs` (new) holds the lock-free half of a
  spawn — `SessionProbes`, `SessionPickup`, `AgentSpawnPlan`, `ReadyToSpawn` —
  and cannot name `AppState`. `deliver`, `ensure_agent_tab` and
  `DeliveryRunner` stay in `app.rs`. **Deviation from the declared boundary**,
  argued: those three are `AppState`'s own bookkeeping (the tab registry, the
  in-flight claim, the resume record, the roster, the project map, the session
  tokens), and moving them across a module line would have made twenty private
  fields and ten private methods `pub(crate)` — an interface as large as the
  implementation behind it, which is the shallow module the rules forbid. What
  moved is what genuinely holds nothing.
- **Interface**

  ```rust
  /// What one lock acquisition took, each turn paired with the mark it OWES
  /// back — so the turn that landed is the only one whose mark can be settled.
  struct PendingTurns { turns: VecDeque<(PendingAgentTurn, TurnMark)>,
                        state: SettlingHandle,
                        clock: Arc<FrameClock> }   // the runner's timer, so it never locks to find one
  impl PendingTurns {
      fn next_turn(&mut self) -> Option<(PendingAgentTurn, TurnMark)>;
  }
  impl Drop for PendingTurns { .. }                          // whatever is left

  /// The turns that have left the queue and not yet reached an agent, counted
  /// under an OWNER (the idle sweep's question) and under an AGENT TAB (the
  /// question every verb that would queue a second turn asks). Every turn
  /// counts under its owner; only a turn that says something counts under its
  /// agent, because only that turn tells the agent to read its thread.
  struct TurnsInFlight { owners: HashMap<String, usize>, agents: HashMap<TabKey, usize> }
  struct TurnMark { owner: String, told_agent: Option<TabKey>,
                    state: SettlingHandle, settled: bool }
  impl TurnMark { fn settle(self, s: &mut AppState); }       // one delivered turn
  impl Drop for TurnMark { .. }                              // unsettled: gives itself back
  impl TurnsInFlight {
      fn take(&mut self, turn: &PendingAgentTurn) -> TurnMark;
      fn give_back(&mut self, mark: &TurnMark);
      fn holds_owner(&self, owner: &str) -> bool;
      fn holds_agent(&self, key: &TabKey) -> bool;
  }
  impl PendingAgentTurn { fn says_something(&self) -> bool; }
  impl AppState {
      fn take_pending_turns(&mut self) -> PendingTurns;
      /// A turn WITH WORDS for this agent is queued or mid-delivery.
      fn agent_is_on_its_way(&self, root: &Path, agent_id: &str) -> bool;
      /// The whole reading a verb needs of the agent its parameters address.
      fn addressed_agent(&mut self, params: &Value) -> Result<AddressedAgent, String>;
  }
  struct AddressedAgent { entity_id: String, agent_id: String, root: PathBuf,
                          model_choice: ModelChoice, has_unread: bool }
  struct DeliveryRunner;
  impl DeliveryRunner {
      /// `take_pending_turns` under one acquisition charged to `timer`, then
      /// `spawn`. The one call every path that queued a turn makes.
      fn drain(state: &Arc<Mutex<AppState>>, timer: &FrameTimer);
      fn spawn(state: &Arc<Mutex<AppState>>, turns: PendingTurns);  // returns at once
      fn run(state: &Arc<Mutex<AppState>>, turns: PendingTurns);    // no runtime: sync tests
  }

  /// Find-or-spawn one agent tab, in three phases and one call each. `Ok(None)`
  /// — the entity lost its session; nothing was published.
  fn ensure_agent_tab(
      state: &Arc<Mutex<AppState>>, root: &Path, owner: &str,
      agent_id: &str, choice: &ModelChoice, timer: &FrameTimer,
  ) -> Result<Option<(String, Spawned)>, String>;

  /// Decide under the lock: a live tab, a wait, or the reservation.
  enum SpawnDecision { Live(String), NoSession, Reserved(Box<ReservedSpawn>) }
  fn claim_agent_spawn(..) -> Result<SpawnDecision, String>;
  /// Probe, scaffold and open the child, with the mutex RELEASED.
  fn open_agent_session(state, reserved: ReservedSpawn, key: &TabKey, timer)
      -> Result<OpenedSession, String>;
  /// Publish it. `None` — the entity's session closed under the spawn.
  fn publish_agent_tab(state, key: &TabKey, opened: OpenedSession,
                       choice: &ModelChoice, timer) -> Option<String>;
  ```

- **A turn's message is optional, so there is one delivery path.**
  `PendingAgentTurn.cold` / `.warm` became `say: Option<TurnText>`
  (`TurnText { cold: String, warm: String }`); `agent.start` queues `None` when
  nothing is unread and no longer branches between `deliver` and a bare
  `ensure_agent_tab`. Its reply drops `live` and `spawned` — neither is knowable
  before the harness exists — and keeps `term_id`, `agent_id` and `notified`.
- **Hides** which half of a turn travels, the readiness wait, the
  `PROMPT_WRITE_EXIT_GRACE` exit race, the in-flight bookkeeping, the
  resume/transcript/locator order — no verb knows a harness exists.
- **Replaces** every site that drained the queue on the caller's thread:
  `dispatch_frame`'s inline `deliver_pending_agent_turns`, the MCP done
  socket's two, and `agent.start`'s inline `deliver` → `DeliveryRunner::drain`,
  one call at all four sites. `ensure_agent_tab`'s `sleep(25 ms)` loop against
  `AGENT_SPAWN_WAIT` → the condvar wait below. `scaffold_agent_worktree`,
  `resume_id_probe`, `transcript_probe`, `session_locator_factory` and
  `agent_harness_spec` left the reservation block for `probe_and_scaffold`.
  `Orchestrator` and `WorktreeManager` gained `#[derive(Clone)]` so the
  reservation can hand the project's orchestrator over.
- **The spawn plan carries what builds a spec, not a spec**, because the spec's
  inputs (`continue_session`, `resume_session_id`) are the probes' outputs and
  the probes are disk reads:

  ```rust
  pub struct AgentSpawnPlan {
      project: Orchestrator,          // cloned under the lock; builds the spec off it
      root: PathBuf, agent_id: String, model_choice: ModelChoice,
      recorded_resume_id: Option<String>,
      may_pick_up_a_conversation: bool,
      probes: SessionProbes,          // the three Arc closures, cloned
      session_token: String,
  }
  impl AgentSpawnPlan { pub fn probe_and_scaffold(self) -> Result<ReadyToSpawn, String>; }

  /// The pick-up rule — resume an exact name, else `--continue` a transcript,
  /// else fresh — in one place.
  pub struct SessionProbes;           // Clone
  pub struct SessionPickup {
      resume_session_id: Option<String>,
      continue_session: bool,
      /// The recorded name the provider no longer holds. The apply phase forgets
      /// it — `record_agent_resume_id(owner, agent_id, None)` is an AppState
      /// write and has no business in a probe.
      recorded_name_is_gone: bool,
  }
  pub struct ReadyToSpawn { spec: HarnessSpec, size: PtySize,
                            locator: Option<Box<dyn SessionLocator>>,
                            recorded_name_is_gone: bool }
  ```

  **Deviation:** the plan carries neither `carried: Option<ScreenHandle>` nor
  `claim: SpawnClaim` and no `owner`. The screen and the claim are `AppState`'s
  and travel beside the plan in `ReservedSpawn`; the harness spec is built from
  the cwd and the agent id alone, so `owner` was never one of its inputs.

  Every field is `pub` and every one of these is built as a struct literal.
  `AgentSpawnPlan` and `SessionProbes` have no constructor: three same-shaped
  `Arc` closures and three adjacent `String`s in a positional argument list are
  ways to hand a harness the wrong probe, or its own agent id as its MCP token,
  and still compile. There is no invariant a constructor could enforce here —
  the module cannot name `AppState`, which is where every one of these values
  comes from.
- **What the reservation is holding travels whole.**

  ```rust
  struct ReservedSpawn { plan: AgentSpawnPlan, role: TabRole, holding: SpawnHolding }
  /// The three things the registry gave up, which go back together if the
  /// spawn never opens: the claim, the dead session's retained grid, and the
  /// MCP token registered before the child existed.
  struct SpawnHolding { claim: SpawnClaim, carried: Option<ScreenHandle>,
                        agent_id: String, session_token: String }
  impl SpawnHolding {
      /// Give it all back; answer with the reason the spawn never opened.
      fn abandon(self, state, error: String, timer: &FrameTimer) -> String;
  }
  struct OpenedSession { tab: Tab, output: SessionOutput,
                         recorded_name_is_gone: bool, claim: SpawnClaim }
  ```

  `open_agent_session` chains the two fallible steps —
  `probe_and_scaffold().and_then(Tab::spawn)` — so there is ONE failure arm and
  one `abandon`. The carried grid is adopted by `Tab::adopt_screen`, which owns
  the whole rule: take the grid, fit the child to it, and close it with
  `NO_TERMINAL_LEFT` if this session paints nothing.
- **The single-flight claim has no public surface.** `ensure_agent_tab` owns
  both ends. Under one acquisition it returns the live tab, or takes the claim
  (`AppState.agent_spawns_in_flight`, read in the same acquisition as the tab
  registry or two callers spawn two harnesses), or hands the guard to
  `AppState.agent_spawn_finished: Arc<Condvar>` and looks again when a spawn
  ends. `SpawnClaim` travels beside the plan and is consumed by the acquisition
  that inserts the tab; its `Drop` releases the claim and notifies every waiter
  on any path that never got there, a panic included. `AGENT_SPAWN_WAIT` is the
  condvar's deadline.
- **What a background job took, it gives back while unwinding.** `SpawnClaim`,
  `PendingTurns` and `TurnMark` are the three, and they hold the daemon the
  same way:

  ```rust
  /// Weakly — a job outliving the daemon has nothing to give back to — and
  /// reacquired through a poisoned mutex deliberately, because a destructor
  /// that panics during an unwind aborts the process.
  struct SettlingHandle(Option<Weak<Mutex<AppState>>>);
  impl SettlingHandle { fn settle(&self, settle: impl FnOnce(&mut AppState)); }
  impl AppState { fn settling_handle(&self) -> SettlingHandle; }
  ```

  `take_pending_turns` marks every turn in flight, and a marked turn is spared
  by the idle sweep for as long as the mark stands — so a batch that unwound
  without giving its marks back left its runs Working with no agent and nothing
  in the daemon able to demote them. `TurnMark::settle` returns one turn's
  marks under the lock the runner already holds; `PendingTurns`'s `Drop`
  settles whatever is still in the batch under one acquisition; and a mark
  that has LEFT the batch — handed out by `next_turn`, then dropped by a
  delivery that panicked before settling it — gives itself back through its
  own `Drop`. The runner's first acquisition used to be a bare one for the
  clock, so a poisoned mutex unwound it before any turn left the batch and
  the batch's `Drop` covered everything; with the clock travelling in
  `PendingTurns` the first acquisition is the delivery's own, one turn out.
  `SpawnClaim`'s `Drop` is the same guard one phase later, and
  a leak there is worse: `agent_spawns_in_flight` is removed from in exactly two
  places, so the claim would be held for the life of the daemon — every later
  delivery to that tab waiting out `AGENT_SPAWN_WAIT` and then failing, the
  entity reading as forever starting, `agent.remove` refusing the agent. `DeliveryRunner::spawn` submits the blocking
  half FIRST and joins it from a task of its own, so a delivery that panicked —
  or one a shutting-down runtime never ran — reaches the log rather than
  vanishing, and a single-threaded runtime still starts the delivery before its
  caller awaits anything.
- **The wait is a `FrameClock` primitive, not a bare condvar call.**
  `LockedFor::wait_until(condvar, timeout, ready)` (timing.rs) ends the frame's
  hold, clears the holder slot, waits with the mutex given back, and charges the
  reacquisition to lock wait. A frame that waited would otherwise report the
  wait as `held` and name itself as the lock holder while holding nothing.
- **A delivery is a frame of its own.** `DeliveryRunner::run` opens a
  `FrameTimer` under `AGENT_DELIVERY_METHOD` (`agent.deliver`), so `bridge.stats`
  reports a cold spawn's seconds against the delivery rather than against every
  verb that ever spoke to an agent. The clock travels in `PendingTurns`, taken
  under the acquisition that took the turns: the runner's first act is never a
  bare acquisition of the app mutex to find the clock it will time itself by.
- **The session gate is asked where the answer is atomic.** An issue whose
  session is over (approved, abandoned) holds no workspace, and its checkout is
  the project's primary one — no place to spawn a replacement for work nobody is
  doing. **Deviation:** the declared design filtered the queue in
  `take_pending_turns`; delivery now outlives the frame that queued it, so the
  gate can close between the take and the spawn. `ensure_agent_tab` asks
  `owner_still_has_a_session` twice — in the reserve acquisition, which costs
  nothing and skips the spawn, and again in the acquisition that publishes the
  tab, which is the only check atomic with the insert; a tab published into a
  closed session is retired in that same acquisition. Both answer `Ok(None)`,
  which the runner logs and never records as a delivery failure.
- **"The agent is already coming" has one owner and one lifetime.** A turn
  passes through three states — queued, off the queue and mid-delivery, claimed
  by the spawn that delivery makes — and the delivery gives its in-flight mark
  back only after settling the claim it became, so the mark covers the claim's
  whole lifetime and the states leave no window. `agent_is_on_its_way` is the
  only way to ask; the revive/nudge guard and `start_routed_issue_agent` used
  to compute it by hand from the queue and the claim alone, which since this
  step's split reads false for the whole of a delivery. A second message
  landing there queued a duplicate turn: the claim still stops a second
  harness, nothing stopped the duplicate `read_unread_messages` nudge. The key
  is per (root, agent_id), so a branch's second agent is never suppressed by
  its first agent's turn.
- **Only a turn with words is "already coming".** The guard's premise is that
  the harness on its way opens on a cold prompt telling it to call
  `read_unread_messages`, so the message just posted is read. A textless turn
  (`agent.start` with nothing unread) opens a harness and sends it nothing, and
  a message posted between the button and the harness would sit on the thread
  with nobody told. So `TurnsInFlight` counts a turn under its agent key only
  when it `says_something()` (`TurnMark.told_agent: Option<TabKey>`),
  `agent_is_on_its_way` ignores textless queued turns, and the bare
  `agent_spawns_in_flight` disjunct is gone: a claim with no textful mark
  behind it is a textless spawn. A post during one queues its own revive turn,
  which waits out the claim in `ensure_agent_tab` and lands Warm on the tab the
  start opened. Two states, then: queued, and mid-delivery.
- **A mark travels with its turn.** `take_pending_turns` pairs each turn with
  the mark `TurnsInFlight::take` produced for it, `next_turn` hands both back,
  and `TurnMark::settle` consumes the one for the turn that landed. Looked up
  by owner alone, a batch carrying two turns for one owner on two agents could
  settle the OTHER agent's mark and leave its undelivered turn reading as
  absent — the window the per-agent count exists to close.
- **A reservation fails before it takes.** `reserve_agent_spawn` resolves the
  owner's project (`project_of`, or `default_project` for a router) and clones
  the orchestrator FIRST; only then does it retire the dead tab keeping its
  screen, sweep the stale owners' tabs, register the MCP token and take the
  claim. `SpawnHolding::abandon` is the one primitive that gives those back,
  and a `?` between the take and the holding would bypass it: the carried grid
  dropped without a close, the token registered for no child. The function's
  only failure arm now runs with the registry untouched.
- **Lock discipline** Three acquisitions. **Take**: the queue, the in-flight
  marks, the reserved tab id (`agent_tab_id(agent_id)` — the agent's own
  identity mints it, so no registry entry is needed to name it), one
  acquisition, so the idle sweep never sees a gap. **Reserve**: the project
  lookup and the orchestrator clone (the only reads that can fail, so they go
  first), then `retire_tab_keeping_screen` for the dead tab this spawn replaces
  and `retire_tab` for the stale-owner sweep (receipts dropped), the probe
  inputs, the session token, the claim. **Run**: probe,
  scaffold, build the spec, spawn, `send_turn` — none. **Apply**: insert the
  tab, consume the claim, `record_agent_resume_id(.., None)` when
  `recorded_name_is_gone`, `record_agent_session_start` /
  `record_agent_delivery_failure`, `note_entity_changed`. A verb that queues a
  turn answers with the reserved tab id; the outcome reaches the browser through
  the entity's push event.
- **Tests** `a_message_is_answered_before_its_agent_has_spawned` (which also
  proves a board read answers while a cold spawn is parked),
  `agent_start_answers_with_the_reserved_tab_before_the_harness_is_up`,
  `a_delivery_that_fails_in_the_background_lands_on_its_entity`,
  `a_delivery_that_panics_gives_its_in_flight_marks_back`,
  `a_second_message_queues_nothing_while_the_first_is_mid_delivery`,
  `a_message_posted_during_a_textless_start_queues_its_own_turn`,
  `settling_one_agents_turn_leaves_the_other_agents_mark_in_flight`,
  `a_reservation_that_cannot_resolve_its_project_takes_nothing_from_the_registry`,
  `a_spawn_that_panics_gives_its_claim_back`,
  `a_claim_dropped_by_a_panic_under_the_app_mutex_does_not_deadlock`,
  `two_callers_of_one_tab_spawn_one_harness_without_spinning`,
  `a_delivery_is_timed_under_its_own_method_and_never_the_frames`,
  `a_frame_waiting_on_a_condvar_holds_nothing_and_charges_the_wait_to_the_lock`
  (timing.rs), and the four pick-up-rule tests in `delivery.rs`. Already
  standing and now covering the apply phase:
  `a_recorded_name_the_provider_no_longer_holds_is_cleared_before_it_is_spent`,
  `a_client_attaching_inside_a_respawn_is_carried_without_rewinding_the_cursor`.

## 4. The diff cache — one read, one owner

**Shipped** with step 3, as declared: the three reads, `refresh_now` /
`refresh_if_stale`, `ScanRead`, `note_worktree_appeared` /
`note_worktree_gone`, and every deletion listed below. Thirteen deviations from
the sketch, each argued where it appears:

1. `board.list` gains a sibling `"scanning"` boolean rather than turning
   `external_worktrees` into `{"worktrees": [], "scanning": true}` — the SPA
   reads that key as an array (`taskFeed.js:45`) and step 5 owns the SPA. The
   emptiness is still rendered rather than stored: `external_worktrees_json`
   returns `ExternalWorktreeRows { rows, scanning }`, and `scanning` is
   `ScanRead.settled` folded over the projects it read.
2. A landed scan or summary calls `note_board_changed` when what it found
   differs from what was there, and having found nothing there at all counts
   as a difference (`store_diff_entry`). Without it nothing tells the browser
   to ask again, and a board that answered `scanning` — or `stat: null` —
   would sit empty until the next poll; the spec's "publishes through
   `publish_diff_refresh` and `note_board_changed`" is this line. A run's
   diffstat separates the two facts one `changed` flag was conflating: two
   computes that disagree are files that landed in the checkout, which is a
   filesystem event (`run_files_changed_at`, `note_entity_changed`), while a
   first compute is no such event but IS the answer a board is still waiting
   for, so only the board push fires.
3. `force` is deleted, but only one of its three callers could go: the forced
   retry inside `resolve_external_worktree`. The other two — `run_adopt`
   (app.rs:12489) and `bare_checkout_on_branch` (13609) — call
   `scan_external_worktrees_now`, which is the old forced arm under a name that
   says what it does, with a doc comment naming §5 as what removes it. Nothing
   else may call it; the tests that want a scan on the spot do.
4. `note_checkout_created(project_id, path)` sits in front of
   `note_worktree_appeared`: `worktree_create` and `cut_branch_for_dispatch`
   hold a path, not an `ExternalWorktree`. It builds one through
   `crate::worktree::describe_checkout` (new, beside `describe_primary_checkout`
   — one `git worktree list` and one checkout's summary, sharing
   `describe_checkouts` and the new `sort_checkouts` with the full scan), and
   falls back to a rescan if the description fails. Both callers already run
   `git worktree add` under the lock; §5 moves the pair off it together. A
   project whose scan has never landed is returned from before anything is
   described: there is nowhere to put the answer, the first scan is what finds
   this checkout anyway, and the amend that would have followed neither
   supersedes that scan nor pushes an invalidation for an edit it did not make.
   `amend_external_scan` is where that is decided, so only an edit that
   happened supersedes a scan and notes the board.
5. `rescan_external_worktrees(project_id)` is what the five
   `invalidate_external_scan` sites became that name neither an appearance nor a
   removal: a git-scope mutation inside a checkout, a branch switch, a released
   or deleted run handing its checkout back, and a run coming off the board for
   its finish. Each starts the scan now and keeps serving the last list —
   emptying it was the behaviour the step removes.
6. `refresh_now` computes inline, `#[cfg(test)]` only, when there is no runtime
   and no shared handle to publish through (`compute_without_a_runtime`).
   Production releases the claim there, as before. The synchronous tests hold
   `AppState` directly — no mutex, nobody waiting — and this is what lets ~1470
   of them keep asserting on numbers; the concurrency tests all run through the
   real `FrameHandler`, where the refresh spawns.
7. `ScanRead.ever_scanned` is `settled`, because a scan attempt can settle
   without landing a list. `DiffCacheEntry::ExternalScanUnreadable` records a
   repository this daemon could not read (`Project.external_scan_failed_at`),
   the interval is measured from whichever attempt settled last
   (`scan_settled_at`), and the list of checkouts stays whatever the last
   readable scan left. Without it a project that can never be scanned is walked
   again by every poll and answers `scanning: true` for the life of the daemon.
8. "Nothing has looked yet" and "there is no such checkout" are one sentence in
   one place: `scan_may_yet_show_it(settled)`, reached through `find_checkout`,
   which is the whole miss — read the cache, look, claim the rescan the refusal
   promises, and refuse in the caller's own words. `resolve_external_worktree`,
   `branch.finish`'s bare-checkout arm and `entity.dismiss`'s branch row miss
   through it; `branch.get`, which misses a work item rather than a checkout,
   ends its own refusal with the same sentence and claims the same rescan —
   from `scan_settled_at` for the project it was asked about, never from
   `ExternalWorktreeRows.scanning`, which is the rail's board-wide flag and
   would blame an unscanned neighbour for a miss on a project that is fully
   scanned. Before this step `warm_diff_caches` computed the first scan ahead
   of all four, so none of them could see a cold cache and `no checkout of this
   project is on branch <b>` was never a lie.
9. `primary_row` answers `Result`. A row built from a summary that has not
   landed carries no head, and `entity.dismiss` on it writes a dismissal at an
   empty head that the walk's own first result revokes — a click that silently
   did nothing, and a base branch that `entity.dismiss` refuses by name in the
   same window. It refuses instead, naming the walk it has just claimed.
10. The attention map is pruned against the scan only for the keys the scan
    mints. Sparing every key while any project was unscanned stopped run, plan
    and branch-row pruning daemon-wide, and a repository that could never be
    scanned grew the map for the life of the daemon.
    `crate::worktree::is_checkout_id` is the test, and it lives beside the mint.
11. `board.list`'s own sweep of runs whose checkout vanished moved off the lock
    with the reads, which is what the open gate note asked this step to settle:
    it is off the lock, not deliberately kept.
    `archive_runs_with_deleted_worktrees` opened every board read, and deciding
    whether one stage's commits had ever been published is `bounded_git_fetch`
    — a `git fetch` with a thirty-second deadline — plus two graph walks, per
    stage, under the mutex every other frame is waiting on. Split the way the
    reads are: `StagePublicationQuery` is what one run's stages must be judged
    against, taken under the lock; `VanishedRunSweep::decide` is the git, on the
    blocking pool holding nothing; `archive_vanished_runs` writes the verdicts
    back and archives, re-checking that the checkout is still gone and the run
    still there. Single-flight, so a board polling faster than a fetch returns
    claims one sweep rather than one per poll, and inline with no runtime under
    it, as deviation 6 has it. `reconcile_missing_run_worktree` is now pure
    bookkeeping over a decided `StagePublications`; its two remaining callers —
    `run.abandon` and a failed recovery — ask git through `classify_stages_now`,
    which names what it does so the two sites §5 moves stay visible.

12. A mutation **supersedes** the refresh it overtakes rather than releasing
    its claim (`diff_refreshes_superseded`, `supersede_diff_refresh`), and
    `publish_diff_refresh` drops what a superseded refresh computed. Releasing
    was two bugs in one: the claim is single-flight, so the very next read
    started a SECOND compute of the same thing behind the first, and the claim
    is also the right to publish, so the first, pre-edit one landed on the
    second's claim, stored its stale answer over the edit, and left the second
    to be discarded. A checkout created during a scan was off the rail and
    unresolvable for a whole interval; a checkout adopted during one came back
    as the unbound card `run_adopt` must never act on. All three sites take the
    same mechanism — `amend_external_scan`, `invalidate_run_stat`,
    `invalidate_primary_summary` — and the reader that is superseded meanwhile
    is answered from the amended cache, which is the newer truth.
13. `store_diff_entry` is a four-line dispatch over `store_run_stat`,
    `store_external_scan`, `store_scan_failure` and `store_primary_summary`.
    The match stays the single construction point where a kind of entry names
    its cache; each arm is that cache's own write, resolved through one
    `project_mut` — the lookup every write into a project's caches goes through.

`amend_external_scan` does not restamp `scanned_at`: an edit knows about one
checkout and the rest of the list is exactly as old as it was, so the
reconciling scan is not pushed back an interval. Its closure answers whether it
changed the list, and only a change supersedes the scan and notes the board: a
checkout bound to a run is excluded from the scan, so `note_worktree_gone` for
one — `run.finish`'s failure branch, the finish epilogue — removes nothing,
and an amendment that removed nothing must not drop the fresh list a running
scan is about to land (`a_removal_of_a_checkout_the_scan_never_had_leaves_the_running_scan_alone`,
`re_noting_an_unchanged_checkout_leaves_the_running_scan_alone`).

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
- **Tests** all five shipped under their declared names, plus
  `a_landed_first_scan_invalidates_the_browser` and
  `a_landed_first_diffstat_invalidates_the_browser` (deviation 2),
  `one_checkout_describes_itself_the_way_the_scan_describes_it` /
  `a_checkout_outside_the_repository_cannot_be_described` in `worktree.rs` and
  `a_create_before_the_first_scan_leaves_the_running_scan_alone` (deviation 4),
  `a_scan_that_cannot_read_its_repository_settles_the_board` (deviation 7),
  `a_missed_checkout_says_whether_a_scan_has_ever_landed` (deviation 8),
  `dismissing_the_primary_row_before_its_walk_lands_is_refused` (deviation 9),
  `attention_survives_a_stamp_taken_before_the_first_scan` /
  `attention_for_a_dead_run_is_pruned_before_the_first_scan` (deviation 10),
  `a_vanished_runs_stages_are_judged_with_the_state_lock_free`
  (deviation 11), `a_create_during_a_scan_outlives_that_scans_landing` and
  `an_invalidated_stat_discards_the_compute_it_overtook` (deviation 12). The four stale-while-revalidate tests are unchanged in what
  they pin; each now seeds its cache by waiting for the refresh a first poll
  claimed (`seeded_run_stat`) instead of by making that poll compute.
  `a_frame_waiting_for_a_first_compute_charges_its_polls_to_the_lock` is
  deleted with `wait_for_first_diff_value`, the only thing it described.

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

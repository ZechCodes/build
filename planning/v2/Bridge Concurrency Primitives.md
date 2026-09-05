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

Every caller but two drops the receipt: the tab is out of the registry, which
is what stops the agent being addressable. `DiscardCheckout` (§5) waits, for a
reason argued there on its own merits — it is not today's order. The router's
scratch directory is the other: `abandon_router_session` used to wipe it right
after a synchronous `session.end()`, and once the kill moved onto a thread the
`remove_dir_all` under the mutex was walking a directory a live harness was
still writing into — the same failing walk §5 argues about, plus a filesystem
walk under the lock. `reaper::remove_dir_once_reaped(writers, dir, timeout,
subject)` is the one rule for both: rename the directory aside under the lock
(one bounded syscall, so a re-fired router can `create_dir_all` the same path
at once), then on a thread that holds nothing `Retirement::wait_all` the
writers out — `CHECKOUT_REAP_WAIT`, logged on expiry — and remove what was
renamed. `DiscardedCheckout::discard` shares `wait_all`; its removal is
`Orchestrator::discard_checkout`, which a rename cannot stand in for.
Test: `cancelling_a_capture_wipes_its_scratch_once_the_router_is_reaped`.

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
  its own, since it is not a wire method. The clock is read off the state once,
  when the listener comes up, and cloned into every connection: a harness
  dialling in used to make one bare acquisition of the app mutex to find the
  clock it would be timed by, the acquisition §3 forbids the delivery runner
  for the same reason. The git a socket line hands back — a router's tool, a
  coding agent's report — runs through one `apply_off_the_socket` (blocking
  thread, then `apply_deferred` under the line's timer), where the two arms
  had spelled it twice.
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

- **A turn for a checkout in flight waits for it.** The one acquisition that
  takes the queue also reads the rows §5's lifecycle verbs reserved, and leaves
  behind any turn whose owner holds one. Spawning an agent scaffolds its
  checkout directory, and `WorktreeManager::restore`'s `git worktree add`
  refuses a path that reappeared under it (worktree.rs:342) — which
  `settle_restored_checkout` reads as a lost branch and answers by handing a
  healthy run to the verified recovery agent, blocking the Issue. The row is
  already the claim on that directory, so it is the claim here too; the drain
  that runs after the job's epilogue takes what was left, and every frame
  drains.
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
  which the runner logs and never records as a delivery failure. The gate
  covers a router's owner too: a capture has a session only while
  `router_sessions` holds one for it, so a `capture.cancel` landing between
  the frame that queued the router's first turn and the delivery that would
  spawn it leaves nothing spawned and no scratch scaffolded back
  (`a_capture_cancelled_before_its_router_spawns_gets_no_router`). As first
  shipped the gate answered `true` for every non-Issue owner, routers named
  among them, and the cancelled capture's harness came up anyway. A reroute
  re-fires under the same capture id, so a turn queued for the session it
  replaced still passes; that turn spawns the retired agent id and is the
  same leak as before this step.
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
    bookkeeping over a decided `StagePublications`. Its two remaining callers
    still ask git under the lock through `classify_stages_now` (app.rs:13992),
    which names the violation it preserves. Where each lands is §5's
    "`DiscardCheckout` judges the stages before it removes the checkout":
    `run_abandon`'s call moves into `DiscardCheckout::perform`, ahead of the
    removal, and `recover_run`'s failed-recovery arm stays, because it runs at
    boot before any frame exists.

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
14. The spawn is one primitive, `OffLockJob`, not one helper per payload.
    `spawn_diff_refresh` and `spawn_vanished_run_sweep` were the same function
    written twice — `Handle::try_current()` or hand the job back,
    `spawn_blocking` the git, take the lock, apply or give the claim back —
    and `refresh_now` and `sweep_vanished_runs` each repeated the other half:
    upgrade `self_handle`, spawn, decide inline when nothing could carry it.
    Now `OffLockJob { type Claim; type Decided; claim(&self); decide(self);
    apply(state, claim, decided); abandon(state, claim) }` is the trait,
    `spawn_off_lock` puts one on the runtime, and `AppState::run_off_lock` is
    the one place "decide off the lock, apply under it" is written, with
    `decide_without_a_runtime` as the no-runtime fallback (deviation 6).
    `DiffRefreshJob` (claim `DiffCacheKey`, apply `publish_diff_refresh`,
    abandon `release_diff_refresh`) and `VanishedRunSweep` (claim `()`, apply
    `archive_vanished_runs`, abandon clears `vanished_run_sweep_in_flight`)
    implement it. The single-flight claim itself stays with each cache —
    `diff_refreshes_in_flight`, `vanished_run_sweep_in_flight` — because what
    is claimed differs (a key, a daemon-wide flag); what was duplicated was the
    carrying, and that is what the trait owns. `Claim` exists so a decide phase
    that never returns (a panic on the blocking pool) can still hand back what
    it held through `abandon`, which sees no job. §5's `WorktreeLifecycleJob`
    runs through `DeferredWork`'s drain on the frame's own worker; a lifecycle
    verb that must leave the frame is a third implementor, not a third spawn.
    Tests: `an_off_lock_job_with_no_runtime_decides_inline_and_applies`,
    `an_off_lock_job_under_a_runtime_applies_what_it_decided`,
    `an_off_lock_job_whose_decide_panics_gives_its_claim_back`.

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
  stale entry end alike: an answer now, `run_off_lock` behind it,
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

**Shipped, first half** (`worktree.create` and `branch.dispatch`;
`bridge/src/lifecycle.rs`). The primitive is in place as declared — decide
under the lock, `perform` holding nothing, apply under it again, with the
reservation rolled back on a failure — and `DeferredWork::Lifecycle` carries
it through the drain `DeferredWork::Finish` already used. What the build
settled differently, and why:

- **The epilogues live in `app.rs`, not `lifecycle.rs`.** A sibling module
  cannot reach `AppState`'s private methods, and an epilogue is nothing but
  those. So `lifecycle.rs` holds the job, the traits, the reservation and the
  mutations — the halves that hold no state — and each verb's `impl
  LifecycleEpilogue` sits beside the records it writes. The boundary the doc
  wanted is stronger for it: a mutation cannot name a private `AppState`
  method even by accident.
- **`ReservedName` and `ReservedCheckout` are one impl, `ReservedRow`.** Both
  claims ARE the row, and what a second verb collides with is a field of it, so
  two empty `roll_back`s would have been one type spelled twice.
  `PendingRow` gained `branch: Option<String>` to carry the ref a verb is
  claiming, and both verbs fill it: a create reserves the `build/<slug>` it is
  cutting, a dispatch the ref its `DispatchTarget` settled. Neither has a
  checkout id yet, and the branch is the only identity two of them share.
  `reserve_row` refuses on any of the three — same `entity_id`, same `branch`,
  same `checkout_id` — within one project. `TakenRun` and `MintedProject`
  arrive with the verbs that need them.
- **`DispatchTarget` settles the ref in the decide phase.** Deriving it inside
  the git phase left `PendingRow.branch` empty for the common case (no branch
  named), so two dispatches of one instruction reserved nothing in common and
  raced into `git worktree add` with the same slug —
  `WorktreeManager::name_taken` is a check-then-act, and the app mutex was what
  used to serialize them. `is_usable_branch_name` and `slugify` are pure, so
  the ref is read out of what the caller said before anything is reserved
  (through `worktree::branch_name_for`, the one place `build/<slug>` is
  spelled — a reservation that did not match the ref `WorktreeManager` cuts
  would make the collision check silently stop firing):
  `Named(ref)` for a ref spelled out (its existing checkout is taken over),
  `Minted { branch, slug }` for words (nothing on disk is taken over, and the
  slug namespace suffixes). The type owns its own variation — which ref, which
  git call, what a scan looks for — so no caller matches on it.
- **The row is reserved inside the deferral.** `release_row` is reachable only
  through `apply_lifecycle`, so a `?` between the reservation and the deferral
  would leave a row standing on the board forever, refusing every later verb
  that matches it. So the two are one call:
  `defer_lifecycle(row, mutation) -> Result<Value, String>` reserves, wraps the
  row in `ReservedRow`, builds the job, holds it open for the tests and stores
  it. `reserve_row` has one caller, `ReservedRow` is private and
  `WorktreeLifecycleJob::reserving` is its only constructor, so "nothing
  fallible runs in between" is a property of the type rather than of an
  ordering a future verb has to remember. Each verb's decide phase ends in one
  `self.defer_lifecycle(row, Box::new(mutation))`.
- **The row is shared, not copied.** `AppState.pending_rows` and the
  reservation both hold `Arc<PendingRow>`, so the board and the job cannot
  disagree about the row in flight.
- **`WorktreeChange` gained `rescan: bool`** — a checkout that is on disk and
  could not be described. The amendment cannot carry it, so the apply phase
  claims the project's scan instead.
- **`board.list` ships them under a new top-level `pending` key**, not folded
  into `items[]`: a row of an unknown `kind` is one an old client cannot
  render, and a key it does not read is one it ignores. `pending_rows_json`
  takes no project id, because `board.list` is every project's.
- **The placeholder id is minted as declared**, through
  `Orchestrator::planned_checkout_path` and a new
  `worktree::canonical_planned_path` (the deepest existing ancestor
  canonicalized, the missing segments joined back on), with the boot
  canonicalization of the worktrees root in `main.rs`. The reply carries
  `pending_worktree_id` beside `worktree_id`; they differ only when
  `WorktreeManager::create` had to suffix the slug.
- **`DeferredWork::Finish` has NOT collapsed into `Lifecycle`.** That collapse
  rewrites `run.finish` and `branch.finish`, which are the second half's verbs;
  it belongs to the step that moves them.
- **`adopt_run` split exactly as declared** — `AdoptableCheckout::judge`,
  `Orchestrator::prepare_adoption` (checkpoint + scaffold),
  `Orchestrator::adopt_run` (pure) — and the free `lifecycle::adopt` that owns
  the order is built. It returns `RunAdopted`, not `Performed`: a dispatch owes
  its instruction on top of the adoption, and a `Box<dyn LifecycleEpilogue>`
  cannot be wrapped. `RunAdopted::open_run` is the other half — `adopt_run`,
  `entity_project.insert`, `forget_row_dismissals`, `note_worktree_gone` — and
  it hands the run back UNPERSISTED, so its caller adds what it owes and writes
  once. `run.adopt` calls both under the app mutex still, which is the same git
  it ran before; the second half moves it, and the sequence is already spelled
  once for it to move.
- **One store write per dispatch, after every decision.** The apply half opened
  the run with `finish_run_mutation` and `dispatch_to_run` took it straight
  back out, mutated it and persisted again — a window in which a failure
  stranded a cut branch, a full checkout and a checkpoint commit under no run.
  `dispatch_to_run` now mutates the `ActiveRun` its caller is holding and
  returns the ids; `open_dispatched_run` (the cut arm) and `join_dispatched_run`
  (the branch Build already runs) each own the take and the single write.
- **An epilogue that fails re-amends the project's checkouts.**
  `apply_lifecycle` rolls the reservation back on a failed `perform`, but a
  failed `apply` is the harder half: the git already ran, so what it made is on
  disk whatever the records say. The amendment is applied again over whatever
  the epilogue got through, which leaves the checkout on the board as the
  unowned card it is — invisible until the next full rescan is how a minted
  checkout gets lost.
- **Every dispatch goes through the drain, the router's included.**
  `dispatch_branch_now` is deleted. `route_to_branch`'s two callers are an
  ordinary frame (`capture.reroute`) and the MCP control socket, and both used
  to hold the mutex through a whole checkout. The capture rides the dispatch
  now — `DispatchCheckout.routed`, carried into `BranchDispatched` — so the
  route is recorded in the apply phase, against the branch that is real by
  then, and each caller names what it answers with (a `fn` field: the capture's
  own row for the reroute, where the work went for the router's tool). The
  socket takes the job out under the guard and runs it with the guard released,
  as `dispatch_frame` does.
- **`note_checkout_created` is deleted.** Both callers describe their checkout
  in the run phase now, off the lock, and hand it back as
  `WorktreeChange::appeared`.
- **Tests** `worktree_create_runs_git_worktree_add_with_the_state_lock_free`
  and `branch_dispatch_cuts_its_branch_with_the_state_lock_free` (both hold the
  git open and answer `board.list` and `thread.post` meanwhile),
  `a_creating_worktree_is_on_the_board_before_its_git_returns`,
  `a_create_that_fails_rolls_its_reservation_back_and_leaves_no_row`,
  `a_suffixed_slug_settles_the_placeholder_under_its_real_id`,
  `a_second_create_of_a_name_being_cut_is_refused`,
  `two_dispatches_of_one_instruction_cut_one_branch`,
  `a_dispatch_onto_a_branch_being_created_is_refused`,
  `a_dispatch_that_fails_after_its_git_leaves_the_checkout_on_the_board`,
  `a_dispatch_that_fails_after_queuing_its_turn_delivers_nothing`,
  `rerouting_a_capture_to_a_branch_cuts_it_with_the_state_lock_free`,
  `a_router_dispatch_over_the_socket_cuts_its_branch_with_the_state_lock_free`,
  `a_capture_cancelled_while_its_dispatch_cuts_the_branch_refuses_before_the_run_is_durable`,
  `a_refused_agent_remove_that_dropped_an_earlier_turn_is_answered_not_a_panic`,
  and the injected-failure dispatch tests, which now also assert the
  reservation is gone.
- **One fault carrier, one seam per variant.** `BranchDispatchStep` and
  `fail_dispatch_at` live in `lifecycle.rs` beside the mutation, and the second
  `AppState` copy is deleted. `Adopt` and `Own` are the git phase's two seams,
  `Open` the apply phase's, `Post` the branch Build already runs, `Settle` the
  window after an agent has been handed the instruction and before the write
  that makes its run real.
- **Dropping a failed request's turns is `AppState`'s rule.** A turn is not
  deliverable until the mutation that queued it is durable, and the rule lived
  in `dispatch_frame` alone: the MCP control socket's drain and both
  synchronous test twins skipped it, so a socket dispatch that failed in its
  apply phase — after `dispatch_to_run` had queued the branch agent's first
  turn — spawned a harness for a run with no record. `dispatch_deferring`,
  `apply_deferred` and the router's own `router_deferring` own it now, through
  one `drop_turns_queued_since`, and every drain inherits it — the socket
  answers a router tool through the same twin its tests do. `PendingAgentTurn.survives_refusal` is the exception the
  rule needs: a recovery is written down and started and THEN its verb refuses
  its caller to say so, so that turn outlives the refusal while everything
  else's is dropped.
- **The socket's lifecycle job runs on `spawn_blocking`.** It ran on the
  connection's own tokio task, so several routers dispatching at once parked
  that many runtime workers — the relay's read loop and every other harness's
  done socket behind minutes of git. The app mutex was free throughout; this
  was the executor.
- **The worktrees root is fatal at boot.** `canonical_root` falls back to the
  path as given, so a root that could not be created leaves every placeholder
  id minted from a path the checkouts never land on. `main.rs` panics with the
  path named rather than swallowing the error.
- **A dispatch's route is written before the write that opens its run.**
  `answer_dispatch` recorded the capture's route after `finish_run_mutation`,
  and `record_routing` can refuse — the capture can be cancelled while the app
  mutex is free for the git, and its store write can fail — which left a
  persisted run reported as a failure, its turn dropped and its checkout
  re-amended onto the board as a second card. `record_dispatch_route` now runs
  ahead of `open_run` on the cut arm and ahead of `take_run` on the join arm,
  `record_routing` takes the entity id its caller knows instead of looking
  the run up by branch, and `RouteRecorded::answer` — what is left after the
  write — cannot refuse. `answer_dispatch` is deleted.
- **`drop_turns_queued_since` clamps its index.** A request that retires an
  agent drops that agent's turns however early they were queued, so a refusal
  after that can find the queue shorter than it measured; `split_off` past the
  end panicked under the app mutex and poisoned it. The split is at the shorter
  of the measured length and the queue's.

**Shipped, second half** (`run.adopt`, `run.abandon`, `run.delete`,
`run.release`, `project.add` / `.clone` / `.create` / `.set_remote`). What the
build settled differently, and why:

- **`run.adopt` has one mutation, not two.** `AdoptExternalCheckout` and
  `AdoptPrimaryCheckout` differ in exactly one step — which git finds the
  checkout — and are identical afterwards (judge, checkpoint, scaffold,
  `RunAdopted`). That is one mutation, `AdoptCheckout`, over an
  `AdoptionTarget` that owns its own variation the way `DispatchTarget` does:
  `Card { worktree_id, excluded }` scans, `Primary { repo_path }` runs
  `describe_primary_checkout`, and each answers `scope()`, `checkout_id()` (the
  identity two adoptions collide on, known before any git) and `amendment()`
  (a card goes back on the board if the record fails; the primary checkout was
  never a card and must not become one). Two mutations spelled the same
  sequence twice.
- **Convergence is an answer that names no run, and the re-ask is what
  converges.** The decide phase has three early returns: a checkout that
  already has an owner answers with that run's view; a checkout an adoption is
  opening right now — the row `AppState::row_claiming` finds, the one rule for
  what two lifecycle verbs collide on — answers `{"adopting": true}` and
  nothing else; everything else reserves. Answering `{"run_id", "adopting":
  true}` from the row was a reply promising state that is not durable: that
  run is not in `self.runs` until the epilogue lands, and an adoption whose
  `perform` fails never mints it at all, while `createScopedAdoptingCall`
  (spa/src/core/adoption.js) caches the id and fires the next verb against it.
  A refusal was the wrong word too. The asker has nothing to correct, and this
  step put the adopt's git behind its answer, so the asker's own `run.adopt`
  can outlive the 12 s timer while succeeding — a client that read the timer
  as a refusal reverted the message it had just sent and the starting state it
  had just laid, and its retry met the standing row's refusal instead of
  converging. So the client asks again: `adoptUntilNamed` (adoption.js) reads
  every reply through `replyOrNothing`, treats the timer and a reply naming no
  run alike, waits `ADOPT_REASK_MS` and asks once more, up to
  `ADOPT_REASK_LIMIT`; the one `adoptInFlight` chain stays alive for every
  action queued behind it, and only a refusal clears it. Once the row is
  released, `primary_run_of` / `run_owning_worktree_id` answer the re-ask with
  the one owner. `PendingState::leaves_a_record` went with it: the state is
  rendered and nothing branches on it.
- **A checkout no run owns is adopted through the same job now.**
  `adopt_implementation_checkout`'s `None` arm no longer calls `run_adopt`
  under the mutex (the note in the first half's build said this was the
  second half's to do). `AdoptImplementation.checkout` became an
  `ImplementationCheckout`: `Owned(path)` is the run the branch already had,
  `Unowned(AdoptionTarget)` runs `lifecycle::adopt` first and hands the
  `RunAdopted` it earned to the epilogue, which opens that run instead of
  taking one off the board. One job, two git steps in its one run phase — an
  epilogue still never defers a second job.
- **`TakenRun` was not built. The run travels in the mutation.** Nothing in a
  discard's run phase can fail: the stage classification, the reap wait and
  the removal are all best-effort by contract (`WorktreeManager::remove`'s
  failure has always been logged, never fatal), so `DiscardCheckout::perform`
  has no `?` and the run cannot be stranded off the board. A reservation that
  puts the run back would only be reachable on a path that does not exist. The
  legality of the abandon is judged in the decide phase instead
  (`run_transition(state, Abandon)`, pure), so the epilogue's verdict cannot
  refuse — `AdoptableCheckout::judge`'s rule, applied to the run.
- **`run.delete` uses the same mutation, and the same decide phase.** It is the
  other caller of `Orchestrator::discard_worktree`, and it ran that under the
  mutex. One `DiscardedCheckout` covers all three outcomes — `Removed`
  (directory goes, branch stays: abandon), `Pruned` (both go: delete), `Kept`
  (nothing on disk is touched) — and owns the wait, which only a removal needs.
  `Orchestrator::discard_worktree` and `discard_checkout_keeping_branch`
  collapse into one public `discard_checkout(worktree, keep_branch)`: whether
  the branch stays is the caller's fact and was never derivable there.
  Everything the two verbs share — the row, the run coming out of the map, the
  cached stat, the agents retired — is one `AppState::discard_run`, and each
  verb is its own refusals plus the three things that differ (which
  `DiscardedCheckout`, which `DiscardSettlement`, what the row is called).
- **`run.delete`'s durable record is deleted in the settlement, not the decide
  phase.** The fused verb deleted the store row first because nothing after it
  could fail; a decide phase that reserves can be refused — `run.adopt` and
  `issue.implement_*` claim the same checkout id, and a terminal run is exactly
  the owner an adoption walks past — so the delete now writes where the refusal
  cannot reach it. `DiscardCheckout::perform` cannot fail, so reaching
  `RunDeleted::settle` is what says the delete is happening; a crash in between
  leaves the record for boot to reload and the vanished-run sweep to archive,
  the story every other reservation already has. The one thing that can still
  refuse there is the store, and a refused `delete_run` puts the `ActiveRun`
  the git phase carried back under its id before answering with the error:
  the record stands, so the card stands (its checkout gone, which the
  vanished-run sweep archives), and the delete is retried like any other
  failed write. Dropping the run on that path cleared the card with the
  record intact, which a restart then brought back
  (`a_delete_the_store_refuses_puts_the_run_back_on_the_board`).
- **The `Orchestrator` rides in the arms that prune with it.**
  `DiscardedCheckout::Removed` and `Pruned` carry `{ project, worktree }`;
  `Kept` carries nothing, because there is a discard with no orchestrator to
  reach for. A run recovered after its repository moved off disk has no
  `entity_project` entry (`recover_run` inserts one only when
  `record.project_path` still exists), and that stale card is exactly what
  `run.delete` is for — so the delete resolves `Option<String>` /
  `Option<Orchestrator>`, forces `Kept` when either is absent, and leaves the
  directory alone. A required `project` field on the mutation would have made
  the one card that most needs deleting undeletable.
- **What each discard still owes is a `DiscardSettlement`, and it says what
  git it needs.** `RunAbandoned` writes the verdict, the stage reconciliation
  and the Issue's lineage; `RunDeleted` clears the card. `settle` takes only
  the `ActiveRun`, which the git phase carried; the stage verdict is
  `RunAbandoned`'s own, asked through `judge_before_removal` — a trait method
  with an empty default body, so a settlement that judges nothing declares
  nothing and `run.delete` builds no `StagePublicationQuery` and pays for no
  `bounded_git_fetch`. Passing every settlement a verdict only one of them
  reads hid which verb needs a pre-removal question, and would have charged a
  plan-less run carrying stage progress a 30 s-bounded fetch per stage for an
  answer it discards. The trait is the whole cross-module surface:
  `RunAbandoned`, `RunDeleted`, `StagePublicationQuery` and
  `StagePublications` are private to `app.rs`, built and boxed there, and
  `lifecycle::DiscardCheckout` holds a `Box<dyn DiscardSettlement>` and
  nothing narrower.
- **`run.release` builds no job, as declared.** Re-read against the code: a
  store delete, a map remove, `retire_agent_tabs` (receipts dropped — the kill
  is already a thread's) and `rescan_external_worktrees` (already a spawn).
  Its map cleanup and `run.delete`'s were the same seven lines twice, now
  `AppState::forget_run`.
- **`MintedProject` was not built either.** `add_project` mints from its own
  counter and is idempotent by canonical path; minting in the decide phase
  would put that fact in two places for a row nobody renders. What two project
  verbs actually contend over is the directory, so the row's `entity_id` IS the
  destination's checkout id — `project.clone` and `project.create` racing one
  folder collide on the board instead of in `git clone`, and
  `project.set_remote` reserves the repository it is about to rewrite through
  the same door, so two of those are one. `PendingRow.project_id` is
  `Option<String>` and every project row's is `None`: nothing the board lists
  stands where such a row does, so `pending_rows_json` skips them rather than
  handing `board.list` a row whose `entity_id` is a folder hash beside rows
  whose ids are runs and checkouts. `PendingState` gained `Updating` for
  `set_remote`, which mints nothing and takes nothing away — borrowing
  `Creating` told the board a registered project was being created, and told
  the user's refusal message the same. The rows that do render now name their
  project through `project_name_by_id`; `project_name_of` reads `entity_project`
  and had been answering the empty string on every pending row since the row
  was introduced.
- **`project.create` moved with the other three.** It is four subprocesses
  (`init`, `add`, `commit`, `remote add`) under the mutex and the same
  primitive, so leaving it would have been the one project door still holding
  the lock. `OpenRepo`, `CloneRepo` and `CreateRepo` all end in one
  `open_repo(path, requested_base)` — canonicalize, open, default branch,
  revparse, origin — and produce one `ProjectAdded`. Each removes what it made
  in its own error path (`CloneRepo` a directory the clone left half-written,
  `CreateRepo` a repository it did not finish), and `CreateRepo` removes
  nothing when the directory was already there. All three take the destination
  as a field — `CreateRepo` carries the `dest` the decide phase reserved its
  row under rather than re-joining `parent` and `name` — so the directory a
  project verb guards and the directory it writes are one fact, computed once.
- **`project_json` takes the remote it was told.** It shelled out for
  `git remote get-url origin` on every call, which put a subprocess inside
  every project epilogue. The four mutating verbs pass what their own git read
  or wrote; `project.list` reads it at its call site, where it is visible — the
  one remaining project-family read under the mutex, argued in the audit below.
- **The git subprocess helpers live in `worktree.rs`.** `git_in`,
  `git_stdout`, `git_remote_origin`, `git_default_branch` and `remotes_match`
  sit beside `bounded_git_fetch`, `pub(crate)`, and the mutations import them
  from there. They had been widened in `app.rs` for `lifecycle.rs` to reach,
  which made the state module the home of five functions that read nothing
  from it and gave `lifecycle.rs` a dependency on `app.rs` beyond the
  epilogues. `app.rs` exports `AppState`, the epilogues and the settlement
  trait, which is the surface this document declares.
- **`defer_lifecycle_holding` is the second reservation door.** `run.abandon`
  and `run.delete` take the run out of the registry between reserving the row
  and building the job, and a refusal in between would have left the run
  stranded. One call reserves, then runs an infallible `take`, then defers —
  so "nothing fallible runs between a row and the job that releases it" stays a
  property of the type. `reserve_lifecycle` and it share `lifecycle_job`, the
  one place a job is held open for the tests.
- **`scan_external_worktrees_now` is `#[cfg(test)]` now.** No production caller
  is left: every verb that must decide against the checkouts that exist asks
  in a run phase. `run_on_worktree` is deleted as a duplicate of
  `run_owning_worktree_id`.
- **Tests** `run_adopt_of_the_primary_checkout_reads_git_with_the_state_lock_free`,
  `run_adopt_answers_from_its_epilogue_with_the_runs_own_view`,
  `two_adopts_of_one_checkout_converge_on_one_run`,
  `run_adopt_refuses_a_detached_head_before_it_writes_a_checkpoint`,
  `run_abandon_removes_its_checkout_with_the_state_lock_free`,
  `run_abandon_judges_its_stages_with_the_state_lock_free`,
  `run_abandon_waits_for_its_agents_to_die_before_removing_the_checkout`,
  `run_abandon_removes_the_checkout_anyway_when_an_agent_will_not_die`,
  `run_delete_clears_an_adopted_card_and_leaves_the_checkout_standing`,
  `a_delete_refused_by_a_running_adopt_keeps_the_runs_record`,
  `a_second_create_of_one_directory_is_refused_by_the_row_guarding_it`,
  `project_add_reads_the_default_branch_with_the_state_lock_free`,
  `project_clone_registers_its_project_from_the_landed_path`,
  `project_create_writes_its_repository_with_the_state_lock_free`,
  `project_set_remote_writes_its_config_with_the_state_lock_free`,
  `a_clone_that_fails_rolls_its_reservation_back_and_leaves_no_row`,
  `a_run_whose_project_is_gone_is_still_deletable`,
  `discarding_a_run_drops_the_stat_the_board_cached_for_it`,
  `a_project_verb_reserves_its_directory_without_a_row_on_the_board`.
- **`PendingRow` is built through constructors, not a literal.**
  `PendingRow::creating` / `::discarding` / `::on_directory` with
  `.on_branch()`, `.on_checkout()` and `.implementing()` — so `since` and the
  three optional claims are spelled in the module that owns the concept rather
  than at eleven decide phases, and a new verb cannot forget one.
- **`settle_abandoned_run` is three functions.** The run's own verdict stays;
  `close_abandoned_run_conversations(&mut ActiveRun)` takes the primary
  thread's close and the per-agent death loop, and `record_abandon_on_issue`
  takes both halves of the Issue write, so the `Option<issue_id>` is
  destructured once at the call site instead of twice in a row.
- **`DiscardedCheckout::Pruned` is defensive, not reachable from today's board.**
  `run.delete` prunes only a run that is plan-less AND not adopted, and every
  plan-less run the daemon mints now comes through `adopt` (`run.adopt` and
  `branch.dispatch` both), so the live arms are `Kept` and — from
  `run.abandon` — `Removed`. The arm stays because a run record persisted by an
  older bridge can still load as one, and the condition is the one the fused
  `run_delete` used before the split; changing it would change what a delete
  does to a directory, which is not this step's to decide.

**What still runs git under the app mutex**, after grepping `app.rs` above its
test module for every `git2::Repository::open`, `Command::new("git")`,
`git_in`, `git_stdout`, `git_default_branch`, `git_remote_origin`,
`bounded_git_fetch`, `discover_external_worktrees`,
`describe_primary_checkout`, `classify_stage_publication` and every
`Orchestrator` method that reaches one of them. Five verbs remain, none of
them a lifecycle verb, each one somebody else's migration:

1. `project_json`'s `git_remote_origin`, on `project.list` only (app.rs:6548).
   A read verb, not a lifecycle one. Caching it on `Project` would make the
   shown remote lie the moment the user edits `.git/config`; keeping it live
   and lock-free needs the deferred-read surface (`DeferredRead`/`ReadSubject`)
   generalized past diffs, which is its own change.
2. `run.git_action` — the whole verb (`run_git_action`, app.rs:12848). The
   write-ahead `run_commit` + `git rev-parse HEAD` the publication contract
   depends on (12884-12885), the action itself (`run_commit` / `run_push` /
   `run_approve_merge` / `run_merge_and_push`, 12907-12910), and the
   `discard_checkout` a `Prune` cleanup ends on (13026). A push or a merge is
   the longest git in the daemon and the one most worth moving, and it is a
   whole publication protocol — the write-ahead intent, the attempt record,
   the recovery on the far side — not a checkout being cut. `run.git_action`'s
   own migration.
3. `dispatch_run_stage`'s `git rev-parse HEAD` (orchestrator.rs:2286), reached
   by `run.stage_dispatch` (app.rs:12676) and by `auto_advance_run`
   (app.rs:12822). Named in the first half's build notes;
   `run.stage_dispatch`'s own migration.
4. The MCP `done` report's two: `on_run_agent_done`'s `Orchestrator::run_diff`
   (app.rs:4989), which renders the completed run's whole patch for the thread
   event, and `consume_recovery_report`'s `restore_run_worktree` plus a `git2`
   open to verify the reported HEAD (app.rs:5201). The `done` report's own
   path.
5. `prune_worktree_records` (`git worktree prune`, app.rs:14954), called from
   `archive_vanished_runs`'s apply phase — the write-back of a sweep whose git
   is already off the lock.

Two more the grep finds and neither is a hold anybody waits on:
`recover_run`'s failed-recovery arm (`classify_stages_now`, app.rs:2437) and
`advance_issue_scheduler_here` (10395) are boot-only, before the first frame
is served; `scan_external_worktrees_now` (4183) is `#[cfg(test)]`.
`sweep_vanished_runs`'s `classify_stage_publication` was the open gate note's
question and has been off the lock since §4 (`VanishedRunSweep` is an
`OffLockJob`, and `reconcile_missing_run_worktree` is now pure bookkeeping over
the `StagePublications` that job carries back).

Everything else the grep finds is inside a `DiffCacheRefresh::compute`, a
`WorktreeFinishJob::run`, a `DeferredRead`, an `OffLockJob::decide`, a
`WorktreeMutation::perform`, or a test.

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
      /// Reserve the row and hand the git to the drain, in one call: nothing
      /// fallible can run between a row and the job that releases it.
      fn defer_lifecycle(&mut self, row: PendingRow, mutation: Box<dyn WorktreeMutation>)
          -> Result<Value, String>;                                      // placeholder
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
                                 target: DispatchTarget, instruction: String, excluded: Vec<PathBuf>,
                                 routed: Option<RoutedCapture>, .. }
  struct DiscardCheckout       { project: Orchestrator, worktree: Worktree,
                                 retirements: Vec<Retirement>,
                                 stages: StagePublicationQuery }
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
  /// `DispatchCheckout`'s adopted arm. It hands back the epilogue itself, not a
  /// `Performed`: a dispatch wraps it with the instruction it still owes, and a
  /// boxed trait object cannot be wrapped.
  pub fn adopt(project: &Orchestrator, project_id: &str, checkout: &ExternalWorktree,
               base_branch: &str, scope: AdoptionScope, run_id: &str,
               model_choice: ModelChoice) -> Result<RunAdopted, String>;
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
                          /// The Issue an implementation is being opened for.
                          /// `ImplementableIssue::judge` reads the run map, and
                          /// the run it is about to open is not there until the
                          /// git lands; the row is the single-writer gate for
                          /// that window, so a second implementation of one
                          /// Issue is refused where a second create of one slug
                          /// and a second adopt of one checkout already are.
                          implements: Option<String>,
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
  | `run.create` / `issue.implement_*` | `OpenImplementation` | `ImplementationOpened` | open the run on the checkout that was cut, bind it to its issue |
  | `run.create` into an existing checkout | `AdoptImplementation` | `ImplementationAdopted` | reset the branch's run onto the baseline the checkpoint made (a checkout no run owns yet is adopted first, and that adoption is still `run_adopt`'s — see below) |
  | `issue.implement_*` with its checkout gone | `RestoreImplementationCheckout` | `RestoredCheckout` | write the recreated checkout onto the run, or hand the run to the recovery agent |
  | every door to a planning agent — `plan.create`, the first `thread.post` to an inert Issue, a route, `plan.send_notes`, `plan.stage_send_notes`, `plan.message` | `OpenPlanWorkspace` | `PlanWorkspaceOpened` / `PlanWorkspaceRefused`, over that door's `PlanSessionOpening` | apply the plan event the door was gated on, render its prompt, queue the turn |
  | `run.adopt` | `AdoptExternalCheckout` / `AdoptPrimaryCheckout` | `RunAdopted` | `adopt_run`'s record, `forget_row_dismissals`, `answer_run_mutation` / `run_view` |
  | `run.abandon` | `DiscardCheckout` | `RunAbandoned` | `abandon_run_keeping_checkout`, `reconcile_missing_run_worktree` over the `StagePublications` `perform` decided — written onto the `ActiveRun` the epilogue takes back from `TakenRun` — close the lineage, mirror the affected stages to the issue |
  | `worktree.finish` | `FinishWorktree` | `WorktreeArchived` | the archive record |
  | `run.finish` | `FinishWorktree` | `RunFinished` | retire the run (`active: Box<ActiveRun>`) |
  | `branch.finish` | `FinishWorktree` | `BranchFinished` | retire the run and settle the issue |
  | `project.add` | `OpenRepo` | `ProjectAdded` | `register_project`, `persist` |
  | `project.clone` | `CloneRepo` | `ProjectAdded` | the same |

  A reply that needs `AppState` — `run.adopt`'s `run_view`, every
  `answer_run_mutation` — is built in the epilogue, which is why
  `WorktreeChange` carries no `reply`.
- **A checkout no run owns yet is adopted under the mutex, until `run.adopt`
  moves.** `adopt_implementation_checkout`'s decide phase (app.rs:11454) calls
  `run_adopt` inline when `worktree_id` names a checkout with no live run —
  the forced `scan_external_worktrees_now` plus `lifecycle::adopt`'s checkpoint
  commit and scaffold, all under the app mutex, exactly where they ran before
  this step. Chaining it is not this step's to do: an adoption is its own
  `AdoptExternalCheckout` job, an epilogue may not defer a second job into a
  drain that has already run, and `run.adopt` has not moved yet. The common
  path — a `worktree_id` a run already owns, and every `run.create` that names
  none — is off the lock. When `run.adopt` moves, this decide phase reserves
  the checkout and hands the git to `AdoptExternalCheckout`, resuming
  `AdoptImplementation` from its epilogue.
- **What an apply-phase failure leaves on disk.** `perform` removes what it
  cut in its own error path, but an epilogue can fail after the git returned
  `Ok` — the Issue was deleted while the git ran, a store write failed — and
  the removal is git, which the epilogue may not run. So the checkout stays,
  and the rule is that it stays VISIBLE: `OpenImplementation::perform`
  describes the checkout it cut as `WorktreeChange::appeared`, and
  `open_implementation_run` calls `note_worktree_gone` once the run owns it, so
  the amendment `apply_lifecycle` re-applies over a failed epilogue leaves the
  checkout on the board as the unbound card it is. That is the same end state
  the `PendingRow` deviation argues for: nothing is left that git and the next
  scan cannot re-derive. `plan.create` is the one that leaves something no
  board shows — `IssueOpened` failing after `OpenPlanWorkspace` leaves
  the Issue's scratch docs dir and the `.build/` config in the primary
  checkout. Neither is a checkout or a branch: the config is overwritten by the
  next plan the project drafts, and the docs dir is `discard_plan_docs_dir`'s
  (orchestrator.rs:1268), which every approve and every abandon runs. A
  scratch dir for a plan that never existed outlives them, and is left.
- **A planning workspace is written once, by one mutation, for every door.**
  `Orchestrator::ensure_plan_workspace` is deleted, and with it the second
  implementation of the same fact: `prepare_plan_workspace` now holds every
  disk touch a workspace needs — the scratch docs dir, the `.build/` config in
  the primary checkout, and the refill of an empty docs dir from the canonical
  store — and `OpenPlanWorkspace` is its only caller. What the doors differ in
  is the plan event they were gated on and what they say to the agent, which
  is the apply half:

  ```rust
  pub trait PlanSessionOpening: Send {
      fn open(self: Box<Self>, state: &mut AppState, workspace: PlanWorkspace)
          -> Result<Value, String>;
      /// The workspace could not be written. An error for every door that
      /// asked for a session; a routed capture says "no agent is reading it"
      /// instead and overrides this.
      fn refused(self: Box<Self>, state: &mut AppState, error: String)
          -> Result<Value, String> { Err(error) }
  }
  ```

  Six impls: `IssueOpened` (`plan.create`), `PlanDraftingStarted` (the first
  message to an inert Issue), `RoutedIssueDrafting` (a router or a reroute),
  `PlanNotesSent`, `StageNotesSent`, `PlanMessaged`. The orchestrator's five
  session verbs split the same way — `send_plan_notes` /
  `send_plan_stage_notes` / `message_plan` / `resume_plan` /
  `start_plan_drafting` become the pure gates `gate_plan_stage_notes` and
  `gate_plan_message` (`plan_transition` is the gate for the other three)
  beside `open_plan_notes` / `open_plan_stage_notes` / `open_plan_message` /
  `open_plan_resume` / `open_plan_drafting`, each of which takes the
  `PlanWorkspace` by value. The gate runs in the decide phase, so an illegal
  revise still scaffolds nothing.

  The row a door reserves stands on the Issue itself: what it holds is the one
  workspace every door writes into, so a second door waits rather than racing
  this one's `.build/` config. `run.release`'s neighbour rule applies —
  `checkout_is_in_flight` holds that Issue's queued turns back for the length
  of the write, and `apply_lifecycle` releases the row before the epilogue
  queues its own.
- **An implementation's epilogue answers to whoever asked, not to a verb.**
  `run.create` and `issue.implement_*` cut the same checkout by the same three
  mutations; what differs is who is waiting. That is one object, carried by the
  mutation into the epilogue:

  ```rust
  pub trait ImplementationCaller: Send {
      fn opened(self: Box<Self>, state: &mut AppState, run_id: &str) -> Result<Value, String>;
      /// The message the frame gets, after whatever the decide phase armed on
      /// the strength of this implementation has been settled.
      fn refused(self: Box<Self>, state: &mut AppState, error: String) -> String;
  }
  ```

  `RunOpenedView` answers with the run and refuses with the error unchanged;
  `IssueSchedulerWaiting` carries on to the stage the checkout was cut for
  (`dispatch_ready_stage`), answers with the Issue, and on a refusal blocks the
  Issue's scheduler — an Issue left saying `Preparing` with nothing preparing
  it is a spinner nothing will ever clear.

  These three mutations therefore hand a **failure to the apply phase as an
  epilogue** (`ImplementationRefused`, and `RestoredCheckout`'s `Err`) rather
  than returning it from `perform`: what a refusal leaves behind is state — a
  blocked Issue, a verified recovery agent started on a run whose branch is
  gone — and state is written under the mutex. `Reservation::roll_back` is for
  registry writes, and it cannot see the error that caused them to be undone.
- **The Issue scheduler hands its git back rather than running it.**
  `advance_issue_scheduler` returns `Result<Option<WorktreeLifecycleJob>>`, and
  every caller that has a drain reaches it through one call,
  `defer_issue_scheduler`: put the job on the drain, or block the Issue on a
  refusal. Three callers have one — `issue.implement_*`, the stage approval
  that wakes a scheduler parked on an unapproved stage (`plan_stage_approve`,
  which serves both `issue.stage_approve` and `plan.stage_approve`), and an
  agent's own recovery report on the MCP `done` socket. The approval's is an
  ordinary frame, and the report's socket already releases its guard for a
  router tool's git; both cut whole implementation checkouts, so neither may
  run one under the mutex. What each answers with is unchanged: the approval
  answers with the Issue view it was going to answer with anyway, read after
  the hop rather than before it, and the report answers with nothing.
  `advance_issue_scheduler_here` is left to boot (app.rs:2173), which
  reconciles every armed Issue before the first frame is served and has no
  drain to hand git to. The pass resumes from the job's own epilogue
  (`dispatch_ready_stage`), so no caller decides anything but where the git
  runs, and no epilogue ever defers a second job into a drain that has already
  run. That resume is not lock-free: its `ImplementationIntent::Stage` arm is
  `dispatch_ready_stage` → `run_stage_dispatch` →
  `Orchestrator::dispatch_run_stage`, whose `git rev-parse HEAD`
  (orchestrator.rs:2276) runs under the app mutex. It is `run.stage_dispatch`'s
  own git, bounded in practice and not in spec finding 2, and moving it is
  `run.stage_dispatch`'s migration to make, not this step's — so an
  `issue.implement_stage` still ends in one git subprocess under the lock,
  after its checkout was cut with the lock free.

  The `done` socket gets `dispatch_frame`'s shape for it: `done_deferring`
  routes the report and hands back `deferred_work`, `spawn_blocking` runs it
  with the guard released, `apply_deferred` writes it down. `on_agent_done`
  survives as the synchronous test twin that drains inline, the way
  `AppState::dispatch` is `dispatch_deferring`'s.
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
- **`DiscardCheckout` judges the stages before it removes the checkout.**
  `run_abandon` (app.rs:12364) runs `classify_stages_now` (13992) between
  `take_run` and the removal, under the app mutex: `classify_stage_publication`
  (16599) per completed stage is a `bounded_git_fetch` (worktree.rs:436, a
  `git fetch` with a thirty-second deadline) plus two graph walks, and the
  comment above the call says it must run while the refs are still
  inspectable. That ordering belongs to the run phase, not the decide phase.
  The decide phase takes the `StagePublicationQuery` (§4 deviation 11: run id,
  `repo_path`, branch, base, each completed stage's `completion_sha`) through
  `stage_publication_query` (13964) — the same query the vanished-run sweep
  takes — and puts it in `DiscardCheckout`. `perform` calls
  `stages.classify()` first, then waits the retirements out, then removes, and
  returns the `StagePublications` inside `Performed`, carried by the
  `RunAbandoned` epilogue it builds. The epilogue takes the `ActiveRun` back
  from `TakenRun`, runs `reconcile_missing_run_worktree` over the decided
  publications (pure bookkeeping: `publication` and `invalidation_reason` on
  each stage), and mirrors the affected stages to the issue (12425-12461). So
  `perform` never reaches the run, and the verdict is written by the same
  acquisition that writes everything else. A refusal or a failed removal
  rolls `TakenRun` back with the stages as they were — the publications were
  read, not written, until the epilogue. `classify_stages_now` keeps one
  caller, `recover_run`'s failed-recovery arm (2451), which runs at boot
  before the first frame is served and is kept under the lock on purpose:
  nothing waits on the mutex then.
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
  `plan_create`'s planning worktree, every other door to a planning agent
  (`plan_send_notes`, `plan_stage_send_notes`, `plan_message`, `route_to_issue`
  and `thread.post` to an inert Issue — `start_inert_plan` and
  `start_routed_issue_agent` are deleted for `reserve_plan_drafting` beside
  `open_inert_plan_drafting`), `run_adopt`, `run_abandon`, `project_add`
  and `project_clone` each stop calling git and return a job.
  `Orchestrator::dispatch_run`, `adopt_implementation` and `dispatch_plan` are
  deleted: each was the two halves of one of those verbs composed under its
  caller's lock, and each is now a `prepare_*` (the git) beside an `open_*`
  (the record) — with the refusals they opened with lifted into
  `ImplementableIssue::judge`, whose construction is the gate.
  `open_implementation_run` keeps the tail every implementation shares and
  stops building the answer, which is the caller's.
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
  `run_abandon_judges_its_stages_with_the_state_lock_free`,
  `a_creating_worktree_is_on_the_board_before_its_git_returns`,
  `a_create_that_fails_rolls_its_reservation_back_and_leaves_no_row`,
  `a_suffixed_slug_settles_the_placeholder_under_its_real_id`,
  `run_adopt_refuses_a_detached_head_before_it_writes_a_checkpoint`,
  `run_adopt_of_the_primary_checkout_reads_git_with_the_state_lock_free`,
  `two_adopts_of_one_checkout_converge_on_one_run`,
  `project_add_reads_the_default_branch_with_the_state_lock_free`,
  `project_clone_registers_its_project_from_the_landed_path`,
  `run_adopt_answers_from_its_epilogue_with_the_runs_own_view`,
  `run_create_opens_its_implementation_with_the_state_lock_free`,
  `issue_implement_all_opens_its_implementation_with_the_state_lock_free`,
  `implement_stage_restores_a_missing_checkout_with_the_state_lock_free`,
  `plan_create_prepares_its_workspace_with_the_state_lock_free`,
  `a_stage_revision_writes_its_workspace_with_the_state_lock_free`,
  `plan_notes_write_their_workspace_with_the_state_lock_free`,
  `an_inert_issues_first_message_starts_its_session_off_the_lock`,
  `run_create_into_an_existing_checkout_checkpoints_it_with_the_state_lock_free`,
  `a_stage_approval_that_implements_cuts_its_checkout_with_the_state_lock_free`,
  `a_recovery_report_advances_its_scheduler_with_the_state_lock_free`,
  `a_turn_queued_for_a_restoring_checkout_never_sends_its_run_to_recovery`,
  `an_implementation_whose_apply_fails_leaves_its_checkout_on_the_board`.

## What builds no primitive

`agent.choose` (app.rs:8200) opens no tab and spawns nothing: it validates a
model choice against the agent's locked harness, writes it, and answers.

Step 5 (SPA), as built. The overlay's patch records name what clears them
(`patchRecord(key, fields, { clearedBy })`, core/optimistic.js): a patch whose
row the entity will never carry as a field stands until the pushed entry
answers it, instead of waiting out `PENDING_GRACE_MS`. `AGENT_STARTING` and
`agentSessionAnswered` (core/agentRailModel.js) are what the rail lays over the
row it just asked for a session for, and `agentIsUp` is what keeps a message
sent behind that press from starting a second harness. The vocabulary stays in
the agent's own module; the overlay stays general.

The question a start asks has THREE answers, so the patch waits for whichever
comes. A session is live; one never opened; or none will open, because the
entity's session is over. The last two say nothing about a session, so waiting
on liveness alone left the ring on for the whole grace and then dropped it
silently, with the reason nowhere. Both travel on the agent that was to hear the
turn: `record_agent_delivery_failure` writes `Agent.start_error` beside the
entity's `last_error` in the one mutation it already makes, and
`record_agent_start_declined` (`DeliveryRunner::run`'s `Ok(None)` arm) writes
`AGENT_START_DECLINED_SESSION_OVER` on `start_error` alone — the entity's
`last_error` is left untouched, because nothing about the work failed. The
digest ships it, and the next turn on its way to that agent forgets it — one
write, in `take_pending_turns`, which is the one door every queued turn passes
through before the verb that queued it has even answered. The rail says it on
the agent's own bubble and raises it once, where the throw used to land. The
starting state itself is one record, `startingRecord` (agentRail.js), laid by
both verbs that ask for a session: the Resume press and the message that wakes
a cold agent (`deliverMessage`), which is also what makes `agentIsUp` hold on
the message path so two sends open one harness.

A reply the browser stopped waiting for is not a refusal. `replyOrNothing(pending)`
in core/session.js is the whole rule — the reply, or null when the timer ended the
call, and a refusal still raises — beside the 12 s timer, which is unchanged. The
predicate that reads the rejection is the module's own and is not exported: one
rule, one answer, so no call site can re-derive it and reach a different verdict.
Six verbs read it: `worktree.create` and `branch.dispatch` shut their form and
let the board carry the work, `issue.implement_*` refreshes the issue rather than
reporting a refusal the daemon never made, `thread.post` leaves the message on
the thread and the draft box empty — the turn is durable the moment the daemon
answers, and handing the draft back would have the human send it again and the
agent hear it twice — `run.adopt` keeps its one adoption in flight and asks again
(`adoptUntilNamed`, §5 second half), so the action behind it neither sends a
second adopt nor reads a refusal the daemon never made, and `agent.start`, on the
post's wake and on Resume, leaves the row wearing `AGENT_STARTING` rather than
reverting it, so nothing paints a failure over a harness the daemon is spawning. A reply that lands but names
nothing is the same story told by the payload instead of by the timer, and reads
the same way: `agent.start` takes the agent off the entity's next answer,
`thread.post` leaves the provisional message for the next thread read to replace,
and `worktree.create` opens nothing and leaves the row where it stands.

The `Creating` row is the daemon's, not the client's: `board.list`'s `pending`
carries one row per lifecycle verb in flight, published before the git runs, and
`mergePendingRows` (core/inbox.js) merges it into the feed. A row names two ids
and a listed card may carry either — the record it will settle as (`entity_id`)
or the checkout it holds (`checkout_id`, an `external_worktree_id` hash, while
the card is keyed by its run id) — so a card matching either is the card the verb
is running on and wears the state; only a row matching no card stands on its own.
One listed card carries no id at all: a project's primary checkout is the
repository, with a null `worktree_id`, `run_id` and `issue_id`. Adopting it is a
verb that acts on that card, so `AdoptionTarget::reserve` marks its row
`primary` beside the checkout id and the merge matches it by project — the
alternative was a second row for the repo root standing beside the card for the
whole of `describe_primary_checkout` plus the checkpoint and the scaffold, while
that card went on offering verbs the row refuses.
An adopt that leaves its run on the board and a `plan.create` that names only its
issue both settle onto the card that is already there, which is what keeps two
entries from patching one key. So `createBranch` needs no provisional row of its
own and the placeholder needs no `rekey`: there is one row, and the record
replaces it in place — `apply_lifecycle` releases the row and runs the epilogue
under one acquisition, so no snapshot ever carries both. A row with a verb in
flight offers no verbs; every one of them would race the verb already running.
Only a PLACEHOLDER opens nowhere, and that is a question about the row rather
than about the verb: a merged row is a card that already exists, and a plan
workspace being cut in an issue or a checkout being restored to a run must not
stop the reader opening their own issue or run. The state string is never
branched on — `pendingItem` marks the row it invents, and `entryRoute` reads
that mark.

The spec's load test, `bridge/tests/concurrency_load.rs`, is the only one that
measures a number.

# Agent Session Interface — Spec

**Status:** Draft — steps 0–7 shipped: the ADK is a provider and the first
carrier with no terminal, and an outcome is a status on the agent's own
message (see §10)
**Last updated:** August 24, 2026
**Branch:** `build/agent-polymorphism`

---

## 1. What this is

Build currently has one kind of agent: a CLI wrapper in a full PTY. Claude's ADK
and Codex's app server are not that. They expose a **session protocol** — an
event stream of reasoning, tool uses and messages — with no terminal anywhere in
it. Pi and OpenCode are the same shape.

The `Harness` / `HarnessSession` split already on this branch made the *launch*
polymorphic and put the session behind a trait. It did not make the terminal
optional: `HarnessSession` required `subscribe() -> bytes`, `resize`,
`write_input` and `pid` of every session, because `PtySession` was the only
implementation there had ever been. Step 5a deleted that trait — `AgentSession`
is what a session is now — and §10 records what each of its calls became.

This spec defines the interface that sits between the UI and the harness when
the byte stream is set aside, and says what happens to every surface that
currently assumes one exists. The goal is a polymorphic interface to agent
wrappers — one set of calls the daemon makes whether the wrapper is a PTY or a
session protocol — so that ADK, the Codex app server, Pi and OpenCode can each
be a new implementation rather than a new subsystem. It is not a change in
functionality: every existing surface behaves exactly as it does today.

**The rule this spec adds to the four in the scope doc:** *the terminal is a
capability, not a guarantee.* A session either offers one or does not, and every
surface that touches an agent must render correctly for both.

**And the shape that follows from it:** a harness with no terminal does not get a
replacement for the terminal. Its reasoning, tool uses and messages go into the
**conversation** as typed status events (§5). There is no second tab, no second
input path and no second scrollback — the terminal survives only as the escape
hatch into a CLI wrapper Build can otherwise only see the outside of.

---

## 2. What already generalizes, and what does not

The two directions are not symmetric today, and that asymmetry is the whole
shape of the work.

### From the agent — already done

Everything an agent says to Build arrives over the MCP unix socket
(`bridge/src/mcp.rs`): `post_thread_message`, `done`, `read_unread_messages`,
`search_conversation`. Build has **never** parsed PTY bytes to learn what an
agent said. The scope doc's rule — "no scraping TUI output for state" — was
honoured, and the dividend is that this half of the interface needs no work at
all. An ADK session calls the same tools over the same socket.

### To the agent — one chokepoint, no interface

`deliver` (`bridge/src/app.rs:17279`) is documented as "the one pipe from Build
to a worktree's agent", and every turn Build sends goes through it. But it ends
in `HarnessSession::write_prompt`, which means bracketed-paste framing, a
sanitised prompt, and a submit key written 1500 ms later
(`REAL_TUI_SUBMIT_DELAY`). Those are keystroke mechanics. A session protocol
takes a turn as a value.

### Status — inferred, and the inference is wrong for an event stream

`agent_is_working` (`bridge/src/app.rs:969`) was four conjuncts — step 2 has
since replaced the last two with `status()`, but the reasoning is why:

```rust
matches!(tab.role, TabRole::Agent { .. })
    && tab.live
    && !tab.session.has_exited()
    && tab.session.idle_for() < AGENT_WORKING_WINDOW   // 30s since last byte
```

Three of the four read the PTY. The last one is a *guess*: an agent that paints
is working, an agent that has been quiet for 30 seconds is waiting for you.

That guess is the best a terminal can do and it is actively wrong for a session
protocol. A model reasoning for 45 seconds with no emitted output would be
reported as "waiting for you" — the rail dot would go dark mid-turn. An ADK
session knows when a turn began and ended; it must be allowed to say so.

---

## 3. The interface

Two traits. One is required of every agent; the other is a capability.

```rust
/// What Build needs from an agent. Every session implements this.
pub trait AgentSession: Send + Sync {
    /// Hand the agent one turn. Returns when the turn is accepted, not when
    /// it is finished.
    fn send_turn(&self, turn: &Turn) -> Result<(), HarnessError>;

    /// What the agent is doing right now.
    fn status(&self) -> AgentStatus;

    /// End the session and release whatever it holds.
    fn end(&self);

    /// The terminal, if this session has one. `None` is a normal answer.
    fn terminal(&self) -> Option<&dyn TerminalView> { None }
}

/// Full access to a harness Build can only see the outside of.
///
/// A CLI wrapper is opaque: Build knows what it launched and what it reported,
/// and everything in between is paint. The terminal is the escape hatch for
/// exactly that — the human drops in and sees what Build cannot. A harness that
/// reports its own reasoning and tool calls is not opaque, so it has nothing to
/// escape to, and offers none of this.
pub trait TerminalView: Send + Sync {
    fn subscribe(&self) -> broadcast::Receiver<Vec<u8>>;
    fn write_input(&self, bytes: &[u8]) -> Result<(), HarnessError>;
    fn resize(&self, size: PtySize) -> Result<(), HarnessError>;
    fn pid(&self) -> Option<u32>;
}
```

`terminal()` returning `Option` rather than a second trait object stored
separately is deliberate: it makes "does this agent have a basement?" a question
with one answer, asked in one place, instead of a flag that can disagree with
reality.

The capability is not "can this session paint". It is **"is this harness opaque
enough to need an escape hatch"** — and the answer is yes for a CLI wrapper and
no for a session protocol, because the second one already tells Build everything
the terminal would have shown.

### What happens to `HarnessSession`

It becomes `PtySession`'s business alone. `write_prompt`, `ready_within` and
`idle_for` stop being the daemon's vocabulary and become how the PTY
implementation *satisfies* `AgentSession`:

| `AgentSession` | PTY implementation | ADK / app server |
|---|---|---|
| `send_turn` | `ready_within` → framed paste → delayed submit | one protocol call |
| `status` | synthesized from `idle_for` (today's 30 s rule, unchanged) | reported turn boundaries |
| activity | nothing beyond lifecycle — the terminal carries it | reasoning, tool use and messages, posted to the conversation as typed events |
| `terminal()` | `Some(self)` | `None` |

### What the daemon still could not ask — the exit and quiescence surface

*Added 2026-08-23.* The shipped `AgentSession` covers the turn, the status and
the end. Reading the daemon's remaining `HarnessSession` call sites (the
inventory in step 5a) shows three questions it asks of every session that the
trait cannot answer yet, plus one contract that was never written down. Each
addition is minimal because a session of either carrier has a true answer of
its own — none of them re-admits the byte stream.

```rust
pub trait AgentSession: Send + Sync {
    // … send_turn, status, end, terminal …

    /// How long since the session last showed evidence of work — bytes
    /// painted for a PTY, protocol events read for a session protocol.
    fn quiet_for(&self) -> Duration;

    /// Whether the session ends within `timeout`. A single status poll is
    /// racy: a dying harness closes its pipes BEFORE its exit status is
    /// reapable, so a caller deciding whether a failed `send_turn` means
    /// "crashed" rather than "wedged" waits the lag out here.
    fn exited_within(&self, timeout: Duration) -> bool;

    /// The session's last words, once it has ended. `None` for a session
    /// that left none worth repeating.
    fn epitaph(&self) -> Option<String> {
        None
    }

    /// The session's own account of what it is doing. `None` is a normal
    /// answer, and the mirror of `terminal()` — the two are alternatives.
    fn activity(&self) -> Option<broadcast::Receiver<AgentActivity>> {
        None
    }
}
```

- **`quiet_for`** exists because the idle sweep asks a minutes-scale question
  that `status()` does not answer. `mark_idle_tasks` (`app.rs:5812`) and
  `agent_last_painted_at` (`app.rs:13479`) read `idle_for` today — the PTY's
  paint clock. The name changes because the meaning generalizes: a PTY answers
  from its last byte (behavior byte-identical to `idle_for`), a protocol
  session from its last read protocol event. This is not status duplicated:
  `Working`/`Waiting` is a 30-second-window judgement; the sweep's
  quiet-for-five-minutes anomaly clock is a different instrument, and both
  carriers can hold one honestly.

- **`exited_within`** is `deliver`'s crashed-versus-wedged wait
  (`app.rs:17310`, `PROMPT_WRITE_EXIT_GRACE`): a failed write to a harness
  that exits within the grace is a crash the sweep will explain, not an error
  to surface. Both carriers are subprocess-backed today, and any carrier that
  is not still has a truthful degenerate answer (`status()` already `Ended`).

- **`epitaph`** exists because `HarnessExit` (`app.rs:16411`) explains a crash
  with the harness's last words, and today those come from the last painted
  screen (`screen_epitaph`) — a surface a no-terminal session does not have.
  The split: the PTY implementation returns `None` (its screen belongs to the
  `Tab`, and the sweep keeps reading `tab.screen` exactly as it does now); a
  protocol session retains the last error it was *told* — the final `result`
  line's error text, or the last line of stderr — and hands that back. The
  sweep asks the screen first and the session second. This honours the
  no-scraping rule rather than bending it: the protocol epitaph is a reported
  value, and the screen epitaph stays what it always was — a human-legible
  crash surface, never parsed for state.

- **May `send_turn` block?** No, and the trait doc says so: **an
  implementation writes the turn out and returns — it never sleeps out a delay
  and never waits on the model.** The reason is where the turn is handed over.
  `deliver` takes the session handle out of the tab registry and calls
  `send_turn` (and the `exited_within` grace on the failure path) with the
  app-wide state lock RELEASED, so the main pipe imposes nothing; but
  `nudge_live_agent_tab` reads its caller's tab registry, so the in-place nudge
  speaks from *under* that lock, and every RPC, every terminal pump and the
  idle sweep queue behind it. Both carriers can hold the contract honestly: the
  PTY writes the framed paste and returns, leaving the submit key to be written
  off-thread `REAL_TUI_SUBMIT_DELAY` (1500 ms) later, and a protocol session
  returns as soon as the turn is written to the child's stdin. Writing it down
  is what keeps an implementer from learning it from a daemon that has gone
  quiet.

  This is the one thing the shipped code and this section disagreed about, and
  the disagreement was the wrong way round: the doc read "callers must not hold
  the state lock, and the daemon already honours it" while `deliver` held it
  across both the write and the 250 ms exit-race wait. Fixed on both sides —
  `Tab.session` is an `Arc<dyn AgentSession>` so a delivery can take the handle
  out of the registry, and `a_turn_travels_with_the_state_lock_released` keeps
  it that way.

### Stopping a turn, and naming the conversation

*Added 2026-08-28, from the live probes recorded in §10 step 6.* A carrier that
reports its own turn boundaries can also be told to end one, and it names the
conversation it is having. Two calls, both defaulting to "no", and neither
re-admits the byte stream.

```rust
pub trait AgentSession: Send + Sync {
    // … send_turn, status, quiet_for, exited_within, end, epitaph,
    //   terminal, activity …

    /// Whether this session can be told to stop the turn it is running.
    ///
    /// Asked without performing it: the agent digest answers the SPA with it
    /// before anyone presses anything. An implementation must answer from the
    /// same value `interrupt` refuses on, so the two cannot disagree.
    fn can_interrupt(&self) -> bool {
        false
    }

    /// Stop the turn the agent is running now, and return.
    ///
    /// **Never kills.** `end` is the kill and it is a different verb with a
    /// different lifetime: a session that answered `interrupt` is the same
    /// session afterwards, still holding its conversation, ready for the turn
    /// Build hands it next. A carrier that cannot stop a turn says so in a
    /// sentence rather than reaching for the kill.
    ///
    /// Returns promptly, for the same reason `send_turn` does: the in-place
    /// nudge speaks from under the app-wide state lock.
    fn interrupt(&self) -> Result<(), HarnessError> {
        Err(HarnessError::Unsupported(
            "this agent has no interrupt — open its terminal and press Esc".to_string(),
        ))
    }

    /// The id the harness gave the conversation this session is having, once it
    /// has announced one. What a respawn resumes BY NAME.
    ///
    /// `None` for a carrier that names no conversation, and for one that has
    /// not announced yet. Reported, never scraped: it is read off the protocol
    /// line the child sent, never out of a transcript directory.
    fn session_id(&self) -> Option<String> {
        None
    }
}
```

- **Why a defaulted method rather than `terminal()`'s `Option` capability.**
  The terminal is a property of the carrier, so `Option<&dyn TerminalView>`
  makes the question one answer asked in one place. An interrupt is not: it is
  announced at runtime, in the `capabilities` array of the child's own `init`
  line, so the same provider can answer differently on two versions of the same
  CLI. And a refusal here has something to say — the PTY's is "open its terminal
  and press Esc" — which a bare `None` cannot carry. So it is a call with a
  refusal, in the manner of `require_shell_kind`: refuse loudly and say where
  the thing actually lives, never fall back to something different.
  `can_interrupt` is the flag `terminal()` avoided, and it is safe here only
  because the implementation answers both from ONE value, held to it by a test
  asserting `interrupt()` refuses exactly when `can_interrupt()` is false.

- **The PTY stays unsupported, and does not map interrupt to ESC bytes.** Three
  reasons, in the order that decides it. ESC is a keystroke whose meaning
  belongs to the harness, not to Build: claude reads it as "stop", another
  harness closes a picker with it, a third clears the composer, and Build has
  no way to know which harness is on the other end of a `SubmitKey`. Enforcement
  is by observation, and a PTY reports no turn boundary — so Build could write
  the bytes and never learn whether anything stopped, which is exactly the state
  where the SPA would claim it stopped an agent that is still working. And the
  terminal is the basement and always accessible: the human who wants a full
  harness stopped drops in and presses Esc with the screen in front of them,
  which is a better outcome than Build pressing it blind. The refusal says so.

---

## 4. `AgentStatus`

```rust
pub enum AgentStatus {
    /// Starting, and not yet able to take a turn.
    Starting,
    /// Mid-turn. The rail dot pulses.
    Working,
    /// Idle at a prompt, waiting for the human.
    Waiting,
    /// Over. The code is `None` for a session with no process behind it.
    Ended { code: Option<i32> },
}
```

The PTY implementation maps its four conjuncts onto this and nothing else
changes: `Working` iff live, not exited, and painted inside 30 s. **The wire
field `working` keeps its exact current meaning for CLI harnesses.** That is the
property that makes this a refactor rather than a behaviour change.

`Starting` is new and worth calling out: today a spawning agent is indistinct
from a waiting one until it paints, and the SPA covers the gap with a spinner it
owns. Naming the state lets the rail stop guessing.

---

## 5. Activity is conversation, not a second stream

**Decided 2026-08-20:** an event-stream harness has no second tab. Its reasoning,
tool uses and non-MCP messages are posted into the **conversation** as typed
events.

This is not a new mechanism. The conversation already carries 32 typed events
alongside its messages (`ThreadEventKind`, `bridge/src/thread.rs:567`) — session
lifecycle, git activity, stage transitions, triage — and already renders them in
the timeline beside what people said. Agent activity becomes more of the same
kind of thing:

```rust
pub enum ThreadEventKind {
    // … the existing 32 …

    /// The agent thought out loud.
    Reasoning,
    /// The agent called a tool.
    ToolUse,
    /// A tool answered.
    ToolResult,
    /// The agent narrated. Distinct from a `post_thread_message`, which is the
    /// agent deliberately addressing the human.
    Narration,
}
```

### Why this is the better answer

It collapses three open problems into one that is already solved.

| Problem | With a second stream | As conversation events |
|---|---|---|
| Where does scrollback live? | a new bounded ring, lost on restart | the thread, durable, already persisted |
| How does a client reconnect? | a new cursor protocol | `thread.revision` — the cursor that exists |
| How do you talk to the agent from the second tab? | a second input path | there is no second tab; the composer is the only input |
| Does it pull the human in? | a new attention rule | `EventClass::Status` — already the rule |

That last row is what makes this safe, and it is worth being precise about,
because it is what the first draft of this spec got wrong.

### Attention is already solved

`EventClass` (`bridge/src/thread.rs:556`) splits every conversation item in two:
`Attention` marks the entry unread and says why; `Status` updates it underneath
the human and stays quiet. The class is intrinsic to the kind, decided once.

All four new kinds are **`Status`**. So:

- No unread badge from an agent thinking. The rail bubble's count is unmoved.
- No attention pull, no notification, no inbox reshuffle.
- `attention.rs`'s standing rule — *"Interaction is the human acting, never the
  agent… an agent commit must not move anything here"* — holds with no new code.

An agent's `post_thread_message` stays `Attention`, because that is the agent
choosing to address you. Its narration is `Status`, because it is not.

### The distinction that has to survive

The review product depends on the conversation staying readable. Putting activity
in it is safe **only** because of the class split, and the split has to be
enforced where the event is minted, not where it is rendered. The scope doc's
ruling stands with one word changed: the execution log is still the execution
log, and the conversation still carries durable messages and lifecycle statuses
— it now holds both, told apart by class rather than by living in two places.

Two mechanical consequences follow, and neither is optional. They are §6.

---

## 6. The two things this breaks

Both are in code that predates agent activity being conversational, and both must
be fixed in the same change that introduces the new kinds. Both are now decided.

### 6.1 The catch-up packet is messages only

**Decided 2026-08-20:** the catch-up packet a resumed agent is handed carries
**only messages to and from the agent** — nothing else on the thread.

`Thread::catch_up_markdown` (`bridge/src/thread.rs:2270`) took the last N items
by recency, N = 40, filtered only for completion messages:

```rust
for item in self.items.iter().rev().take(limit).rev() { … }
```

Nothing about class. A session that emitted forty tool calls before restarting
would hand its replacement forty tool calls and **none of the human's messages**
— the exact context the packet exists to carry. Filtering to messages fixes that
by construction rather than by tuning a ratio, and it does not need revisiting
when a fifth activity kind is added later.

**Shipped 2026-08-23 with the kinds**, and with one thing the sketch above does
not say: the filter runs **before** the take, so `limit` counts messages. Taking
forty items and then dropping the events would have handed that same session an
empty packet — the same failure in a different shape.

**What this drops, deliberately:** the packet's event branch emits a line for
every event carrying a summary. In practice that is four places — `Done`,
`Blocked` / `ReviewBlocked`, `RunFailed` / `IdleUnreported` / crash reasons, and
revision/approval events. So a resumed agent stops being told "you went quiet
without reporting done" or "the reviewer approved revision 3".

Two reasons that is the right trade:

1. Those are **Build's observations about the agent**, not the conversation. An
   agent that blocked said why in a message; the event is Build restating it for
   the human's timeline.
2. The structured completion report survives regardless —
   `conversation_prompt` appends `thread.last_completion` separately, after the
   packet (`bridge/src/orchestrator.rs:660`), so the densest of the four is not
   carried by this path anyway.

**Known gap, closed by step 7 — noted 2026-08-23, fixed 2026-08-24.** Reason 1 no longer holds for
outcomes reported through `done`: `post_completion` (`thread.rs:1604`)
deliberately posts no message — the event is the only record — and
`last_completion` is only set when a structured report exists, which a blocked
`done` does not carry. So a messages-only packet tells a replacement agent
nothing about why its predecessor blocked. This spec stays a refactor and does
not fix that here.

**The fix, designed 2026-08-23 — §10 step 7.** An outcome is a **status
attached to a message**, not a separate event.

- **What changes where it is minted.** `record_report_in_thread`
  (`app.rs:16348`) stops pushing `Done` / `Blocked` / `RunFailed` for a
  reported outcome. Instead the report's summary is posted as an ordinary
  **agent message** carrying an outcome, with the structured completion report
  attached to the message when the report wrote one. `post_completion` is
  retired with it; `remember_completion` / `last_completion` are untouched —
  they are set from the same report, after the same call.

- **Wire shape, additive.** A message's data gains one field:
  `"outcome": "completed" | "blocked" | "failed"`, absent on every message
  that is not an outcome. A completed outcome also sets the existing
  `done: true`, whose meaning does not move — a client that only knows `done`
  renders a completion exactly as it does today. The `completion_report`
  envelope the `Done` event carries moves onto the message, additive there
  too. No field changes meaning; nothing is removed from the wire.

- **Attention needs no new rule.** An outcome message is the agent addressing
  the human — `Attention` by the rule messages already have. The `Done`
  event's attention job moves onto the message whole; the count of unread
  entries per outcome stays exactly one.

- **The events are dropped, not reclassified.** Two reasons. First, keeping
  `Done`/`Blocked` beside the message as `Status` markers would be two records
  of one outcome, free to disagree — the flag-standing-beside-the-call shape
  this spec rejects everywhere else. Second, class is intrinsic to the kind
  and asked of persisted rows: quieting `Done` from `Attention` to `Status`
  would rewrite what every already-persisted `Done` row means. So the kinds
  stay on `ThreadEventKind` and keep their class — old rows deserialize and
  render unchanged — and nothing new emits them. `RunFailed`,
  `IdleUnreported` and `Interrupted` remain events: those are **Build's
  observations** about an agent that said nothing, and there is no agent
  message to attach them to.

- **The catch-up packet carries outcomes by construction.**
  `catch_up_markdown`'s message filter currently *excludes* completion
  messages (`message.done || source == Completion`) — an exclusion that
  existed because the event was the record and such a message was its
  duplicate. With the message the only record, the exclusion inverts: outcome
  messages are included, each line prefixed with its outcome. No event lines
  are re-admitted; a replacement agent reads why its predecessor blocked from
  the same packet that carries what the human said.

- **The Issue mirror moves with the record.** `run_outcome_mirrors_to_issue`
  keys on attention-classed *events*; once outcomes are messages, the mirror
  onto the Issue conversation must key on the outcome message instead. The
  step's tests hold the equivalence: every outcome that reached the Issue
  timeline before reaches it after.

- **Compatibility.** Existing persisted threads need no migration: their
  `Done`/`Blocked` events remain valid rows and render exactly as today, and
  an outcome message is an ordinary message row whose new field defaults
  absent on old records. An older SPA against a newer bridge ignores
  `outcome`: a completion still renders as a completion via `done`; a blocked
  outcome renders as a plain agent message stating the reason — strictly more
  than the nothing its thread shows today between the event it no longer
  receives and the packet that dropped it.

### 6.2 Activity cannot live in the aggregate record

> **Superseded 2026-08-21 by `Store Migration Spec.md`.** The conclusion below —
> that activity needs a per-agent jsonl log — was reasoning around a JSON store
> that rewrites a whole aggregate per append. That store is being replaced with
> SQLite, where a thread item is a row, appending is one `INSERT`, and paging is
> a `LIMIT`. The jsonl log is not needed and should not be built. The *diagnosis*
> below still stands and is why the store is changing; only the remedy is
> withdrawn.

This is the part the storage layer decides, not the design.

**There is no database.** The store is one `record.json` per Issue under
`~/.build/tasks/issues/<id>/`, holding the Issue *and every implementation
inside it*, each with its own thread. Records are written atomically — tmp file,
fsync, rename — and `save_issue_implementation` (the JSON store's, since removed) is a
read-modify-write of the whole aggregate:

```rust
let mut aggregate: PersistedIssue = read_record(&path)?;   // the entire Issue
… replace one implementation …
let json = serde_json::to_string_pretty(&aggregate)?;      // re-serialize all of it
write_record_atomically(&path, &json)                      // + fsync
```

So appending one thread item costs a full read, a full pretty-print
serialization, and an fsync **of every thread on that Issue**.

That is correct and cheap for what the thread holds today: messages and
lifecycle events, a handful per run. It is quadratic for activity. Tool events
arrive hundreds per session, and each one would rewrite a file that each previous
one made bigger — **O(n²) bytes written and fsynced over a session**, against a
disk, with the app mutex in the neighbourhood.

The problem is not that the file gets big. It is write amplification.

**So activity does not go in the aggregate record at all.** It goes in an
append-only log beside it:

```text
issues/<issue_id>/record.json              messages + lifecycle  (unchanged)
issues/<issue_id>/activity/<agent_id>.jsonl   reasoning, tool use, narration
```

- **Appending is O(1)** — one write, no read, no re-serialization of anything
  else. This is the whole point.
- `Thread::items` holds a **recent window** in memory and on the wire; older
  activity is read back from the log on demand.
- The log is per agent, so two agents on one checkout never contend, and
  deleting one agent's history never touches another's.

This is also a format Build already reads: claude keeps transcripts as
`~/.claude/projects/**/*.jsonl` and codex keeps dated JSONL rollouts, and the
transcript probes in `harness/claude.rs` and `harness/codex.rs` parse both today.
The activity log is the same shape, owned by Build.

**The alternative considered and rejected for now:** an embedded database
(sqlite, redb). It solves paging properly and would let the thread be queried
rather than loaded whole. But there is no database dependency in the bridge
today and the entire store is file-per-entity JSON, so introducing one for a
single subsystem is an architectural move that should be made deliberately and on
its own — not smuggled in behind an activity feed. The jsonl log can be migrated
into a database later; it cannot be un-migrated out of the aggregate record once
the write amplification has been shipped.

### 6.3 Activity must not flood the resident tail — open

> **Merged from main's reconciliation of this spec (written 2026-08-23, merged
> 2026-08-28). Not implemented** — steps 4 and 6 shipped without it, so this is
> the one §6 obligation still open now that a headless carrier is live.

A conversation is no longer fully resident: a boot reads the newest
`RESIDENT_CONVERSATION_TAIL` items (200, `store.rs:477`) and pages the rest
from the store. Lifecycle events arrive a handful per run, so that tail is
comfortably a conversation's working set — but a single session emits hundreds
of activity rows, and counted against the same bounds they push every message
the human and the agent exchanged out of the tail. Two silent failures follow:

- **The catch-up packet starves after a restart.** `catch_up_markdown` reads
  `self.items` — the tail. Its messages-only filter (§6.1) is right, but
  filtering a tail that holds no messages yields nothing, and the resumed
  agent the packet exists for is exactly the one that boots onto a tail an
  activity-heavy predecessor filled.
- **The first page shows no conversation.** `DEFAULT_THREAD_PAGE` is 60
  items; opening a conversation mid-session paints tool calls with the last
  thing anyone said somewhere below them.

So activity must not be counted against either bound: the catch-up packet is
built from a store query for messages rather than the resident tail (the store
already tells the kinds apart — a `WHERE`, not a scan), and the page a client
opens on is measured in conversation, with the activity between two messages
travelling folded beside them instead of consuming the budget that decides how
far back the human can see.

## 7. Terminal-coupled surfaces — the inventory

Everything that must become conditional. This is the actual size of the work.

### Bridge

| Site | Today | Change |
|---|---|---|
| `Tab.session` (`app.rs:676`) | `Box<dyn HarnessSession>` | `Arc<dyn AgentSession>` — **shipped**; shared so a turn travels with the state lock released |
| `Tab.screen` (`app.rs:682`) | always a `TermScreen` | `Option<TermScreen>` — no grid without a terminal |
| `agent_is_working` (`app.rs:969`) | reads `idle_for` | reads `status()` |
| `agent_attach` (`app.rs`) | attaches a grid, defaults 40×120 | refuses, with a reason, for a session with no terminal |
| `term.input` / `term.resize` | assume a PTY | refuse for an agent tab with no terminal |
| `spawn_tab_pump` | pumps bytes into `TermScreen` | one pump per capability — **shipped**: `spawn_tab_pumps` starts the byte pump for a terminal and the activity pump for a session that reports itself |
| `mark_idle_tasks` (`app.rs:5860`) | demotes on `quiet_for` + `last_delivered_at` | **shipped**: `status()` not `Working` is the first conjunct, a no-op for the PTY |
| `agent_digest` (`app.rs:7857`) | `"working": bool` | add `"has_terminal": bool`; keep `working` — **shipped**, and asked of the provider before a session exists |
| `catch_up_markdown` (`thread.rs:2270`) | last 40 items by recency, events included | messages only, the limit counting messages (§6.1) — **shipped** |
| `Thread::items` (`thread.rs:1191`) | resident tail of 200 items, older items paged from SQLite | unchanged — activity rows ride the same tail and pages |

### SPA

| Site | Today | Change |
|---|---|---|
| `agentRail.js:177` | Chat / TUI switch | **TUI button shown only when `has_terminal`** — shipped |
| thread rendering (`core/thread.js`) | messages + lifecycle events | renders the four activity kinds; folded by default — shipped |
| `surfaceTabs.js` | mounts the agent's PTY pane | unchanged — it is simply not reached for a no-terminal agent |
| `terminal/manager.js` | one shared socket, demuxed by `term_id` | unchanged |
| `console.js` | the human's own shells | unchanged; the console was never the agent's |

Two of these deserve calling out as **explicitly unchanged**, because the first
draft of this spec had them changing:

- **`surfaceTabs.js` needs no work.** It mounts a PTY pane. A no-terminal agent
  never asks it to, so there is nothing to make conditional — the rail just does
  not offer the button.
- **The console is not the agent's.** It hosts the human's own shells in the
  checkout. A worktree still has terminals even when its agent does not.

---

## 8. Wire contract

Additive. No field changes meaning, and there is **no new RPC** — the
conversation's existing `thread.revision` carries activity like everything else
on a thread.

```json
// agent digest — two new fields, one per step
{ "id": "agent-…", "working": true, "has_terminal": true,
  "can_interrupt": false, … }
```

```json
// thread.post — one new flag (step 8), absent on every post that is not one
{ "entity_id": "run-…", "agent_id": "agent-…", "body": "stop, do X instead",
  "interrupt": true }
```

The two digest booleans read their absence differently, and deliberately.
`has_terminal` absent is `true`: every agent had a basement before the question
could be asked, so silence is not a refusal. `can_interrupt` absent is `false`:
a bridge that does not send the field cannot stop a turn, and a client that
guessed `true` would offer a control whose flag the bridge drops — the human
would be told the turn was stopped while it ran on. Absence means "no" for a
capability that is new, and "yes" for one that predates the question.

```json
// thread items — four new event kinds, in the envelope every event already has
{ "type": "event",
  "data": { "id": "event-41", "sequence": 41, "event": "tool_use",
            "created_at": "…", "summary": "Read bridge/src/app.rs" } }
```

```json
// an outcome — an ordinary agent message with two more fields, both absent
// on every message that is not one (step 7)
{ "type": "message",
  "data": { "id": "message-42", "sequence": 42, "role": "agent",
            "body": "Needs production credentials", "created_at": "…",
            "outcome": "blocked" } }
```

`outcome` is `completed` | `blocked` | `failed`; a completed outcome also sets
the existing `done`, whose meaning does not move, and the `completion_report`
envelope the `Done` event carried rides the message when the agent wrote one.
No field changes meaning and nothing is removed, so a client that knows only
`done` renders a completion exactly as it did.

The class is **not** a field. It is intrinsic to the kind and asked for through
`ThreadEventKind::class` / `ThreadItem::attention_reason`, which is how the
existing 32 kinds work — serializing it would be a second copy of an answer the
kind already gives, free to drift from it. What a client reads off the wire is
the kind; what the kind means for attention is decided in one place.

`agent.attach` gains a typed refusal for a session with no terminal, so an old
client asking gets a sentence rather than a hang. This follows the precedent set
by `require_shell_kind` (`app.rs:97`): refuse loudly and say where the thing
actually lives, never fall back to something different.

---

## 9. Reconnect

Solved by the decision in §5, rather than designed here.

| Surface | Snapshot | Cursor |
|---|---|---|
| Terminal | vt100 screen + byte total | `term.ack` flow control |
| Conversation (recent activity included) | thread revision | item sequence |

There is no third protocol. Activity reconnects the way the conversation does
because it *is* the conversation — no new cursor, no new snapshot, no new flow
control, and no live-only window a reconnecting client can fall out of.

Paging **older** activity out of the store is a separate, ordinary read — the
backward paging the store migration already ships (`wire_value_page`,
`has_more`, `oldest_sequence`) — a client asking for history it has scrolled
back to, not a client resyncing. It has no bearing on reconnect, which only
ever needs the resident tail the thread already carries.

---

## 10. Migration order

Each step compiles, ships and is green on its own.

> **Where this stands, 2026-08-23.** Step 0 — the `Harness` trait, one
> implementation per provider, reached only through `harness_for` — shipped in
> `a26acd2` and is documented in `Harness Refactor.md`. That is the launch side.
>
> Step 1 shipped in `f4e7958`: `AgentSession`, `TerminalView`, `AgentStatus` and
> `Turn` live in `bridge/src/harness/session.rs`, and `PtySession` implements
> both traits with `terminal()` returning `Some(self)`. Two details worth
> knowing before step 2. `AGENT_WORKING_WINDOW` moved from `app.rs` into
> `pty.rs`, because the 30 s paint rule is the terminal's guess and belongs
> beside the implementation that has no better answer — `agent_is_working` still
> reads `idle_for` directly, and swapping it for `status()` is step 2 with the
> constant already in place. And the PTY's `send_turn` is `write_prompt` alone:
> the readiness check the §3 table names stays at the spawn site, since
> re-running it per turn would park a caller holding the app-wide state lock
> behind a mid-turn repaint.
>
> Step 2 shipped in `bf6c730`: `agent_is_working` matches on
> `AgentStatus::Working` and no longer reads `has_exited` or `idle_for`. The
> detail that made it landable alone is that `HarnessSession` now **requires**
> `AgentSession`, so a `Box<dyn HarnessSession>` answers the daemon's question
> while the byte-stream calls around it are still reachable — the daemon
> migrates off the wider trait one call at a time instead of in one change, and
> `Tab.session` keeps its type until step 3 needs it. `PtySession` reports
> `Working` rather than `Starting` for a fresh spawn, because a spawning agent
> is stamped as having just painted and today's rule counts that as working;
> naming the gap is a session protocol's job, not a terminal's, so `Starting`
> stays unreported here and the wire is unmoved.
>
> Step 3 shipped in `a96acba`: `Tab.screen` is an `Option<TermScreen>`, the
> agent digest carries `has_terminal`, and `agent.attach` / `term.attach` /
> `term.input` / `term.resize` / `term.ack` refuse a session with no terminal.
> Four details worth knowing. The refusal is asked through
> `AgentSession::terminal()` and the call is then made through the
> `TerminalView` it hands back, so the capability check and the write are one
> question rather than a flag standing beside a call that could disagree with
> it; `Tab::require_terminal_and_screen` returns the pair for the same reason,
> since a terminal and its grid are made together in `Tab::spawn` and a session
> without one has neither. Two of the five verbs are not in the §7 table:
> `term.attach` reaches an agent tab by wire id and ends in the same
> `attach_to_tab` body, so leaving it out would have left open the hole the
> refusal exists to close, and `term.ack` refuses because a client that was
> never allowed to attach has nothing to acknowledge. `has_terminal` is asked of
> the live session — the only thing that can answer it — so an agent with **no**
> session answers `true`: the attach opens a blank screen for one that has not
> started, exactly as it always has, and the rail must keep offering it. And
> `Tab.session` is untouched, contrary to the note above: what step 3 needed was
> the capability, not the narrower trait bound, so the daemon still holds a
> `Box<dyn HarnessSession>` and §7's first row waits for the step that removes
> the last byte-stream call. **Steps 4–6 have not been started**: no session
> returns `None` from `terminal()`, so every refusal above is dead in
> production and walked only by tests that build the terminal-free session by
> hand.
>
> Step 4 shipped in `bafdfa6`, and its dependency on the store was already
> discharged — the store migration landed, so the four kinds are ordinary
> thread rows and the jsonl log in §6.2 was not built. Three details worth
> knowing. The class needed no new mechanism and no new field: `class()`
> answers `Status` for all four, so `attention_reason` returns `None`, the
> unread count is unmoved, and `run_outcome_mirrors_to_issue` — which is
> `class() == Attention` and is tested over `ThreadEventKind::ALL` — kept the
> four off the Issue conversation without being touched. The catch-up packet's
> `limit` now counts **messages** rather than items: filtering after taking the
> last 40 items would have left a session that emitted forty tool calls with an
> empty packet, which is the failure §6.1 exists to prevent, so the filter runs
> before the take. And nothing emits the kinds — no call site pushes one, and
> the only exercise they get is the tests that push them by hand.
>
> Step 5 shipped in `7240262` and `a3f2d42`, in the SPA. Three details worth
> knowing. `has_terminal` absent is `true`, so an older bridge's digest behaves
> exactly as it always has and only an explicit `false` takes the terminal
> away; the rail overrides the *shown* face rather than the remembered one, so
> opening a terminal-less agent's bubble lands on the conversation while the
> sibling that does have a screen is still where the human left it. The four
> kinds fold as a shut `<details>` carrying the summary's first line in its
> head — a stack of rows all reading "Agent called a tool" is a stack nobody
> can scan — and an activity event with neither a summary nor links is not a
> fold at all. And one thing outside the two the step names had to give way:
> `domPatch` strips any attribute the render does not carry, and a fold is
> always rendered shut, so `open` is on the live element only because the
> reader put it there. Without an exception for it the 1.6 s poll would shut
> every fold the reader opened.
>
> Verifying step 5 turned up three failures that were not step 5's, one per
> suite plus one, and all three were tests reading a clock instead of a
> behaviour: they waited a fixed stretch of wall clock and then asserted on
> whatever had happened last, which on a machine running the whole suite is not
> the thing they meant. `826def1` and `58a4d2e` fix them — the bridge's
> screen-drain helper now waits for the PTY reader's own end of stream rather
> than treating a reaped child as proof the reader is finished, the issue view's
> post-switch read is named rather than taken as the newest, and the socket
> backoff test moved to fake timers. No assertion was weakened and nothing under
> test moved.
>
> Step 5a shipped in `af19486`, `aff5ea6` and `ad9755c`. After it, nothing
> above `pty.rs` names a terminal call, `Tab.session` is a
> `Box<dyn AgentSession>`, and adding a carrier means implementing one trait.
> Five details worth knowing.
>
> `HarnessSession` is **deleted**, not shrunk. What was left of it once the
> table was discharged — the framed paste, the paint-settled readiness wait,
> the paint clock, the reap — is inherent on `PtySession`: a trait with one
> implementation is a name standing in front of a body, and the daemon can no
> longer reach any of it. The clearest evidence the step did what it set out
> to do is a test double: `DictatedSession` used to need an
> `impl HarnessSession` — twelve terminal answers it has none of — merely to
> sit in a `Tab`, and now implements the six calls it actually has answers
> for.
>
> `open_session` returns the session **and its output**, and the subscribe
> happens BEFORE the readiness wait. This is the one thing the table above
> did not anticipate. `Tab::spawn` used to subscribe the instant the PTY was
> spawned and `ensure_agent_tab` waited for readiness afterwards; moving the
> wait into `open_session` without moving the subscribe with it would have put
> a harness's whole startup paint — up to `HARNESS_READY_GRACE` of it — on the
> floor, opening the human's terminal blank and losing the epitaph of a
> harness that died during the wait. The crash test caught it.
>
> Readiness is asked for only when Build will hand the session a turn, so
> `Tab::spawn` passes the grace for an agent and `None` for a shell. An
> unconditional wait would have been a serious regression rather than a
> refactor: a login shell need never announce a line editor, and
> `term.create` holds the app-wide state lock across the open — so every
> `term.create` would have stalled every project for twenty seconds.
>
> Nine `has_exited` sites became one question. Six of them read
> `tab.live && !tab.session.has_exited()`, so the enum arrived as
> `Tab::session_is_live`: the tab's own retention rule and the session's
> report, asked together in the one place that knows both. `HarnessExit`
> takes its code from inside `Ended` and its epitaph from `tab.screen` first
> and `session.epitaph()` second — and the PTY's `epitaph` is `None`, so the
> sweep reads exactly the screen it always did.
>
> And `has_terminal` for a session-less agent comes from `Harness`, which
> defaults `true` — so the digest is byte-identical for every provider that
> exists. `open_session` does not branch on it yet, because there is still
> only one carrier to choose; that arm is step 6's, and the authority it will
> ask is already in place.
>
> **Corrected after review.** The `send_turn` contract in §3 was written the
> wrong way round — it said callers must not hold the app-wide state lock and
> that the daemon already honoured it, while `deliver` held that lock across
> both the write and the 250 ms `exited_within` grace. A provider implementer
> reading it would have felt free to block in `send_turn` and stalled every
> RPC and pump in the daemon. Both sides are now true: `Tab.session` is an
> `Arc<dyn AgentSession>`, so `deliver` takes the handle out of the registry
> and hands the turn over with the lock released, and the contract reads as
> what it has to be — **`send_turn` returns promptly** — because
> `nudge_live_agent_tab` still speaks to a live tab from under the lock, by
> construction: it reads its caller's own tab registry.

> **Step 6, the session half, shipped in `1fbb76d`** (with the `activity()`
> capability in `3d36b71`). `bridge/src/harness/adk.rs` holds `AdkSession`: a
> child running `claude -p --input-format stream-json --output-format
> stream-json` over piped stdio, read line by line, answering every trait call
> from a value it was told. Nothing chooses it yet — `AdkHarness`,
> `AgentProvider`'s arm and `open_session`'s carrier choice are the provider
> half — so the module carries an `allow(dead_code)` that comes off with them.
>
> **The protocol was verified against the installed CLI before a line was
> written: claude 2.1.231, and nothing the step assumed is missing.** `-p`,
> `--input-format stream-json`, `--output-format stream-json`, `--verbose`,
> `--resume`, `--mcp-config` and `--strict-mcp-config` are all there, so the
> argv the provider half will build is exactly the one step 6's detail below
> specifies. Three
> flags worth knowing about that the spec had not named:
> `--include-partial-messages` (not used — Build wants whole events, not token
> deltas), `--forward-subagent-text` (not used, and its default is what makes
> the subagent fold below cheap), and `--fork-session`, which a resume that
> must not overwrite its predecessor's transcript may want later.
>
> Four things the implementation decided that the step did not say.
>
> **An open turn is a flag, not a count.** The obvious reading of "`Working`
> from an accepted turn until its `result`" is to count turns against results,
> and it wedges: a message written while the child is mid-turn is absorbed into
> the running turn, which still ends in ONE result, so a counting session would
> report `Working` for the rest of its life. Since `Working` short-circuits the
> idle sweep (§11 q4), that is not a cosmetic error — it is an agent that
> quietly stopped and is never explained. The test that holds the line hands
> over two turns, gets one result back, and requires the session to be waiting
> after it.
>
> **A successful result clears the reported error.** `epitaph` is the last
> error the session was *told*, and an epitaph explains how a session ENDED —
> so a turn that failed and a later turn that succeeded leave nothing worth
> repeating, and the stderr fallback carries a child that died before it could
> report anything at all.
>
> **The voice decides what a content block can be.** Text and thinking are
> minted only off an `assistant` message: a `user` message carrying text is
> Build's own turn echoed back, and minting it would put the human's words in
> the timeline a second time as narration. `tool_result` is read only off a
> `user` message, which is where the protocol puts it.
>
> **A tool result is named by the call it answers.** The protocol pairs them by
> id and nothing else, so the reader remembers each call until its answer
> arrives — which is also how the exclusion holds for both halves: a
> `mcp__build__*` call is recorded as Build's own and neither it nor its answer
> is minted. Tool inputs and results are clipped to one line (240 chars); what
> the agent itself said — reasoning, narration — is carried whole, because the
> conversation carries what an agent says whole.
>
> Tested entirely against a fake stream-json harness (a `sh -c` script
> replaying recorded protocol lines and reading stdin for turns), never a real
> model turn: `init` → `Waiting` with the session id recorded for `--resume`,
> the turn's status flips including a silent mid-turn stretch, activity minted
> in order, the MCP and subagent exclusions, the epitaph from a reported error
> and from a child that died mid-turn on stderr, `exited_within`'s reap lag,
> `end`'s reap, the stream closing when the child's stdout does, and
> `send_turn` returning on the write to a child that never answers.

> **Step 6's provider half shipped in `06bb399`, `357225c`, `a1bc0d3` and
> `5f9c06d`.** `AgentProvider::ClaudeAdk` is a provider like any other —
> `harness_for` has its arm, `provider_catalogs` offers it, and the persisted
> `ModelChoice.provider` is the whole launch config — and it is the first one
> that answers `has_terminal` false. From there the answer flows on its own:
> the digest reads it before a session exists and off the session after, the
> rail never offers the basement, and `open_session` opens `Carrier::Protocol`
> instead of a PTY. The step-3 refusals are live in production for the first
> time, and `the_terminal_verbs_refuse_the_headless_agent_the_daemon_spawned`
> walks all five against a child the daemon spawned rather than a session built
> by hand.
>
> Six things worth knowing.
>
> **The carrier is a parameter, not a flag.** `open_session` takes a `Carrier`
> — `Terminal { size, turn_ready_grace }` or `Protocol` — so each arm
> carries only what its own carrier has an answer for: a grid and a readiness
> wait belong to a terminal, and a session protocol has neither. Above that
> call a session is a session.
>
> **A session offering neither stream is refused rather than opened.** Not
> because Build could not watch it work: the death rites hang off a stream
> CLOSING, so a session with no stream would leave a dead agent's tab reading
> as live until the idle sweep explained the exit as silence.
> `SessionOutput` carries whichever stream the session has, subscribed at the
> open — the protocol arm subscribes inside `AdkSession::spawn` for the reason
> the PTY arm subscribes before its readiness wait: a child starts talking the
> moment it is forked.
>
> **The activity pump is the byte pump's mirror.** It posts the four kinds into
> the conversation the agent speaks in — which is now one rule
> (`edit_agent_conversation`), read by the MCP action path too instead of
> carrying its own copy — and on close performs the two rites that are not the
> terminal's: the tab goes not live and `record_agent_session_end` runs. The
> tab is RETAINED, exactly as an agent tab whose PTY ended is.
>
> **The `Working` short-circuit landed with it, and is a no-op for the PTY by
> construction.** `a_pty_quiet_past_the_threshold_is_never_working` asserts
> that a PTY silent past the threshold can never claim `Working`, so the new
> conjunct cannot spare an agent the sweep used to demote; the mid-turn side is
> a dictated session past the threshold that is not demoted. Two of the sweep's
> existing tests stopped sleeping out a wall clock and age the paint clock
> instead — they were asserting on whatever had happened after 200 ms, which
> on a loaded machine is not the thing they meant.
>
> **Both screens a spawn can be holding are closed when it has no terminal**,
> not just the one §10 named. The screen clients wait on before a worktree has
> an agent is the case the review flagged; the grid the session being replaced
> retained is the same failure one respawn later, and a human who changes an
> agent's provider between two sessions would otherwise be left watching the
> dead one's last frame. Both hear `term.closed` with reason `no_terminal`.
>
> **The fake stream-json harness moved out of the session's test module** into
> `adk::fake`, beside the reader it exercises, because two suites need the same
> child: the session's tests, which read one session's protocol, and the
> daemon's, which drive a headless agent through the whole spawn path — a
> message posted on a run, a real child, the four activity kinds landing in the
> thread in order, and the child leaving. No test runs a model-backed turn.
>
> **The one bullet of step 6 that did not ship: resume by recorded session id.**
> `AdkSession::session_id` captures the id from `system/init`, but the daemon
> still resumes the way it always has — the transcript probe answers and the
> argv carries `--continue`, which for a Build-owned worktree picks up the same
> conversation. Persisting the id beside the agent needs an `AgentSession` call
> to read it, a persisted field on the agent record and a capture point after
> the child announces itself; it is the sharper resume this makes possible
> rather than something the carrier needs, and the fallback this spec already
> keeps is what runs until it lands. It is now **step 8's second half**,
> specified there down to the record field, the capture point and the crash
> window.
>
> **Live-verified against claude 2.1.236 (2026-08-28)** — the first real
> model-backed runs of this carrier, via the ignored test
> `real_adk_session_steers_mid_turn` (`cargo test --lib real_adk -- --ignored
> --nocapture`, one small haiku turn) and hand probes. What the wire actually
> does:
>
> - **Mid-turn steering works.** A user message written to stdin while a turn
>   runs — including during active tool execution — is delivered at the next
>   step boundary and decides the same turn's outcome. Verified through the
>   real carrier end to end.
> - **`init` arrives only after the first stdin message**, not at spawn. The
>   fake emits it at spawn, which is why the daemon must never wait for init
>   before delivering (it does not; the test originally did and deadlocked).
>   A headless agent spawned without a first turn reports `Starting` until one
>   is delivered.
> - **A narrow loss window exists**: a message written within ~100ms of a
>   `tool_use` event was observed silently dropped once; the same message
>   seconds later is reliable. Human follow-ups are seconds-scale. The init
>   `capabilities` array advertises `msg_lifecycle_v1` (receipts), the future
>   hardening if confirmation is ever needed.
> - **The native interrupt landed upstream.** `capabilities` advertises
>   `interrupt_receipt_v1` / `interrupt_cancel_queued_v1`, and a raw
>   `{"type":"control_request","request_id":…,"request":{"subtype":"interrupt"}}`
>   is acknowledged with a `control_response`, ends the in-flight turn with
>   `result: error_during_execution`, and the next queued user message runs in
>   the same session. Stop-and-redirect is a wire message, not a kill — the
>   kill-and-resteer design is the fallback for a CLI that does not advertise
>   the capability, not the mechanism.
>
> **Both of the above are step 8**, designed from these findings: the interrupt
> as a capability-gated `AgentSession` call with a flag on `thread.post` above
> it, and the recorded session id persisted for `--resume`. Step 8 also retires
> the kill-and-resteer fallback named in the last bullet — the probes showed a
> plain queued message reaches the running turn anyway, so a carrier that
> cannot interrupt degrades to an ordinary send rather than to a kill.
>
> **The SPA's start cards offer the headless carrier (2026-08-24).** The idle
> agent panel's picker draws from a list the UI ships (`STARTABLE_PROVIDERS`,
> `spa/src/core/modelPicker.js`) rather than the catalog RPC, so the new
> provider had to be added there: `claude_adk`, under the bridge's own label
> "Claude Code (headless)". The full PTY harness stays the default start, and
> `providerLabel` now names a headless agent's bubble properly instead of
> echoing the wire token.

1. ~~**Introduce `AgentSession` + `TerminalView`**; `PtySession` implements
   both, `terminal()` returns `Some(self)`. Nothing is optional yet. No
   behaviour change.~~ **Shipped** — see the note above.
2. ~~**Move status behind `status()`.** `agent_is_working` reads the enum; the
   PTY implementation synthesizes it from `idle_for`. The wire is unchanged.~~
   **Shipped** — see the note above.
3. ~~**Make `Tab.screen` an `Option`**, add `has_terminal` to the agent digest,
   and add the typed refusals to `agent_attach` / `term.input` /
   `term.resize`. No session returns `None` yet — the paths are dead but
   exercised by tests.~~ **Shipped** — see the note above; `term.attach` and
   `term.ack` refuse too.
4. ~~**Add the four activity kinds**, classed `Status`, *with* the
   messages-only catch-up packet fix (§6.1) in the same change. Nothing emits
   them yet. Persistence needs no work: an activity item is an ordinary thread
   row, appended as one `INSERT` and paged like every other item — the jsonl
   log §6.2 designed is superseded by the store migration. The catch-up fix
   must not be split out: it exists precisely because the kinds do.~~
   **Shipped** — see the note above.
5. ~~**SPA: hide the TUI button when `has_terminal` is false**, and render the
   four kinds in the thread, folded by default.~~ **Shipped** — see the note
   above.
5a. ~~**Prep: the daemon speaks `AgentSession` only.** `Tab.session` becomes
   `Box<dyn AgentSession>`, every remaining daemon call on the wider trait
   moves behind `AgentSession` / `TerminalView` or into `PtySession`, the
   `HarnessSession: AgentSession` supertrait bridge is dropped, and
   `has_terminal` for a not-yet-started agent comes from the provider. Pure
   refactor: the wire is unchanged and every behaviour byte-identical.~~
   **Shipped** — see the note above; `HarnessSession` is deleted outright.
6. ~~**Then, and only then, add a provider with no terminal** — the ADK, as
   `bridge/src/harness/adk.rs`. By this point it is a new file, not a
   migration. Detail below.~~ **Shipped** — see the note above; resume by
   recorded session id is the one bullet still open.
7. ~~**Outcomes become message statuses** — the §6.1 deferred fix, as designed
   there. Independent of step 6 (it repairs the catch-up packet for every
   carrier) and ordered after it only because step 6 is what makes the gap
   bite daily; it may land first if ADK slips.~~ **Shipped** — see the detail
   below.
8. **The turn can be stopped, and the conversation resumed by name.** The two
   things step 6's live probes made possible: `AgentSession::interrupt`
   (capability-gated, never a kill) with `thread.post`'s `interrupt` flag and
   the composer's split send above it, and `AgentSession::session_id` persisted
   on the agent record so a respawn carries `--resume <id>` instead of
   `--continue` — the one step-6 bullet that did not ship. Detail below. The
   two halves are independent and the resume half may land first, but they
   share one capture point and one test child, so they are one step.
   **The bridge half is shipped** (`66ad2ac`, `defa1be`, `f3ec04b`); §8.4, the
   composer's split send, is what remains.

Steps 1–5a add no providers and change no behaviour. If ADK slips they are
still worth having: step 2 alone removes "quiet for 30 seconds" from being the
only thing Build can say about an agent, step 4's class fix is a latent bug in
the catch-up packet regardless of who fills the thread, and step 5a leaves the
daemon with one vocabulary instead of two.

### Step 5a in detail — the prep refactor

The point of the step: after it, no code above `bridge/src/pty.rs` names
`HarnessSession`, and adding a carrier means implementing `AgentSession` and
nothing else. The daemon's real (non-test) call sites on the wider trait,
inventoried 2026-08-23, and where each one goes:

| Call | Daemon sites | Moves to |
|---|---|---|
| `write_prompt` | `deliver` (`app.rs:17304`), `nudge_live_agent_tab` (`app.rs:16036`) | `send_turn(Turn)` — the value the trait has carried since step 1 |
| `ready_within` | `ensure_agent_tab`, once, after `Tab::spawn` | into the PTY arm of the carrier-choosing spawn (`open_session`, `harness/mod.rs`) — readiness is how a *terminal* opens, still run outside the state lock |
| `idle_for` | `mark_idle_tasks` (`app.rs:5849`), `agent_last_painted_at` (`app.rs:13489`) | `quiet_for()` (§3) — PTY answer unchanged |
| `exited_within` | `deliver` (`app.rs:17310`) | `exited_within()` (§3) |
| `has_exited` | 9 sites (`agent_is_live`, `term_input`/`term_resize` liveness, the idle sweep, `agent_digest`, single-agent guards, the nudge) | `matches!(status(), AgentStatus::Ended { .. })` — the enum already says it |
| `exit_code` | idle sweep (`app.rs:5835`) | the code inside `AgentStatus::Ended` |
| `kill_and_reap` | 8 sites (tab close/retire/reap paths, `ensure_agent_tab`'s stale-agent sweep, the shell pump) | `end()` — its doc inherits the reap obligation: killing without releasing the process-table entry leaks a zombie per session |
| `subscribe` | `Tab::spawn` (pump wiring) | `terminal()`-gated: the byte pump is spawned only for a session that offers one, which is already how `spawn_tab_pump` behaves |
| `resize` | `ensure_agent_tab`'s two spawn-time screen carries | through the `TerminalView` handed back by `terminal()` |
| `pid` | none outside tests | stays on `TerminalView`; tests keep it |
| `backdate_last_output` | tests only | `#[cfg(test)]` hook moves with `quiet_for` |

With the table discharged, `Tab.session` becomes `Box<dyn AgentSession>`, the
supertrait bound comes off `HarnessSession`, and what is left of the wider
trait — `write_prompt`, `ready_within`, `idle_for` as paint-clock mechanics —
either becomes inherent on `PtySession` or disappears into its `AgentSession`
impl; whether the trait name survives at all is `pty.rs`'s private business.
`HarnessExit` construction changes shape but not meaning: code from
`Ended { code }`, epitaph from `tab.screen` first and `session.epitaph()`
second (§3).

Two things the step must also settle:

- **`has_terminal` before there is a session.** Step 3 answers the digest's
  `has_terminal` from the live session and defaults a session-less agent to
  `true` — right when every provider has a terminal, wrong the day one does
  not: the rail would offer a TUI button that the spawn then refuses. The
  provider knows before the session exists, so `Harness` (`harness/mod.rs`)
  gains `fn has_terminal(&self) -> bool { true }`, `agent_digest`
  (`app.rs:7897`) asks `harness_for(agent.choice.provider)` when there is no
  tab, and the same answer is what `open_session` branches on to choose the
  carrier. One authority, asked before and after spawn.
- **Refactor guarantees.** The wire is unchanged (the digest emits the same
  fields; `has_terminal` merely gains a truthful pre-spawn source), no
  constant moves value, and the equivalence proofs follow step 2's precedent:
  a test per mapping row above asserting the new call answers exactly what
  the old one did in every state a PTY can be in.

### Step 6 in detail — the ADK provider

The research first, so the step is built on what the protocol actually is.
`claude -p --input-format stream-json --output-format stream-json` runs the
full Claude Code harness headless over newline-delimited JSON on
stdin/stdout: a turn is written as a `{"type": "user", "message": …}` line;
the process answers with `system` events (`init` carries the `session_id`,
model, tool and MCP-server roster; `api_retry` carries error categories),
`assistant` / `user` messages whose content blocks are text, thinking,
`tool_use` and `tool_result` (subagent traffic marked by
`parent_tool_use_id`), and one `result` line per turn — the turn boundary,
carrying the outcome text, cost and `session_id`. The process stays alive for
turn after turn while stdin is open. `--resume <session_id>` reopens a
recorded conversation (from any directory since claude 2.1.223) and
`--continue` the cwd's most recent; headless sessions write the same
`~/.claude/projects/**/*.jsonl` transcripts the interactive TUI does — the
format `harness/claude.rs`'s probe already reads. `--mcp-config` /
`--strict-mcp-config` work as in interactive mode, and since 2.1.221 the
first turn waits for pending MCP servers, so the `done` socket is live before
any turn runs.

What the step builds, all in `bridge/src/harness/adk.rs` plus one enum arm:

- **Provider selection is the launch config that already exists.**
  `AgentProvider` gains a variant (working name `ClaudeAdk`, labelled
  "Claude Code (headless)"), `harness_for` gains its arm, and the model
  picker offers it through the same `models()` / `label()` surface — the
  persisted `ModelChoice.provider` on the entity is the whole launch config,
  no new wire field. `AdkHarness` reuses `ClaudeHarness`'s catalog,
  `model_args`, workspace pre-trust and transcript probe; its `spec` swaps
  the interactive argv for `-p --input-format stream-json --output-format
  stream-json --verbose`, keeps `--mcp-config` + `--strict-mcp-config` +
  `--dangerously-skip-permissions` and the `INHERITED_AGENT_MARKERS`
  clearing, and answers `has_terminal() == false` — which flows through
  `open_session`'s carrier choice to the digest, so the rail never offers the
  TUI and the step-3 refusals go live in production for the first time.

- **`AdkSession` implements `AgentSession`.** It owns the child (piped
  stdio, no PTY) and one reader task over stdout. `send_turn` serializes the
  turn as a user line and returns when the write is accepted — the protocol
  column of §3's table, no readiness dance, no submit delay. `status()` is
  reported, not guessed: `Starting` until `system/init`, `Working` from an
  accepted turn until its `result` line, `Waiting` after, `Ended { code }`
  when the child exits. `quiet_for` is time since the last line read;
  `exited_within` waits on the child; `epitaph` retains the last
  error-bearing `result` (or last stderr line); `end` kills and reaps;
  `terminal()` stays the default `None`.

- **Minting activity into the conversation — the pump seam.** The byte pump
  (`spawn_tab_pump`, `app.rs:17408`) is the precedent: a task spawned beside
  the tab that owns the session's output and, on stream close, marks the tab
  dead and closes the conversation's session lineage
  (`record_agent_session_end`, `app.rs:2916`). Step 6 adds the second pump
  behind a capability that mirrors `terminal()`:
  `AgentSession::activity() -> Option<Receiver<AgentActivity>>`, default
  `None`, `Some` for a session that reports its own events. Where the spawn
  path starts the byte pump for a terminal, it starts the **activity pump**
  for an activity stream: the pump owns the receiver, posts the four §5 kinds
  through the thread under the app lock — thinking summaries as `Reasoning`,
  `tool_use` as `ToolUse` (summary: tool name plus a one-line input),
  `tool_result` as `ToolResult`, assistant text as `Narration` — and on
  close performs exactly the byte pump's death rites. Two deliberate
  exclusions: `tool_use` of Build's own MCP tools is not minted (the socket
  already carries `post_thread_message` and `done` as their real selves —
  minting the call too would tell the timeline everything twice), and
  subagent-attributed events (`parent_tool_use_id` set) are folded into the
  spawning tool call rather than minted individually, at least at first —
  both are additive to revisit.

- **Three requirements the step must hold, flagged in review 2026-08-23.**
  Each is an edge a PTY holds up by accident and a no-terminal carrier falls
  through. They are normative, not advisory.

  - **A session with no terminal MUST report activity.** `terminal()` and
    `activity()` may not both be `None`, and `open_session` refuses a carrier
    that answers `None` to both rather than spawning it. The death rites hang
    off the activity pump exactly the way they hang off the byte pump: the
    byte pump performs them when the broadcast closes (`app.rs:17458`) —
    marks the tab not live, pushes `term.closed` through the screen, calls
    `record_agent_session_end`. A session with neither stream has no close to
    hang them on, so its tab would keep reading as live and its
    conversation's session lineage would stay open until the idle sweep
    noticed minutes later and explained an exit as silence. The activity
    pump owes the two rites that are not the terminal's: the tab goes not
    live and `record_agent_session_end` runs. The `term.closed` push is the
    screen's half and there is no screen — the refusals of step 3 already
    kept every client off it.

  - **§11 q4's `Working` short-circuit lands in this step.**
    `mark_idle_tasks` (`app.rs:5812`) today demotes on `quiet_for` and
    `last_delivered_at` alone. Step 5a deliberately did not add the `status()`
    test, because it is behaviour design for a turn-boundary carrier rather
    than part of a refactor: for a PTY it is a no-op, since paint inside 30 s
    is what makes `Working` and a tab quiet past the 300 s threshold cannot
    be `Working`. It stops being a no-op the day a carrier reports its own
    turn boundaries — a model reasoning for forty minutes is `Working` and
    silent, and without the short-circuit the sweep demotes it mid-turn. So
    the demotion condition becomes `status()` is not `Working` **and**
    `quiet_for() >= threshold` **and** nothing delivered within the
    threshold. It must be tested from both sides: byte-identical for the PTY
    (the existing sweep tests stand unmodified, plus one asserting a PTY past
    the threshold is never `Working`, so the new conjunct can never spare
    one), and a fake turn-boundary session mid-turn past the threshold is not
    demoted.

  - **A no-terminal spawn MUST close the screens waiting on it.** Clients
    that mount an Agent tab before its worktree has an agent are held on a
    screen with no PTY (`agent_screens_awaiting_spawn`, `app.rs:1909`), and
    the spawn carries them onto the real screen under the same lock
    acquisition that publishes the tab (`app.rs:17228`). A session with no
    terminal has no real screen to carry them to, so that carry silently
    drops them and they sit attached to a grid nothing will ever paint. The
    spawn instead pushes `term.closed` through the waiting screen — the way
    `retire_agent` (`app.rs:7810`) and the orphan reaper (`app.rs:5783`)
    already end one — so the client is told, and the rail, which by then
    reads `has_terminal: false` off the digest, stops offering the TUI.
    Refusing the attach outright is the same answer said earlier; what is
    forbidden is leaving the client attached to nothing.

- **The MCP socket wiring is identical, by construction.** The argv carries
  the same per-agent `--mcp-config` the orchestrator scaffolds today, the
  env carries the same `BRIDGE_MCP_SOCKET` / `BRIDGE_MCP_TOKEN`, and `done` /
  `post_thread_message` / `read_unread_messages` / `search_conversation`
  arrive over the same unix socket. §2's dividend cashes out: the from-agent
  half of the interface needs zero work.

- **The worktree is unconditional** — see §11 q3: a no-terminal agent still
  gets one, because the worktree is the workspace and the PTY was only ever
  the way one kind of agent sat in it. Everything that runs before the
  carrier is chosen — adoption, the primary checkout,
  `scaffold_agent_worktree`, the transcript probe, the session-token mint —
  is path-and-provider work that holds unchanged for a headless child with
  the same cwd, so step 6 adds nothing there.

- **Resume.** `has_transcript` already answers for headless sessions (same
  transcript directory). Sharper than the cwd heuristic: `AdkSession`
  captures the `session_id` from `system/init`, the daemon persists it beside
  the agent, and a respawn passes `--resume <id>`; when no id was recorded —
  a Build-adopted worktree whose transcript the human left behind — the
  existing `continue_session` probe falls back to `--continue`, unchanged.

- **The idle sweep meets a session that cannot lie about working** — see
  §11 q4: a `Working` status is never demoted, and the quiet clock reads
  protocol events instead of paint. `done` remains the only completion
  signal; a `result` line is a turn boundary, never a report.

### Step 7 in detail

Specified in §6.1 ("The fix, designed"). Summary of the moving parts: the
outcome message with its additive `outcome` field, `post_completion` retired,
the `Done`/`Blocked` emission dropped (kinds retained for old rows), the
catch-up filter inverted for outcome messages, the Issue mirror re-keyed, and
the compatibility story for persisted threads and older clients — all there.

> **Shipped in `4f2c4d0` and `02dd6df`.** `Thread::post_outcome` is the one
> place an outcome is written down, `record_report_in_thread` is its only
> caller, and `post_completion` is gone. Five details worth knowing.
>
> **The outcome names itself with the token of the event it replaced.**
> `MessageOutcome::{Completed, Blocked, Failed}` answers `done` / `blocked` /
> `run_failed` when the unread rule asks a message why it needs the human, so
> the inbox line, the push kind and `unread_reason` on the wire are unmoved —
> the equivalence §6.1 asks for is held by the reason token rather than by a
> second copy of the rule. `done` is the completed arm and nothing else sets
> it: the parameter every other post threaded through is deleted, so the flag
> cannot come apart from the outcome it stands for.
>
> **Only a REPORTED outcome stops being an event.** The other three arms of
> `record_report_in_thread` are Build's own reading rather than the agent's
> report, and they have no message to attach to: a triage pass (`Triaged` —
> nothing waits on it), a validation Build judged (`ReviewBlocked`, which is
> also outside the three-value outcome vocabulary and would have had to change
> what its row means to fit), and every `RunFailed` raised where no report was
> made. A report Build could not APPLY is still the agent's report, so it is a
> `Failed` outcome carrying Build's note in the same body — which is what keeps
> `run_failed` on that row.
>
> **The Issue mirror needed no re-keying, and the tests say why.** A planned
> implementation's report is written straight onto the Issue's conversation by
> `record_report_in_thread`'s caller — `run_outcome_mirrors_to_issue` never
> carried it — so the outcome reaches the same timeline as the same one unread
> entry, now with the packet carrying it too. The helper still keys on event
> class for the one thing that does travel through it (an abandoned branch),
> and its doc says so rather than leaving the next reader to work it out.
>
> **The report is boxed on the message.** Four vectors on the rarest field of
> the largest thread item made `ThreadItem::Message` 600 bytes against the
> event's 328, which is a clippy error before it is a design question, and the
> answer is the same either way: ordinary messages should not carry a
> completion report's bulk through every conversation the daemon holds.
>
> **Nothing changed in the SPA, because this step names nothing there.** The
> compatibility bullet in §6.1 is exactly what the shipped client now is: a
> completion still renders as an agent message, and a blocked outcome renders
> as one stating the reason. Two things it will want, when a step asks for
> them: the completion-report card is drawn from `event.completion_report`
> only, so a new completion's report has no card until the message half is
> rendered, and the "Agent reported done" event row no longer appears on new
> work.
>
> **Both are closed now (SPA, 2026-08-24).** `messageHtml`
> (`spa/src/core/thread.js`) draws a marker from `message.outcome` in the
> vocabulary of the event each outcome replaced — the same icon, words and tone
> for `completed` / `blocked` / `failed` — and the same `completionReportHtml`
> card from `message.completion_report`. Nothing keys on `done`, so a
> pre-step-7 thread still renders its `Done` / `Blocked` rows and their
> event-attached cards exactly as before.

### Step 8 in detail — stopping a turn, and resuming by name

Grounded in the step-6 probes above, not in a guess about the protocol: the
native interrupt exists and is advertised, a mid-turn message is delivered at
the next step boundary, `init` carries the session id and arrives only after
the first stdin message. Everything below follows from those four facts.

> **The bridge half shipped in `66ad2ac`, `defa1be` and `f3ec04b`** — §8.1,
> §8.2, §8.3 and §8.5, as designed, with tests 1–10 of the list below. Three
> things worth knowing, none of which changed a decision.
>
> **The `--resume` fallback is not one path but two.** §8.2's crash window says
> a missing id falls back to `--continue`; the shipped `AdkHarness::spec` says
> the same thing as one `match` over `resume_session_id`, so "never both" is a
> shape rather than a rule anyone has to remember. The daemon still fills
> `continue_session` unconditionally from the transcript probe, exactly as it
> did — the name simply wins where there is one.
>
> **`agent_harness_spec` grew a parameter rather than a struct.** Its callers
> are the one spawn reservation and five tests, and the alternative — folding
> `continue_session` and `resume_session_id` into a resume enum — would have
> touched `ClaudeHarness` and `CodexHarness`, which §8.2 says are untouched.
>
> **The daemon test reads the child's stdin.** What Build SAID is otherwise
> invisible from outside the session: with an ordinary send and an interrupted
> send both ending in a delivered turn, a test watching only what came back
> would pass with the flag ignored. So `adk::fake` grew a stdin recorder, and
> test 7 asserts the sequence `user`, `control_request`, `user` — the order
> §8.3 exists to hold, read off the wire.

#### 8.1 The carrier's interrupt

`AdkSession` gains the two calls §3 adds, both answered from the protocol.

- **The capability comes from `init`, not from the provider.** `ProtocolState`
  records the `capabilities` array the child announced, and `can_interrupt()`
  is `capabilities` containing `interrupt_receipt_v1`. A session that has not
  announced yet answers `false` — it has no turn to stop either — and a CLI
  built before the feature landed answers `false` forever, which is the whole
  point of asking the child rather than the version number.

- **`interrupt()` writes one line and returns.**
  `{"type":"control_request","request_id":"<uuid>","request":{"subtype":"interrupt"}}`,
  with a fresh id per request (`uuid::Uuid::new_v4`, the mint the daemon already
  uses for MCP session tokens). No wait for the ack: the contract is
  `send_turn`'s, for `send_turn`'s reason — the in-place nudge speaks from under
  the app-wide state lock.

- **What the reader does with the ack.** `ProtocolState` holds at most one
  outstanding interrupt:

  ```rust
  /// The interrupt Build asked for, until the result that closes the turn it
  /// ended arrives.
  struct PendingInterrupt {
      request_id: String,
      /// The child answered `control_response` for this id.
      acked: bool,
      /// A turn was handed over behind the interrupt — the steering turn,
      /// which the child runs once the interrupted one is closed.
      steered: bool,
  }
  ```

  A `control_response` whose `request_id` matches sets `acked`; one that does
  not is noise and is ignored. A second `interrupt()` while one is outstanding
  replaces it: asking twice to stop the same turn is one ask. The pending
  interrupt is dropped when the turn's result arrives, acked or not, so it can
  never leak into the turn after it.

- **The interrupted result must read as interrupted.** This is the rule the
  whole step exists to hold. `read_result` today records a failing result's
  text as `reported_error`, which `epitaph()` hands to `HarnessExit` — so an
  interrupt that changed nothing else would end the human's own stop with a
  crash notice quoting `error_during_execution`. When an interrupt is
  outstanding, `read_result` instead takes the pending interrupt and:
  - clears `reported_error` when the interrupt was **acked** — a turn the human
    stopped leaves no epitaph, whatever subtype the result carried. The ack is
    what makes that true rather than a guess: the child answers the
    `control_response` before it emits the result (probe, 2.1.236), so an
    interrupt still unacked at the result is one the child never acted on, and
    the failure the result reports is the turn's own and keeps its epitaph;
  - sets `turn_open = steered` rather than `false`.

  The second half is subtler than it looks and is why the flag is not enough on
  its own. The sequence is: turn A open, `interrupt()`, `send_turn(B)` (which
  sets `turn_open`, already true), then the child emits the ack, then A's
  `error_during_execution` result, then it starts running B. Clearing
  `turn_open` on A's result would leave a session that is actively working
  reporting `Waiting` — and B is exactly the kind of turn that then goes silent
  for minutes inside one tool call, so the idle sweep would demote a steered
  agent to `IdleUnreported` mid-work. There is no second result to reopen the
  flag, because B ends in its own single result. So the result that closes an
  interrupted turn hands the flag on to the turn queued behind it.

- **`error_during_execution` is not a failure anywhere else**, and nothing has
  to be taught that. A `result` line is a turn boundary and never a report
  (§11 q4): the only path by which its error text reaches a human is the
  epitaph, and the epitaph is what the interrupt clears. No `RunFailed`, no
  `Blocked`, no `Interrupted` event, no `failed` outcome — outcomes are minted
  by `record_report_in_thread` from an agent's `done` call and by nothing else
  (step 7). `Interrupted` in particular stays what it means: a session that is
  GONE. An interrupted session is the same session, holding the same
  conversation, and it is about to answer.

- **What `interrupt()` does when the capability is absent: it refuses, and it
  never kills.** `HarnessError::Unsupported(String)` is the new variant, and
  the refusal carries the sentence that says where the thing actually lives —
  the PTY's names the terminal and Esc, the headless one says its CLI does not
  advertise an interrupt. The kill-and-resteer fallback named in step 6 —
  `end()`, respawn with `--resume <id>`, steering turn first — is **not built**,
  for reasons that got stronger as the probes came in:
  - A kill throws away everything the turn had done, including a tool call
    halfway through a write, and costs a full startup plus a catch-up packet to
    get back to a worse position than the one it left.
  - `--resume` needs the persisted id, which §8.2's crash window says may not
    exist — so the fallback would itself need a fallback, and the one it would
    fall back to is a fresh agent that has lost the turn.
  - The probes made it unnecessary. A plain queued message is delivered at the
    next step boundary and DECIDES the same turn's outcome. That is most of
    what the human wanted, at none of the cost — so where Build cannot
    interrupt, the right degradation is the ordinary send, not the kill.
  - A session-level call that ended and respawned the session would be lying
    about its own lifetime: the object the caller holds would be dead on
    return. Respawning is the daemon's business — it owns the tab registry, the
    spawn lock, the MCP token mint and the lineage record — and the human's
    handle on it is `agent.start`, which already stops and starts a session
    deliberately.

#### 8.2 The persisted session id

- **The record.** `Agent` (`bridge/src/agent.rs:55`) gains
  `#[serde(default)] pub resume_session_id: Option<String>`.

- **The store change is no schema change.** The `agents` table
  (`store.rs:407`) keeps the whole `Agent` serde shape in its `record` column,
  so the field rides in that JSON and a row written before this step loads as
  `None`. Nothing like `add_attention_column` is needed — that migration exists
  because `attention` is a COLUMN, hoisted out of the JSON so the database
  could count it. Nothing counts or sorts by a resume id; it is read only when
  its own agent respawns. The write is the roster upsert that already runs
  (`save_*_agents`, `store.rs:880`).

- **The capture point is the activity pump** (`spawn_activity_pump`,
  `app.rs:17689`). It is the only task that wakes on this carrier's own events,
  it already resolves `(owner, agent_id)` and takes the state lock, and it
  already runs the other rites of a session's life. On every wake — each event
  and the close — it reads `session.session_id()` and, when that differs from
  what the record carries, writes it through the agent's own roster (a
  `record_agent_resume_id` beside `record_agent_activity`; NOT
  `edit_agent_conversation`, which edits the conversation an agent SPEAKS in
  and for an implementation's first agent that is the Issue's thread, not the
  agent's record). The compare runs per event; the write runs once per session.

- **The argv.** `SpawnOptions` gains `resume_session_id: Option<String>`,
  filled at the spawn reservation beside `continue_session`
  (`app.rs:17274`) from the agent's record. `AdkHarness::spec` prefers it:
  `--resume <id>` when there is one, `--continue` when there is not and the
  transcript probe said yes, neither otherwise. The two are alternatives and
  never both — `--resume` names the exact conversation and `--continue` guesses
  the newest one in the cwd, so passing both would be asking for two different
  conversations. `ClaudeHarness` and `CodexHarness` are untouched: a PTY session
  answers `None` to `session_id()`, so the field is always empty for them, and
  the option is carrier-neutral in shape only.

- **The crash window: `--continue` stands, unchanged.** An id is captured only
  after the child announces `init`, and `init` arrives only after the first
  stdin message (probe, 2.1.236). So an agent that spawns and dies before its
  first turn, or whose daemon is killed between `init` and the pump's next
  wake, has no persisted id — and the shipped fallback runs exactly as it does
  today: the transcript probe answers, the argv carries `--continue`, and for a
  Build-owned worktree that picks up the same conversation. `--resume` is a
  sharpening of a path that already works, never a requirement, and no code
  path may treat a missing id as an error.

- **A bad id must not poison every respawn.** A session spawned with
  `--resume <id>` whose id no longer resolves exits without announcing itself.
  So the pump's close arm clears the record's id when the session that just
  ended never announced one of its own: the next spawn falls back to
  `--continue`, and one dead id costs one restart rather than every restart.
  The same clearing covers a child that died at startup for an unrelated reason
  (auth, a missing binary), where clearing is harmless because the fallback is
  what would have run anyway.

#### 8.3 The daemon's steering flow

**The one pipe stays `deliver`.** Interrupt is a flag on a delivery, not a
second way to reach an agent.

- **The wire choice: `thread.post` accepts `interrupt: true`, and there is no
  `agent.interrupt` RPC.** Three reasons, against the existing wire's grain:
  - The precedent is already in `thread_post` (`app.rs:10635`) and written down
    there: a press on the agent's suggested actions comes in through
    `thread.post` "rather than through a verb of its own: it IS a reviewer
    message, so everything that follows one — waking the agent, resuming a
    parked entity, the inbox anchor — has to happen exactly as it does for a
    typed one". An interrupt-and-steer is a reviewer message with one more
    thing to say about how urgently it should land.
  - Build never interrupts without a turn to follow (§8.4: the SPA only offers
    "Interrupt & send"). A verb of its own would therefore always be followed
    by a `thread.post` a moment later, and the gap between the two round trips
    is a window in which the child starts a fresh turn, or the agent calls
    `done` — so the flag on the message is not merely tidier, it is the only
    ordering that cannot come apart.
  - `thread.post` already owns the fan-out an interrupt needs and an RPC would
    have to copy: which agent of the roster is addressed, the Issue-to-live
    implementation redirection, the parked-entity `Reply`, the inbox anchor.

- **Where the flag travels.** `thread_post` parses it once
  (absent is `false`), passes it to `tell_the_agent_a_message_is_waiting`
  (`app.rs:10885`, four call sites, all in `thread_post`), which hands it to
  `nudge_live_agent_tab` (`app.rs:16137`). There, and only there:
  `session.interrupt()` then `session.send_turn(…)`, in that order, from under
  the state lock exactly as the nudge already speaks — both calls return
  promptly by contract.

- **The flag is dropped, without an error, on the cold path.** A message that
  has to SPAWN an agent has no turn to stop, so `PendingAgentTurn` does not
  carry the flag: an interrupt of nothing is not a failure, it is a stronger
  form of what was asked for. The same holds when the tab exists but the
  session is not live.

- **A refused interrupt does not fail the post.** Where `interrupt()` returns
  `Unsupported` — a capability lost between the digest the client read and the
  post it sent — the daemon logs it and delivers the message as an ordinary
  queued turn, which the probes verified is delivered at the next step boundary
  anyway. The alternative is an error the human must read for a difference they
  cannot act on and did not cause.

- **Status and attention move by exactly one step: the human's message.**
  Everything `thread.post` does today happens unchanged (the message is durable
  on the thread, a parked entity takes its `Reply`, the anchor gets its one
  chance to move, `last_delivered_at` restarts the quiescence clock). And
  nothing else is minted: no `Blocked`, no `RunFailed`, no `Interrupted`, no
  outcome, no new event kind, and no state transition of any kind that the same
  message without the flag would not have made. The record of why the turn
  stopped is the human's own message sitting in the timeline after the agent's
  last tool call — which is enough precisely because Build never interrupts
  without one.

#### 8.4 The SPA affordance

Per the standing principle (`user-agency-at-the-trigger`: split-button
dropdowns for multi-behavior verbs), the composer's send becomes a split
control where — and only where — there are two behaviours to choose between.

- **The digest field is `can_interrupt`** (additive; absent is `false`, per §8).
  Answered in `agent_digest` (`app.rs:7964`) from the live session
  (`tab.session.can_interrupt()`) and `false` when there is no session — unlike
  `has_terminal`, the provider cannot answer this one, because the capability is
  announced by the child at `init` rather than decided by the argv.

- **The condition is `working && can_interrupt`.** A reader in
  `agentRailModel.js` beside `agentHasTerminal` (`agentCanInterrupt(agent)`,
  strict `=== true`). Headlessness is not tested separately: only a carrier
  with no terminal can answer `can_interrupt` true today, and if a future one
  could, the control should be offered there too — one condition, one source of
  truth. Not working means no turn to stop, so the plain button stands.

- **The control.** `splitButtonMarkup` (`spa/src/core/splitButton.js`,
  `variant: "primary"`) with `Send` first — so the default press is unchanged,
  queued, and lands at the next step boundary — and `Interrupt & send` as the
  menu alternative, described as "Stop what the agent is doing now and hand it
  this message". The destructive-ish half is never the default press; that is
  the principle, and it is also what the probes support, since the queued send
  usually gets there anyway.

- **The composer is not rebuilt to swap it.** `composerHtml` gains a
  `canInterrupt` option, and the poll moves the control the way it already
  moves the placeholder (`syncComposerPlaceholder`, `agentRail.js`): the send
  control is re-rendered in place only when the condition changes. Rebuilding
  the composer would take the draft and the focus with it, mid-sentence, every
  time an agent started or finished a turn.

- **The send carries the flag.** Choosing the alternative calls `thread.post`
  with `interrupt: true` and the same body and attachments the plain send would
  have carried. One send path, one flag.

#### 8.5 The fake harness learns the control protocol

Everything above is testable without a model turn, which is the rule this
carrier has been built under from the start. `adk::fake` (`adk.rs:735`) grows:

- `INIT` carries the `capabilities` array the live CLI sends
  (`msg_lifecycle_v1`, `interrupt_receipt_v1`, `interrupt_cancel_queued_v1`),
  and a second `INIT_WITHOUT_INTERRUPT` announces none — the child a refusal is
  tested against.
- The replay loop reads each stdin line and branches on it: a
  `control_request` line is answered with a `control_response` **echoing the
  request id back** (so the reader's matching is exercised rather than
  assumed), followed by an `error_during_execution` result closing the
  interrupted turn; then the loop continues, and the next queued user line is
  replayed as an ordinary turn. That is the live sequence, in order.
- The existing single-quote rule still holds for the recorded lines; the
  control branch is the one place the script interpolates, since it has to
  quote a value it read at runtime.

The tests the step is written first as, all against that child:

1. an interrupt is a `control_request` the child acks, and the session's
   pending request is cleared by the matching id;
2. an interrupted turn leaves no epitaph — an `error_during_execution` result
   under an ACKED interrupt reports `None`, while the same result with no
   interrupt outstanding, and the same result under an interrupt the child
   never acked, both still report the error (the equivalence, from all three
   sides);
3. a steering turn written behind an interrupt keeps the session `Working`
   across the interrupted turn's result — the `turn_open = steered` rule, which
   is what keeps the idle sweep off a steered agent;
4. a session whose `init` advertised no interrupt refuses, says where to go
   instead, and is still alive afterwards (the refusal is not a kill);
5. `can_interrupt()` is false exactly when `interrupt()` refuses;
6. the PTY refuses and names the terminal;
7. through the daemon: a `thread.post` with `interrupt: true` on a live
   headless agent stops the turn and delivers the message, and the thread has
   the human's message and NO `Blocked` / `RunFailed` / `Interrupted` event, no
   outcome, and the entity's state unmoved;
8. through the daemon: an interrupt the carrier refuses still delivers the
   message;
9. the session id is persisted once and the next spawn's argv carries
   `--resume sess-adk` and no `--continue`;
10. a session that ends having never announced clears the persisted id;
11. in the SPA: `can_interrupt` absent renders the plain Send; `working` plus
    `can_interrupt` renders the split with Send as the default; choosing the
    alternative posts `interrupt: true`.

---

## 11. Decisions needed before step 1

The first four gated step 1; q5 is a later question kept here because this is
where this document records what was decided and why, and the numbering other
sections cite (§11 q3, §11 q4) must not move.

1. ~~**How large is the thread's recent activity window?**~~ Answered by the
   store migration: a conversation keeps a resident tail of 200 items in memory
   (`RESIDENT_CONVERSATION_TAIL`, `store.rs:477`), first loads and scroll-back
   ship 60 items per page (`DEFAULT_THREAD_PAGE`, `thread.rs:985`), and older
   items are read back from SQLite. Activity rows get the same treatment with
   no new code. Nothing gates step 4.

2. **Does `Turn` carry structure, or stay a string?** A PTY can only take text.
   ADK can take structured content (attachments, images, tool results). Making
   `Turn` a struct now costs little; making it one later touches every caller of
   `deliver`.

3. ~~**Does a no-terminal agent still get a worktree?**~~ **Answered
   2026-08-23: yes, unconditionally.** ADK and the app server operate on
   files; the worktree is the workspace, and the PTY was only ever the way
   one kind of agent sat in it. What matters is that the paths which assume
   "an agent can be dropped into a checkout" assume nothing about the
   carrier, and reading them confirms it:
   - **Adoption** (`run_adopt`, `app.rs:11822` → `Orchestrator::adopt_run`,
     `orchestrator.rs:2366`; `adopt_implementation`, `orchestrator.rs:1476`)
     is git and records: checkpoint commit, `.build` scaffold, roster,
     lifecycle events. No session exists at adoption and none is consulted.
   - **The primary-checkout super-worktree** (`run_adopt` with
     `primary: true`, `describe_primary_checkout` in `worktree.rs:595`, the
     `owns_primary_checkout` / `primary_run_of` guards, `app.rs:6407`) is
     derived ownership over a directory; its lifecycle guards never touch a
     session.
   - **The drop-in itself** (`deliver` → `ensure_agent_tab`,
     `app.rs:17064`) is the first place a carrier exists, chosen at
     `open_session` — everything before it (`scaffold_agent_worktree`, the
     per-provider `has_transcript` probe, the session-token mint) is
     path-and-provider work that holds for a headless child with the same
     cwd.
   The one PTY-flavoured residue found is `agent_last_painted_at`
   (`app.rs:13479`) reading the paint clock for external worktree cards;
   step 5a's `quiet_for` makes that carrier-neutral. Adoption's
   `--continue` pickup holds too: headless claude keeps the same cwd-keyed
   transcripts the probe reads.

4. ~~**What does `done` mean when the harness reports turn boundaries?**~~
   **Answered 2026-08-23: `done` stays the completion contract; turn
   boundaries only sharpen status and idle detection, and never substitute
   for it.** `done` carries the structured report and drives the lifecycle;
   a turn boundary carries neither, so a `result` line is not a completion
   any more than a quiet PTY was — "silence is an anomaly, never completion"
   survives with the anomaly clock reading a better instrument. Precisely,
   for the idle sweep (`mark_idle_tasks`, `app.rs:5812`, threshold
   `BRIDGE_IDLE_SECONDS`, default 300 s; the demotions land through
   `on_plan_idle` / `on_run_idle`, `orchestrator.rs:1074` / `2000`):
   - An exited session is explained as today: code from
     `AgentStatus::Ended`, epitaph per §3.
   - A live session is demoted to `IdleUnreported` iff **`status()` is not
     `Working`** and **`quiet_for() ≥ threshold`** and Build has not
     delivered a turn within the threshold (`last_delivered_at`,
     carrier-independent). For the PTY this is byte-identical to today —
     `quiet_for` is the paint clock and paint-within-30 s is what makes
     `Working`. For a turn-boundary session, "quiet for N minutes" is
     replaced by exactly this pair: `Working` short-circuits the sweep, so
     a model mid-turn for 45 minutes is never demoted; after a `result`
     with no `done`, the session reports `Waiting` and the quiet clock runs
     from the last protocol event, so the same five-minute rule fires from
     the turn's true end rather than from a guess about paint.
   - Everything downstream is untouched: a question posted mid-work still
     never blocks, a demoted entity still resumes on reply, and a late
     `done` from `IdleUnreported` is still honoured.

5. ~~**What does Build do when a human wants a working agent stopped and
   pointed somewhere else?**~~ **Answered 2026-08-28, from the live probes,
   and specified as §10 step 8.** Three decisions, each of which could have
   gone the other way:
   - **An interrupt is a message, not a kill.** `AgentSession::interrupt`
     stops the turn and leaves the session alive, holding the same
     conversation; a carrier that cannot do it REFUSES with a sentence
     (`HarnessError::Unsupported`) and the daemon degrades to an ordinary
     queued send, which the probes showed reaches the running turn at its next
     step boundary anyway. The kill-and-resteer fallback step 6 named is not
     built: it costs the turn's work and a full restart to reach a worse
     position, and it needs a persisted session id that the crash window says
     may not exist.
   - **The PTY does not map it to ESC bytes.** ESC means what the harness on
     the other end decides it means, and a PTY reports no turn boundary — so
     Build would be pressing a key blind and telling the human it worked. The
     human drops into the basement and presses it themselves, which is what
     the refusal's sentence says.
   - **The turn it ends is not a failure.** The interrupted turn's
     `error_during_execution` result clears rather than records the reported
     error, so there is no epitaph, no `RunFailed`, no `Blocked`, no
     `Interrupted` and no failed outcome. What the timeline carries is the
     human's own message, which is always there because Build never
     interrupts without one.

---

## 12. Revision history

- **2026-08-28, step 8's bridge half shipped.** §8.1, §8.2, §8.3 and §8.5 as
  designed, in `66ad2ac` (the trait's two calls and the PTY's refusal),
  `defa1be` (the carrier: capability from `init`, the interrupted result read
  as interrupted, the session id captured and spent as `--resume`) and
  `f3ec04b` (the daemon: `thread.post`'s flag through `nudge_live_agent_tab`,
  and `can_interrupt` on the digest). Tests 1–10 of §8.5's list, all against
  the fake, none running a model turn. §8.4 — the composer's split send — is
  what remains.
- **2026-08-28, step 8 specified — the interrupt and the persisted session
  id.** The two things the live probes of step 6 made possible, designed
  against what the wire was actually observed doing. `AgentSession` gains
  `interrupt` (defaulting to a refusal that says where to go instead),
  `can_interrupt` (the same answer asked without performing it, from one value)
  and `session_id`. The interrupt is capability-gated on the `init` line's
  `capabilities` array, never kills, and its result is read as interrupted
  rather than as a crash — the epitaph is cleared and the open-turn flag is
  handed to the steering turn queued behind it, so a steered agent is still
  `Working` and the idle sweep leaves it alone. The wire stays one pipe:
  `thread.post` gains `interrupt: true` (the precedent is `option_reply` — a
  press on the agent's actions is a reviewer message, not a verb of its own),
  and the flag reaches `nudge_live_agent_tab` as `interrupt()` then
  `send_turn()`. The digest gains `can_interrupt`, whose absence means `false`
  where `has_terminal`'s means `true`, and the SPA turns the composer's send
  into a split control — default Send, alternative "Interrupt & send" — for a
  working agent that advertises it. The session id is persisted as an additive
  `Agent` field needing no schema change (the `agents` table keeps the whole
  serde record in one column), captured by the activity pump, spent as
  `--resume <id>` instead of `--continue`, cleared when a session ends having
  never announced one, and absent whenever the crash window swallowed it — in
  which case today's `--continue` fallback stands untouched. The fake
  stream-json child learns the control protocol, so all of it is testable
  without a model turn. §11 q5 records the three decisions.
- **2026-08-28, merged main.** Main had independently reconciled its older
  copy of this spec with the store migration; that copy's one insight this
  branch lacked — activity flooding the resident tail and the two bounds it
  starves — is ported as §6.3, open. Everything else in this document remains
  the shipped record.
- **2026-08-24, step 7 shipped.** An outcome is a status on the agent's own
  message: `Thread::post_outcome` writes the summary the agent reported as an
  ordinary agent message carrying `completed` / `blocked` / `failed`, with the
  structured report attached to it, and `post_completion` and the
  `Done`/`Blocked`/`RunFailed` emissions behind a report are gone. The kinds
  and their classes stay, so every persisted row still means what it meant —
  held by a test that loads a pre-step-7 thread. Attention needed no new rule
  and no new copy of one: an outcome answers the unread question with the token
  of the event it replaced, so the reason on the wire, the inbox line and the
  push kind are unmoved, and the count per outcome is still exactly one. The
  catch-up exclusion inverts — outcome messages are carried, each prefixed with
  its outcome — which closes the §6.1 gap: a replacement agent now reads why
  its predecessor blocked out of the packet that carries what the human said.
  The Issue mirror needed no re-keying (a planned implementation's report is
  written onto the Issue's conversation directly, and never travelled through
  `run_outcome_mirrors_to_issue`), and the equivalence is held end to end by a
  test rather than by the helper's signature. What stays an event is what Build
  observed for itself: `Triaged`, `ReviewBlocked`, `IdleUnreported`,
  `Interrupted`, and every `RunFailed` raised where no report was made. Nothing
  in the SPA changed, because the step names nothing there — it is now exactly
  the older client §6.1's compatibility bullet describes.
- **2026-08-24, step 6 shipped.** `AgentProvider::ClaudeAdk` is a provider like
  any other and the first with no terminal, so every refusal and every
  conditional the earlier steps built is live in production rather than walked
  only by tests. `open_session` takes a `Carrier` and refuses a session that
  offers neither a terminal nor an activity stream; the activity pump posts the
  four kinds into the conversation and performs the two death rites that are
  not the terminal's; the idle sweep's `Working` short-circuit landed, tested
  from both sides; and a no-terminal spawn closes both screens it could be
  holding — the one clients wait on before a worktree has an agent, and the
  grid the session being replaced retained. Tested end to end against a
  recorded stream-json child, never a model turn. Resume by recorded session id
  is the one step-6 bullet still open; `--continue` carries it meanwhile.
- **2026-08-23, step 6's session half shipped.** `AdkSession`
  (`bridge/src/harness/adk.rs`) is the first carrier with no terminal, and
  `AgentSession::activity()` is the capability that stands in for the one it
  does not have — the two are alternatives, and both default to absent. The
  stream-json protocol was verified against claude 2.1.231 first and no flag
  the step assumed is missing, so the argv the provider half will build is
  unchanged from what §10 specifies. Four implementation decisions are recorded
  in the step note: an open turn is a flag rather than a count (a mid-turn
  message is absorbed and still ends in one `result`, so counting would report
  `Working` forever and the idle sweep would never explain a stopped agent), a
  successful result clears the reported error, only an `assistant` message's
  text is narration, and a tool result is named by the call it answers — which
  is what makes the `mcp__build__*` exclusion cover the answer as well as the
  call. Nothing chooses the carrier yet; `AdkHarness` and `open_session`'s arm
  are the provider half.
- **2026-08-23, step 6's edges made normative.** Review of the step-5a
  revision flagged three ways a no-terminal carrier falls through a path the
  PTY holds up by accident, and they are now requirements in §10 step 6
  rather than things an implementer would find out at runtime. A session
  whose `terminal()` is `None` must return `Some` from `activity()`, because
  the death rites hang off a stream closing and a carrier with neither stream
  would keep a dead tab reading as live until the idle sweep explained the
  exit as silence. §11 q4's `Working` short-circuit lands in this step, not
  earlier: it is behaviour design for a turn-boundary carrier, a no-op for
  the PTY, and must be tested as both. And a no-terminal spawn must close the
  screens waiting on it (`agent_screens_awaiting_spawn`) instead of dropping
  their clients onto a grid nothing will ever paint. Drifted line references
  refreshed against the tree (`thread.rs`, `orchestrator.rs`); the removed
  JSON store's `save_issue_implementation` no longer cites a line.
- **2026-08-23, the `send_turn` concurrency contract corrected.** §3 and the
  trait doc claimed callers must not hold the app-wide state lock across
  `send_turn`; `deliver` did exactly that, across the write and the 250 ms
  exit-race grace. `Tab.session` became an `Arc<dyn AgentSession>` so the
  delivery takes the handle out of the registry and speaks with the lock
  released, and the contract is now stated as the invariant every caller can
  hold: `send_turn` writes the turn out and returns, because
  `nudge_live_agent_tab` reaches a live tab from under the lock. Guarded by
  `a_turn_travels_with_the_state_lock_released`. Behaviour otherwise unchanged
  — a tab that closes while a turn is in flight no longer fails the delivery it
  already completed, it only has no quiescence clock left to restart.
- **2026-08-23, step 5a shipped.** The daemon holds a `Box<dyn AgentSession>`
  and names no terminal call: liveness and the crash code are
  `AgentStatus::Ended`, silence is `quiet_for`, a turn is `send_turn`, the
  teardown is `end`, and `subscribe` / `resize` / `pid` go through
  `terminal()`. `HarnessSession` is gone rather than shrunk — its remains are
  inherent on `PtySession`. Readiness moved into `open_session`, which now
  hands back the session's stream subscribed *before* that wait (a harness
  paints its whole startup, or its last words, while readiness is waited out)
  and asks for it only where Build will hand a turn, since `term.create`
  holds the state lock across a shell's open. `Harness::has_terminal` answers
  the digest before a session exists and defaults true, so no provider's
  digest moves. The wire is unchanged and no existing test was weakened.
- **2026-08-23, §11 q3 echoed in the body.** A consistency check found q3's
  conclusion — a no-terminal agent still gets a worktree, unconditionally —
  living only inside §11 and this changelog, while q4's is restated in §10's
  step 6 where the idle sweep depends on it. Step 6 now carries the same
  restatement at the point the headless spawn relies on it. No decision
  changed.
- **2026-08-23, the road past step 5 is specified.** §11 q3 answered — a
  no-terminal agent gets a worktree unconditionally, and the adoption /
  `run.adopt` / primary-checkout paths were read to confirm none assumes a
  carrier. §11 q4 answered — `done` stays the completion contract; the idle
  sweep's "quiet for N minutes" becomes "`status()` not `Working`, quiet
  past threshold, nothing delivered within threshold", byte-identical for
  the PTY and precise for a turn-boundary session. §3 gained the exit and
  quiescence surface (`quiet_for`, `exited_within`, `epitaph`, the
  send-turn-may-block contract). §10 gained step 5a (the daemon speaks
  `AgentSession` only, with the real call-site inventory; provider-sourced
  `has_terminal`), a concrete step 6 for the ADK (`harness/adk.rs`,
  stream-json protocol, `AdkSession`, the activity pump behind an
  `activity()` capability, identical MCP wiring, `--resume` by recorded
  session id), and step 7 for the §6.1 fix, now fully designed there: an
  outcome is a status attached to a message, the `Done`/`Blocked` emissions
  are dropped rather than reclassified, and the catch-up packet carries
  outcomes by construction.
- **2026-08-23, step 5 shipped.** The SPA reads `has_terminal` and offers the
  TUI button only where there is a terminal — dropped rather than dimmed, and
  absent still means `true` — and no path lets a terminal-less agent's panel
  enter TUI mode, including the remembered per-work-item face. The four
  activity kinds render in the timeline as folded, quieter rows; every other
  event kind, known or not, renders exactly as it did. `domPatch` gained one
  exception so a fold the reader opened survives the poll under it.
- **2026-08-23, step 4 shipped.** `Reasoning`, `ToolUse`, `ToolResult` and
  `Narration` are on `ThreadEventKind`, wire tokens `reasoning`, `tool_use`,
  `tool_result`, `narration`, all classed `Status` — so an agent thinking out
  loud moves no unread count, reaches no Issue conversation and sends no
  notification, with no new code anywhere those rules live. `catch_up_markdown`
  is messages-only and its limit counts messages, not items. Nothing emits the
  four kinds yet; §8's example is corrected to the envelope events actually
  ship in, and the class stays a property of the kind rather than a field.
- **2026-08-23, step 3 shipped.** `Tab.screen` is an `Option<TermScreen>`, the
  agent digest carries `has_terminal` (additive, and `true` for every session
  today), and the five terminal verbs refuse a session with no terminal in the
  manner of `require_shell_kind` — a sentence saying where that agent's work
  actually is, never a fallback. `term.attach` and `term.ack` joined the three
  the §7 table names, because both reach an agent tab that `agent.attach`
  would have refused. Nothing returns `None` from `terminal()` yet, so the
  refusals are exercised only by tests that construct the terminal-free
  session directly.
- **2026-08-23, step 2 shipped.** `agent_is_working` reads
  `AgentStatus::Working` instead of `has_exited` / `idle_for`, and
  `HarnessSession` gained `AgentSession` as a supertrait so the daemon can move
  off the wider trait a call at a time. The wire is unchanged and the pulse's
  existing tests pass unmodified; `working_is_exactly_the_two_conjuncts_it_replaced`
  in `pty.rs` is the equivalence proof, asserting `Working` iff the old pair in
  every state a PTY can be in.
- **2026-08-23, step 1 shipped.** The two traits, `AgentStatus` and `Turn` (a
  struct carrying `text`, answering §11 q2 with the minimal shape) are in, and
  `PtySession` satisfies both. `AGENT_WORKING_WINDOW` moved down into `pty.rs`;
  the four terminal calls both traits name became inherent on `PtySession` with
  the traits delegating, so there is one body per call. No call site in the
  daemon changed and no behaviour moved.
- **2026-08-23, §6.1 gap accepted, fix deferred.** The messages-only packet
  loses a blocked agent's reason, because `post_completion` records outcomes as
  events only and `last_completion` needs a structured report. Accepted for now
  to keep this spec a refactor. Decided for later: an outcome becomes a status
  attached to a message — a blocked agent sends a message with a blocked status
  — which puts outcomes back in the packet by construction.
- **2026-08-23, aligned with the store migration.** Swept the §6.2
  supersession through the rest of the document: step 4 no longer instructs
  building the withdrawn jsonl log, §7 and §9 describe SQLite paging instead of
  the log, and §11's window question is answered by shipped constants
  (resident tail 200, page 60). Named Pi and OpenCode as further
  session-protocol targets and stated the goal in §1: a polymorphic interface
  to agent wrappers, with no change in functionality. Refreshed drifted line
  references.
- **2026-08-22, restored.** This file was clobbered in `0eb5b5b` by a stray
  write that replaced it with a copy of the store spec, and the loss rode in on
  a `git add -A`. Restored from `97611dd` and brought up to date. The store work
  it was waiting on is done; the session side is not started.
- **2026-08-21, §6.2 superseded.** The store is moving to SQLite
  (`Store Migration Spec.md`), which removes the write amplification the jsonl
  activity log was designed to route around and gives pagination directly. The
  activity kinds land as ordinary thread rows. Step 4's dependency order changes:
  it now waits on store phases 1–3.
- **2026-08-20, §6 decided.** Catch-up carries messages only, dropping the
  lifecycle summaries it used to include. Activity is persisted in a per-agent
  append-only jsonl log rather than the Issue's aggregate record — the store has
  no database and rewrites the whole aggregate per append, so activity in the
  record would be O(n²) bytes written per session. The open question narrows from
  "what is the retention rule" to "how large is the in-memory window".
- **2026-08-20, revised.** Activity moves into the conversation as typed
  `Status` events; the second tab disappears for event-stream harnesses and the
  terminal becomes purely an escape hatch for opaque CLI wrappers. Removed the
  event ring, the cursor protocol and the RPC. Added §6 — the catch-up packet
  and thread-retention consequences the move creates.
- **2026-08-20, first draft.** Proposed a separate `AgentEvent` stream with its
  own ring, cursor and `agent.events` RPC, and a second rail mode named by
  capability (`TUI` or `Activity`).

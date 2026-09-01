# Agent Session Interface — Spec

**Status:** Draft — steps 0–10 shipped: the ADK is a provider and the first
carrier with no terminal, an outcome is a status on the agent's own message,
activity floods neither bound, and every carrier names the conversation it is
having — a terminal off its harness's own transcript tree — so a respawn
resumes by name and a brand-new agent record starts fresh (see §10)
**Last updated:** August 30, 2026
**Branch:** `build/agent-polymorphism` (steps 0–9); `build/session-identity`
(step 10); `build/background-visibility` (step 11, specified);
`build/new-agent-flow` (step 14, specified); `build/chat-polish` (step 15)

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
    /// not announced yet. Never scraped off a screen: a protocol carrier reads
    /// it off the `init` line the child sent, and a PTY carrier reads it out
    /// of the harness's own durable records — the same transcript trees the
    /// resume probe has always read (§10 step 10).
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
  *(Amended by step 11: once `Working` can outlive the turn, `can_interrupt`
  answers "is there a turn to stop AND can this child stop one" — a refusal
  still implies `false`, but a `false` may also mean "no turn open", the case
  §8.1's stale-press guard already answers with the satisfied no-op.)*

- **`session_id` was first written protocol-only, and step 10 widens it.** The
  doc originally read "never out of a transcript directory", which drew the
  no-scraping line in the wrong place: the scope doc's rule forbids parsing
  PAINT, and the transcript tree is the harness's own durable record — the
  thing `has_transcript` has read since adoption shipped, and where the resume
  a spawn performs actually lives. A PTY carrier answers from a locator over
  that tree (§10 step 10); what stays forbidden is the screen.

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
    /// The agent called a tool. Since step 12 the row is the whole call: its
    /// answer updates it in place rather than minting a `ToolResult` beside it.
    ToolUse,
    /// A tool answered. Step 12 pairs the answer into the `ToolUse` row, so
    /// new sessions stop minting this on the happy path — the kind stays,
    /// because stored rows render forever and the orphan fallback still
    /// produces it.
    ToolResult,
    /// The agent narrated. Distinct from a `post_thread_message`, which is the
    /// agent deliberately addressing the human.
    Narration,
    /// Background work the harness runs beyond the turn moved — started,
    /// finished, failed, or said something worth reading. (step 11)
    TaskUpdate,
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

All four new kinds are **`Status`** — and the fifth, `TaskUpdate` (step 11),
joins them for the same reason: background work moving is the agent working,
never the agent addressing anyone. So:

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

`Thread::catch_up_markdown` (`bridge/src/thread.rs:2337`) took the last N items
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

### 6.3 Activity must not flood the resident tail — shipped

> **Merged from main's reconciliation of this spec (written 2026-08-23, merged
> 2026-08-28)** — steps 4 and 6 shipped without it, so this was the one §6
> obligation still open once a headless carrier was live. **Designed
> 2026-08-29** ("The fix, designed" below); §10 step 9 is the implementation,
> **shipped 2026-08-29** in `51c6d0b`, `48d3933`, `50f2cb7` and `a7efeb8` —
> see the step-9 note in §10 for what the implementation decided. The client
> half is `c0d1b89`: the SPA needed **no change**, and the three tests that
> prove it are the last thing §6.3 owed.

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
built from a store query for messages rather than the resident tail (a
`WHERE`, not a scan — though the store must first be taught to ask it: only
`attention` is hoisted out of the item JSON today), and the page a client
opens on is measured in conversation, with the activity between two messages
travelling folded beside them instead of consuming the budget that decides how
far back the human can see.

**The fix, designed 2026-08-29.** One hoisted column funds both remedies, and
one predicate is the whole of the new counting.

**The predicate: an item is *counted* when the human reads it as
conversation.** Counted = it is a message (either role — and outcome messages
are messages since step 7, so §6.1's inversion carries into every reader
below by construction) or an attention-classed event (Build calling the
human: `Blocked`, `IdleUnreported`, `Interrupted`, `RunFailed`, `Merged`…).
Every Status-classed event rides free — the four activity kinds, and the
quiet lifecycle markers with them, `Triaged` among them: triage orders what
the reviewer reads and asks nothing of them, so it never marks the entry
unread and never buys a slot here. In Rust this is one new reading,
`ThreadItem::counted()` = `matches!(item, Message(_)) ||
attention_reason().is_some()`; in SQL it is `message = 1 OR attention = 1`
over two hoisted columns, and a test holds the two readings equal across
every kind.

**Schema v3 hoists `message`.** `thread_items` keeps each item whole in its
JSON column and hoists only what SQL must filter on — today that is
`attention` alone (`store.rs:427`), so "message or event" is not yet a
question the store can put in a `WHERE`. The migration repeats the v1→v2
precedent exactly (`add_attention_column` / `classify_stored_items`,
`store.rs:625/644`): a `message INTEGER NOT NULL DEFAULT 0` column added
before the schema batch (the new index names it), written where
`write_agents` already writes `attention` (`store.rs:933`), backfilled in
Rust by decoding rows — no `json_extract` dependency — and `SCHEMA_VERSION`
bumped to 3, with the v1 path running both classifiers. One partial index
serves both new queries: `thread_items_conversation ON thread_items(agent_id,
sequence) WHERE message = 1 OR attention = 1`. Each new statement repeats
that predicate verbatim so the planner's implication check is trivial, and
the `EXPLAIN QUERY PLAN` test that pins `THREAD_PAGE_SQL` /
`THREAD_CURSOR_SQL` (`store.rs:2299`) grows to pin these two.

**Remedy 1 — the catch-up packet reads the store when the tail is starved.**

- **The query.** A store fn beside `thread_page`:
  `thread_message_page(agent_id, limit)` — `SELECT item … WHERE agent_id = ?1
  AND (message = 1 OR attention = 1) AND message = 1 ORDER BY sequence DESC
  LIMIT ?2`, reversed the way `read_thread_page` reverses. Messages only,
  both roles, outcomes included because they are messages.
- **N stays 40** (`CATCH_UP_MESSAGES`, replacing the literals at
  `orchestrator.rs:654` and the router's call). §6.1 already made the limit
  count messages and the 12 KB newest-first byte bound stays the real cap;
  this remedy changes where the packet reads, never how much it says.
- **The gate, and the common case.**
  `Thread::catch_up_reaches_stored_history(limit)` — the sibling of
  `page_reaches_stored_history` — is `earlier_item_count > 0 &&` resident
  messages `< limit`. A thread whose store holds no history (everything
  resident — the common small case, and every storeless test daemon) answers
  `false` off two integers, touches no SQL, and hands a byte-identical
  packet at today's speed. A long thread whose tail still holds 40 messages
  is answered from memory too; only the starved tail pays the read, and it
  is the one the fix exists for.
- **The merge is the `wire_value_after_including_history` precedent**
  (`thread.rs:2235`): store rows are admitted only below
  `resident_from_sequence`, the tail is the fresher copy of everything it
  still holds, and the chain then runs the same filter-take-reverse. (The
  packet renders only fields that never mutate in place — role, outcome,
  body, attachments — so the sequence filter is there to keep a row from
  appearing twice, and the freshness rule is discipline held with the
  precedent rather than a correctness need.) Composition lives in
  `Thread::catch_up_markdown_including_history(history, limit)`, beside the
  tail-only fn it extends.
- **Where it runs: at delivery, not where the turn is built.**
  `conversation_prompt` (`orchestrator.rs:635`) bakes the packet into
  `AgentTurn.cold` at transition time, inside orchestrator fns that hold a
  `&Thread` and no store. Rather than threading store reads through every
  transition, the packet moves to the one door every cold prompt already
  passes: `conversation_prompt` keeps the prompt + protocol block,
  `PendingAgentTurn` gains `wants_catch_up: bool` (true from every
  constructor except the router's push — a router is one decision long and
  its prompt deliberately carries no packet), and
  `deliver_pending_agent_turns` (`app.rs:17604`) composes the packet and the
  `last_completion` block onto `cold` just before `deliver`, through an
  App-level helper (gate → query → merge) that `router_read_conversation`
  (`app.rs:8790`) calls too, so the router's read-only transcript stops
  starving with the packet. Late binding is a small correctness gain of its
  own: a packet baked at queue time misses messages posted while the turn
  waited for the lock; one composed at delivery does not.
- **One behavior change, owned:** `PendingAgentTurn::for_recovery`
  (`app.rs:919`) wraps cold *and* warm today, so a warm recovery currently
  receives a packet. It stops: a warm recovery is a live process that lived
  the conversation, the protocol block it keeps instructs
  `read_unread_messages`, and the packet there was belt-and-braces.
- **Failure:** a store error at the drain logs and falls back to the
  tail-built packet — a starved packet is today's behavior, and dropping the
  turn would be worse than either.

**Remedy 2 — the page is measured in conversation.**

- **The counting rule, exact.** A page's `limit` buys **counted items**.
  Walking newest→older from the seek, every item ships and the walk stops at
  the `limit`-th counted item — so activity *between* the counted items
  travels with the page uncounted, folded beside the messages it sits
  between, and activity older than the page's oldest counted item waits for
  the next page. A page stays one contiguous run of sequences.
- **The ceiling.** A page ships at most `limit ×
  THREAD_PAGE_SPAN_FACTOR` items, factor **10** — the hard bound that stops
  an all-activity stretch from being unbounded. When the ceiling stops the
  walk before the budget fills, the page ends higher and `has_more` says so;
  the client scrolls again. Worst cases, stated: the default page of 60 → at
  most 600 items; the clamp maximum 200 → at most 2000, only on an explicit
  scroll-back ask; the `SMALLEST_THREAD_PAGE` polls (`thread_limit: 1`) → at
  most 10 items a tick where they get exactly 1 today — which is why the
  ceiling is a multiple of the budget rather than a flat constant.
- **`has_more` and `oldest_sequence` keep their wire meaning exactly.**
  `oldest_sequence` is the oldest item shipped, counted or rider; `has_more`
  is whether items of any kind remain below it; `thread_total` still counts
  every item. Pages abut at their seeks, so a client walking
  `before_sequence = oldest_sequence` sees every row exactly once and skips
  none.
- **Where.** `Thread::wire_value_page` (`thread.rs:2286`) walks the new rule
  over the tail. `page_reaches_stored_history` (`thread.rs:2022`) becomes
  the counted gate: the store answers when `earlier_item_count > 0`,
  resident counted items below the seek `< limit`, *and* resident items
  below the seek are under the ceiling (a tail that fills the ceiling is a
  full page from memory). The store fn beside `thread_page`:
  `thread_conversation_page(agent_id, before, limit)` → `(items, has_more)`
  — one seek down the partial index for the `limit`-th counted sequence
  below the seek (`ORDER BY sequence DESC LIMIT 1 OFFSET limit − 1`; none
  found → floor 0), one read of that span newest-first `LIMIT` ceiling,
  reversed, and one `LIMIT 1` probe below the shipped floor for `has_more`.
  `stored_thread_page` (`app.rs:10631`) calls it in place of
  `thread_page(limit + 1)`; the raw `thread_page` stays for the boot's tail
  read, which is unchanged — `RESIDENT_CONVERSATION_TAIL` still counts
  items, because residency bounds this process's memory, not what the human
  sees.

**Wire and SPA, stated honestly.** No field changes meaning and none is
added. What changes is composition: a page may carry more items than its
`limit` — at most ten per unit — with `limit` bounding the conversation in
it. The shipped client needs no change, for reasons read out of it rather
than hoped: `absorbOlderPage` (`core/thread.js`) merges whatever items
arrive and trusts the daemon's `has_more`; the gap check compares
`thread_total`, whose meaning is unmoved; a page folds in only at its seek;
nothing in `thread.js` assumes `items.length <= thread_limit`; and activity
folding shipped with step 5. Mutation answers (`MUTATION_THREAD_PAGE`) and
the smallest-page polls grow by at most the factor, bounded above. The
Client check stage has one job: hold the merge path against an oversized
page (more items than its `thread_limit`) — a test, not a change.

**Verdict, 2026-08-29 (`c0d1b89`): no change, and for the reasons read out
above.** The whole of the SPA's paging is `createThreadCache`
(`spa/src/core/thread.js`), reached from one surface (`agentRail.js`), and
nothing in it counts items against a limit: the soundness check is
`merged.length <= thread_total` — a window may not be larger than the
conversation it is a window on, which an oversized page never is — and the
deletion check predicts `thread_total` from `thread_total`, both meanings
unmoved. `absorbOlderPage` merges whatever arrives, keyed by sequence and
sorted, guarded by the seek rather than by any size. The `thread_limit` the
client asks with is a request, which the comment above `FIRST_PAGE_ITEMS`
already said.

Three tests hold it, driven by a JS mirror of `page_span` so the fixtures are
pages the daemon would actually cut. A first page of 360 items against a
limit of 60 — eighty turns of five tool calls — opens a sound window and
engages the forward cursor. A conversation dense enough for the ×10 ceiling
to end its pages early (fifteen tool calls a turn, so the first page carries
37 messages, not 60) still walks back to the start with every sequence seen
exactly once and none skipped. And a poll delivering a turn's message plus
its five tool calls does not trip the gap check, whose `thread_total` counts
them. They are not vacuous: restoring an item-count bound to the soundness
check fails all three.

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
| `catch_up_markdown` (`thread.rs:2337`) | last 40 items by recency, events included | messages only, the limit counting messages (§6.1) — **shipped**; read from the store when the tail is starved and composed at delivery (§6.3, step 9) — **shipped** |
| `Thread::items` (`thread.rs:1255`) | resident tail of 200 items, older items paged from SQLite | residency unchanged — activity rows ride the same tail; the pages and the packet stop counting them (§6.3, step 9) — **shipped** |

### SPA

| Site | Today | Change |
|---|---|---|
| `agentRail.js:177` | Chat / TUI switch | **TUI button shown only when `has_terminal`** — shipped |
| thread rendering (`core/thread.js`) | messages + lifecycle events | renders the five activity kinds; folded by default — shipped |
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
// thread items — the activity event kinds (step 4's four; step 11 adds
// task_update), in the envelope every event already has
{ "type": "event",
  "data": { "id": "event-41", "sequence": 41, "event": "tool_use",
            "created_at": "…", "summary": "Read bridge/src/app.rs" } }
```

```json
// the same row after its answer arrived (step 12): two additive fields.
// `updated_sequence` re-ships it through the existing cursor — the same
// machinery a message marked seen rides — and `outcome` is
// "ok" | "error" | "unanswered", absent on every event that is not a
// completed tool call. A client that knows neither field reads a slightly
// longer summary on a row it already holds.
{ "type": "event",
  "data": { "id": "event-41", "sequence": 41, "updated_sequence": 47,
            "event": "tool_use", "created_at": "…",
            "summary": "Read bridge/src/app.rs\n→ fn main() {}",
            "outcome": "ok" } }
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
> echoing the wire token. *(Superseded 2026-08-30 by the note below: the
> picker no longer offers a carrier at all.)*
>
> **The carrier is an account setting, not a start card (2026-08-30).** Two
> cards for one agent made every start re-answer a question with one right
> answer, and put two Claude Codes side by side with only a parenthetical to
> tell them apart. So the choice moved to the account: `settings.get` /
> `settings.set` carry `claude_mode` (`"headless" | "tui"`, absent means
> headless) and a `codex_mode` locked to `"tui"`, and `model_choice_from`
> resolves the generic token `claude` to whichever carrier the mode names.
>
> In the SPA, `STARTABLE_PROVIDERS` is back to two entries — `claude` and
> `codex` — and Account → Settings holds the mode controls ("Claude Code" /
> "Claude Code TUI", plus Codex's locked field with its reason). The naming
> table behind `providerLabel` still answers for `claude_adk`, because a run
> persisted on it must read as itself: both carriers are called "Claude Code"
> everywhere a person can see, including on the restart offer of a worktree
> that ran one, and `normalizeModelCatalog` folds the catalog's two entries of
> that name into one so the Account page's agent dropdown asks once. The word
> "headless" survives only as a wire token — never as anything a person reads.

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
8. ~~**The turn can be stopped, and the conversation resumed by name.**~~ The two
   things step 6's live probes made possible: `AgentSession::interrupt`
   (capability-gated, never a kill) with `thread.post`'s `interrupt` flag and
   the composer's split send above it, and `AgentSession::session_id` persisted
   on the agent record so a respawn carries `--resume <id>` instead of
   `--continue` — the one step-6 bullet that did not ship. Detail below. The
   two halves are independent and the resume half may land first, but they
   share one capture point and one test child, so they are one step.
   **Shipped**: the bridge half in `66ad2ac`, `defa1be` and `f3ec04b`, §8.4 —
   the composer's split send — in `07980b3`, `8d114b4` and `7eb0398`.
9. ~~**Activity stops flooding the two bounds**~~ — §6.3 as designed there: schema
   v3 hoists `message` beside `attention` with one partial index over the
   counted predicate; the catch-up packet composes at
   `deliver_pending_agent_turns` from a store query when the tail is starved
   (byte-identical and SQL-free for an all-resident thread); and a page's
   `limit` buys counted items with activity riding free under a ×10 ceiling,
   `has_more` / `oldest_sequence` / `thread_total` unmoved. Bridge-only; the
   SPA's part is one oversized-page test. **Shipped** — see the note below,
   and `c0d1b89` for the client half, which was the test and nothing else.
10. **Every carrier names its conversation, and a fresh agent record is a
    fresh conversation.** `AgentSession::session_id` answered by the PTY
    carriers too — the `Harness` supplies a locator over its own transcript
    tree, the spawn installs it into `PtySession` — captured by the idle
    sweep through the step-8 record path, spent per carrier
    (`--resume <id>` for claude, `codex resume <id>` for codex), and verified
    against the same tree before it is spent. With it, the sharper spawn
    rule: a persisted id resumes exactly; an agent record with history keeps
    the `--continue` guess; a brand-new agent record resumes NOTHING — with
    adoption's continue-pickup preserved by the flag adoption already sets.
    Detail below. **Shipped 2026-08-29** in `6d937fc`, `178085d`, `480d3a8`
    and `55e7f10` — see the note after §10.5.
11. **Background tasks are visible, and the agent stays Working while they
    run.** Headless carrier only. The stream's `system` task events —
    `task_started`, `task_updated`, `task_notification` and
    `background_tasks_changed` (all observed live 2026-08-29; payloads pinned
    at implementation, not here) — reconcile a live-task set on
    `ProtocolState` with `background_tasks_changed` as the source of truth;
    membership transitions mint the fifth activity kind, `TaskUpdate` (wire
    `task_update`, class `Status`, riding the existing pump and §6.3's free
    ride); and `status()` reports `Working` while a turn is open OR the set
    is non-empty, which the idle sweep's existing short-circuit honours with
    NO new sweep code. `can_interrupt` stays tied to an open turn, `Ended`
    still wins, the death rites are untouched. Detail below.
    **Shipped** — the bridge half 2026-08-29, the SPA's one activity-map
    entry 2026-08-30.

> **Step 9 shipped in `51c6d0b`, `48d3933`, `50f2cb7` and `a7efeb8`**, as
> designed in §6.3. The two failures it exists for are the two tests that
> would have caught them: a resident tail holding only activity still yields a
> packet carrying the human's messages, and a first page over an
> activity-heavy conversation still shows what was said with `has_more` /
> `oldest_sequence` honest for a client walking back by sequence. Five things
> worth knowing.
>
> **`conversation_prompt` lost its thread entirely**, rather than keeping it
> and skipping the packet. Once the packet composes at the drain, the previous
> completion report has to move with it — both are the durable conversation,
> and splitting them would have left one read at transition time and one at
> delivery. So the prompt builder takes a `&str`, `AgentTurn::dispatched` /
> `posted` lose their thread parameter at fourteen call sites, and
> `append_durable_conversation` is the second half, called once.
>
> **The packet's conversation is now one rule, and it is the right one.** At
> the drain the thread comes from `agent_conversation(owner, agent_id)` — the
> conversation the agent SPEAKS in, which for a planned implementation's first
> agent is its Issue's. The orchestrator's turns used to build their packet
> from `&active.agents`, the run's OWN thread, where a planned implementation's
> human never says anything: the §6.1 failure in a third shape, fixed here as a
> side effect of asking the question in one place. Six tests that asserted the
> reviewer's words were in `queued.cold` now read the prompt the turn is
> handed over with, which is what they always meant.
>
> **`for_recovery` gave up its `issue_thread` parameter** and, with it, a
> fabricated fallback roster one call site built purely so the packet would
> have something to read. The warm half stops carrying a packet, as designed.
>
> **The ceiling is a real bound, not a formality.** Writing the tests found it
> biting immediately: a page of 5 over a session emitting fifteen tool calls
> per message carries three messages, not five, because fifty items is the
> most it may ship. That is the design working — the page ends higher,
> `has_more` says so, and the client scrolls again — but it means the factor,
> not the budget, is what decides a page's shape on an activity-dense thread.
>
> **`has_more` is answered off the page's own oldest item, never off the floor
> the seek asked for.** The ceiling can stop the read above that floor, and a
> page that claimed to reach a floor it never sent would tell a client to seek
> past rows it does not hold. The store's backward-walk test holds it.

> **The client half is `c0d1b89`, and it is a test.** The judgement §6.3 asked
> for came back the way that section had read it: the SPA's paging is one
> cache (`createThreadCache`) reached from one surface, it bounds a window by
> `thread_total` and by the seek a page was fetched at, and it counts nothing
> — so a page carrying ten items per unit of budget merges the way any other
> page does. The three tests drive that merge with pages cut by a JS mirror of
> the daemon's `page_span`, including the ceiling case where a page ends
> higher than the budget asked for, and all three fail if an item-count bound
> is put back into the soundness check. The verdict, with what was read to
> reach it, is at the end of §6.3.

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
  `tool_result` as `ToolResult` (step 12 later pairs it into the call's own
  row), assistant text as `Narration` — and on
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
>
> **Live-verified against claude 2.1.236 (2026-08-28), interrupt included.**
> The final review added a second ignored test beside the steering one,
> `real_adk_session_interrupts_mid_tool`, and ran both against the real binary.
> Steering: a follow-up written mid-tool decided the same turn's outcome (no
> second turn, no drop). Interrupt: the child announced
> `interrupt_receipt_v1`, the `control_request` written while a 120-second
> tool ran cut the turn 8 seconds after the ask, the interrupted result left
> no epitaph, and the steering turn queued behind the interrupt ran in the
> same session — it wrote the file whose name only the FIRST message carried,
> which is what proves the conversation survived its own stop.

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

- **Unless there is no turn open, in which case it writes nothing and returns
  `Ok`.** The control is offered off a digest up to 1.6s old, so the result can
  close the turn inside that window, or race the ask by milliseconds. Such a
  press is one that arrived too late: the turn the human meant to stop is
  already over, so the ask is satisfied and the message it rode in on is
  delivered as the ordinary turn it now is. The guard is one `if` under the
  same lock the pending interrupt is recorded under, and it is doing two jobs:
  - **Nothing is recorded.** Taking the pending on a result holds the "an
    interrupt can never leak into the turn after it" rule only when a result
    intervenes. A pending recorded against a turn already closed is marked
    `steered` by the send that follows, and that turn's OWN result then sets
    `turn_open = steered = true` with nothing running — the session reports
    `Working` indefinitely (blocking the idle sweep, which demotes only what is
    not working) and that turn's real error is cleared from the epitaph.
  - **Nothing is written.** The live child announces
    `interrupt_cancel_queued_v1`, so a `control_request` sent with nothing
    running is a request that could take the queued turn with it. An
    unmatched ack, were one to come anyway, is already ignored as noise.

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
  the option is carrier-neutral in shape only. *(True as shipped; step 10
  retires it — the PTY carriers grow locators and their own resume arms, and
  the field becomes carrier-neutral in fact.)*

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

> **Shipped in `07980b3`, `8d114b4` and `7eb0398`**, as designed: the condition,
> the two shapes of the control, the in-place swap and the flag on the post.
> Test 11 of §8.5's list is six tests in `agentRailDom.test.js` — the plain send
> for an agent with nothing to stop and for one that is not working, the split
> for one that is both, the flag on the alternative, no flag on the default
> press, and the swap that keeps the box and the words in it. Four things the
> implementation decided that the step did not say.
>
> **The control is `splitButtonMarkup`, and the menu came out of
> `mountSplitButton` to meet it.** Mounting the whole split button would have
> been the obvious reading and it breaks the composer: `mountSplitButton` is a
> single-flight latch that leaves the primary DISABLED when the action resolves,
> because its callers repaint or navigate — a composer wired that way could be
> sent from exactly once. The composer's press is a submit that restores its own
> button, before any repaint, and has been since the plan and diff composers
> drifted apart over it. So `mountSplitMenu` — the caret, the toggle, the
> outside-press watch armed in the same event cycle, the choice — is now shared
> by both, and `wireThreadComposer` keeps the press.
>
> **The send keeps its id in both shapes.** `splitButtonMarkup` gained
> `primaryId` so one lookup (`#railsend`) finds the button whether it is the
> plain one or the primary half of a split, which is what lets the submit path,
> the Cmd+Enter path and the tests stay as they were. What the split shape drops
> is the arrow icon and the label span it wrapped — the caret stands where the
> arrow was — and `sendLabel`'s existing fallback already drives a button
> without one.
>
> **The swap is refused mid-press.** `setCanInterrupt` returns without doing
> anything while a send is in flight (the button's word is "sending…" and
> belongs to that press) or while the menu is open (a choice is being made). The
> poll comes round 1.6 s later, by which time the press has landed. This is the
> same rule `mountSplitButton` holds for its own remount, for the same reason.
>
> **The condition is asked of the agent whose conversation is open**, not of the
> work item's feed row. The rail reads two sources — the pinned status line
> clocks the row's `working_time`, the bubbles read each agent's own digest —
> and an interrupt stops ONE session's turn, so the composer reads the digest of
> the agent it is writing to.

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
    alternative posts `interrupt: true`;
12. a press that lands between turns — the turn's result has already closed it
    — is not spoken to the child at all, and the turn sent behind it ends
    `Waiting` with its own error kept. Against a child that acks a
    `control_request` and emits NO result of its own, since a turn that was
    never running has none to end: the only result that follows is the next
    turn's, which is exactly what a leaked pending would swallow.

### Step 10 in detail — the session id is polymorphic, and a fresh agent starts fresh

Grounded in live probes (2026-08-29), not in guesses about the harnesses.
claude writes one transcript per conversation at
`~/.claude/projects/<munged-cwd>/<session-uuid>.jsonl`, and **the filename is
the id**. codex writes global dated rollouts at
`~/.codex/sessions/<Y>/<M>/<D>/rollout-<timestamp>-<uuid>.jsonl`, with the
session's cwd in the first metadata line — global, so cwd matching is
mandatory: recency alone misattributes the moment two codex sessions run.
`claude --resume <id>` works in the TUI exactly as it does headless, and the
installed codex CLI resumes by `codex resume <SESSION_ID>` — a subcommand,
not a flag. No screen is ever scraped: everything below reads the same
durable trees the transcript probe (`Harness::has_transcript`) has always
read, which is why §3's `session_id` doc is amended rather than bent.

The step also carries the behaviour change that makes it urgent rather than
merely sharp. The spawn reservation runs the transcript probe
unconditionally today (`app.rs:17434` — "the probe is unconditional too"),
so a brand-new agent spawned into a worktree holding any old transcript
inherits the newest conversation in it. That is a misdelivery, hit live
2026-08-29: a new headless agent adopted the worktree's old conversation.
§10.4 ends it without breaking the one flow that inheritance was built for.

#### 10.1 The locator — polymorphic on `Harness`, evaluated by `PtySession`

```rust
/// Finds the name a harness gave the conversation a PTY session is having,
/// by watching the harness's own transcript tree — the durable records the
/// resume probe already reads, never the screen.
pub trait SessionLocator: Send + Sync {
    /// The id, once exactly one transcript this session could be has
    /// appeared. `None` until then; cached once found, so a locator never
    /// changes its answer and the steady-state cost is a field read.
    fn session_id(&self) -> Option<String>;
}
```

- **The `Harness` supplies it.** `fn session_locator(&self, home: &Path,
  cwd: &Path) -> Option<Box<dyn SessionLocator>> { None }`, in
  `harness/mod.rs` beside the trait — the mirror of `has_transcript`, with
  `home` a parameter for the same reason: tests never read the real
  `~/.claude` or `~/.codex`, they fake the transcript trees in tempdirs.
  `ClaudeHarness` and `CodexHarness` override; `AdkHarness` keeps the
  default `None`, because its session answers `session_id` from `init` and a
  locator beside that would be two records of one answer, free to disagree —
  the flag-standing-beside-the-call shape this spec rejects everywhere else.
- **Claude's locator reuses the path knowledge that exists.**
  `encode_project_dir` and the `.jsonl` listing (`harness/claude.rs:122/132`)
  are called, not copied. At construction it snapshots the set of `.jsonl`
  stems in `~/.claude/projects/<munged-cwd>/`; `session_id()` lists again
  and considers only stems not in the snapshot. The stem is the id.
- **Codex's locator reuses the header parse.** `transcript_exists`'s
  `/payload/cwd` read (`harness/codex.rs:185`) moves into a shared
  `rollout_header` helper both call. Rollouts are global, so the candidate
  walk is date-bounded — only date directories on or after the construction
  date, with the rollouts already present there snapshotted — and
  **cwd-matched**: a candidate counts only when its header's canonicalized
  cwd equals the session's. The id is read from the same header
  (`payload.id`), with the filename's uuid as the fallback — both are
  codex's own record.
- **One candidate, or none.** Exactly one candidate → cached and answered
  forever. More than one → `None`, indefinitely. A branch legally carries
  several agents in one checkout, and two sessions spawned together there
  produce two new files nobody can tell apart; guessing by recency is the
  misattribution this step exists to end, so the locator refuses to guess.
  The cost is the probe's own bargain restated: a missing id never errs
  (§8.2 — no code path may treat one as an error), it only forgoes the
  sharper resume, exactly as `has_transcript`'s "a false negative costs a
  fresh session, never a wrong one".
- **Built at the spawn reservation, before the child exists.** The
  reservation (`app.rs:17435`'s neighbourhood) builds the locator through an
  injectable seam beside `transcript_probe` — `session_locator_factory`,
  defaulting to `harness_for(provider).session_locator(home, root)` — so
  the snapshot can never contain the child's own file. It travels with the
  reserved spec through `Tab::spawn` into `open_session`'s Terminal arm and
  into `PtySession`, which stores it and answers
  `AgentSession::session_id()` by delegation: lazy, cached by the locator,
  `None` until the file appears. Shells and the Protocol arm take no
  locator, held by a test: the provider that opens `Protocol` answers `None`
  from `session_locator`.

#### 10.2 Capture — the idle sweep, writing the step-8 record path

- **The capture point is the idle monitor's tick** (`spawn_idle_monitor`,
  `app.rs:6100`; 5 s, `main.rs:296`), not the status poll. The digest poll
  is client-driven: with no browser open nothing would ever be captured —
  and a daemon running agents unattended is normal here — while every
  attached client would multiply the filesystem probe by its own poll rate,
  on the RPC path that answers the SPA from under the lock. The sweep is
  daemon-owned and fixed-cadence, it already wakes beside
  `mark_idle_tasks`, and 5 s bounds the crash window to the same order as
  the activity pump's event-driven capture.
- **Asked with the state lock released** — the `deliver` precedent. The
  tick collects each live agent tab's `(owner, agent_id,
  Arc<dyn AgentSession>, recorded id)` under the lock, drops it, asks
  `session_id()` — the one call that may touch the filesystem — then
  re-locks and writes each answer that moved through
  `record_agent_resume_id` (`app.rs:3086`): the exact path step 8 built, so
  a captured PTY id and a captured headless id are the same record written
  by the same hand. Compared before written, so a session costs one write
  however long it lives.
- **The byte pump's close arm takes one final reading**, just before
  `record_agent_session_end` (`app.rs:17828`): a session shorter than a
  tick is still captured at EOF, and the respawn that needs the id is the
  very next thing after a close. It records; it **never clears**. The
  headless pump's clearing rule (§8.2) keys on a child that ended without
  announcing, which for that carrier means a dead `--resume` id — but a PTY
  session resumed in place legitimately makes no new file, so `None` at
  close is its normal answer, and clearing on it would throw away a good id
  at every restart. The dead-id problem moves to the one place it can be
  answered exactly: §10.3's verification.
- The activity pump is untouched: the headless carrier's capture and its
  clearing stay exactly as step 8 shipped them.

#### 10.3 Spending the id — the argv per carrier, verified first

- **`ClaudeHarness` gains `AdkHarness`'s match verbatim** (`adk.rs:110`):
  `--resume <id>` when the record names one, `--continue` when it does not
  and the probe said yes, neither otherwise — never both, for step 8's
  reason. `--resume <id>` is already the headless shape and works in the
  TUI (verified live 2026-08-29).
- **`CodexHarness` gains the same three-way match in a different argv
  shape**: resume is a **subcommand**, not a flag. Today `resume --last` is
  appended at the argv tail, after the global `--config` overrides
  (`codex.rs:148`); the named form keeps that position and swaps the
  argument — `resume <SESSION_ID>` (verified live 2026-08-29), `resume
  --last` as the guess, nothing otherwise. The overrides staying ahead of
  the subcommand is the shape that already works.
- **A recorded id is verified against the same tree before it is spent.**
  `Harness::holds_conversation(&self, home: &Path, cwd: &Path, id: &str) ->
  bool`, default `true`: claude stats `<munged-cwd>/<id>.jsonl`; codex
  walks rollout FILENAMES for `-<id>.jsonl` (no file is opened);
  `AdkHarness` delegates to `ClaudeHarness`. The reservation filters the
  recorded id through it (via an injectable sibling of the probe,
  `resume_id_probe`) and clears a filtered-out id through
  `record_agent_resume_id` on the spot — so a dead id costs zero restarts
  on every carrier, sharpening §8.2's "one restart", whose pump-side
  clearing stays as the backstop. This is also why a provider swap is safe:
  an agent moved from claude to codex holds a claude uuid codex does not
  recognize, and the check clears it instead of letting `codex resume`
  choke on it.
- **A dividend worth naming:** an id captured under one claude carrier
  resumes under the other. TUI ⇄ headless provider swaps keep the exact
  conversation, because both write the same tree and both spend
  `--resume` — the polymorphism paying for itself.

#### 10.4 The sharper spawn rule — a fresh record is a fresh conversation

The reservation decides in order, and the order is the rule:

1. **A recorded id that verifies → resume by name.** No `--continue` beside
   it, ever.
2. **No id, but this agent's own record shows history — its
   `thread.sessions` is non-empty → today's continue-guess**, still gated
   on the transcript probe. This is the crash window (§8.2): a session that
   died before capture, an agent from before this step shipped. The SAME
   agent continuing ITS OWN conversation, which is the only history Build
   holds.
3. **Otherwise fresh: no resume of any kind, on every carrier.** A
   brand-new agent record has no conversation to pick up, and the worktree's
   old one belongs to whoever had it — an adopted branch's first agent
   included.

- **The gate is one reading:** `may_pick_up = !agent.thread.sessions
  .is_empty()`, and `continue_session = recorded-id absent && may_pick_up
  && probe`.
- **Whose `sessions` — stated exactly, because lineage is not per-agent
  today.** `record_agent_session_start` writes through `edit_owner_thread`
  (`app.rs:3000`), which edits the ROSTER's first agent's thread — so for
  the first agent, its own `thread.sessions` is the entity's whole lineage
  and the gate is exact. A non-first agent's is empty, so its crash window
  is FRESH rather than guessed — deliberate: `--continue` guesses the
  newest conversation in the cwd, and on a shared checkout that is
  precisely the misattribution being retired; one sweep tick after its
  first session, rule 1 carries it by name instead.
- ~~**Adoption keeps its pickup, on the flag adoption already sets.**~~
  **Retired 2026-08-31 — adoption inherits nothing.** The rule shipped with
  a second disjunct (`entity is adopted && no session lineage has ever
  opened on it`) so that an adopted branch's first agent opened on the
  human's own conversation. It is deleted: **Build cannot show that
  history.** Its conversation view for a new agent starts at sequence 1, so
  a session resumed under it is an agent answering messages that are nowhere
  on screen — and the derivation was not even stable, because the lineage it
  read lives on the roster's threads, so removing every agent from an old
  adopted branch made it read as freshly adopted again and granted the
  pickup a second time (observed live 2026-08-31). What remains of the arm
  is nothing: no `adopted` reading in the gate, no `pending_continuation`
  (the flag it was derived from the retirement of is deleted with it), and
  the `adopted` flag goes back to meaning what the rest of the daemon reads
  it for — prune rules, boot-recovery parking, the release verb.
- **Routers always spawn fresh**, by construction: no roster, no record,
  rule 3 — and rightly, since a router is one decision long and today's
  unconditional probe could hand it a stale conversation.
- Two comments are corrected where they stand: the reservation's "the probe
  is unconditional too" (`app.rs:17433`) describes rule 2's gate instead,
  and `SpawnOptions.continue_session`'s "Set only for the first session
  after adoption" (`orchestrator.rs:562`) becomes "a respawn of an agent
  with recorded history" (as amended by the retirement above).

#### 10.5 Wire, SPA, and the tests

**No wire change and no SPA change, confirmed by reading rather than
assumed.** `resume_session_id` appears on no payload: the agent digest
(`app.rs:8030`) carries id / ordinal / provider / model / effort / state /
unread / working / `has_terminal` / `can_interrupt` / created_at and gains
nothing; the record rides the `agents` table's serde JSON exactly as step 8
shipped it, no schema change; the `sessions` array a thread ships is
`SessionLineage`, untouched. Resume is invisible to the client on purpose —
it is the daemon respawning the same conversation, which a client
experiences as nothing having happened.

> **Step 10 shipped in `6d937fc`, `178085d`, `480d3a8` and `55e7f10`**, as
> designed above. Four things worth knowing, all of them places the code had
> to be more specific than the design.
>
> **The codex walk needed a bound that survives midnight.** The design says
> "only date directories on or after the construction date"; the code compares
> a directory's path relative to the sessions root (`2026/08/29`), which is
> zero-padded and so orders lexicographically the way the dates order — and it
> descends into a directory that is a PREFIX of the bound as well as one at or
> past it, or the walk would never reach the bound's own leaves. A session that
> opens at 23:59 and writes its rollout after midnight lands in a later dated
> directory, which is `>=` the bound and therefore walked.
>
> **A candidate with no readable name still counts as a candidate.** The
> locator's rule is "exactly one, or none", and a rollout that matched the cwd
> but yielded neither a header id nor a filename uuid is still one of this
> checkout's new conversations. Counting it keeps two such rollouts refusing
> rather than letting an unnamed one wave a named one through.
>
> **The pump-side capture and the sweep are literally one function.**
> `note_announced_conversation` became `note_named_conversation`: the activity
> pump, the byte pump's close arm and the sweep all call it, so the compare
> before the write and the "a name that has not arrived leaves the record
> alone" rule exist once. `capture_conversation_names` is the sweep half —
> collect under the lock, ask with it released, write back what moved.
>
> **One existing test changed meaning rather than being adjusted around.**
> `an_agent_tab_spawns_the_harness_the_orchestrator_built` spawns for an owner
> that holds no record at all — the shape a router has — and asserted the
> unconditional probe's `--continue`. Under rule 3 that owner inherits nothing,
> so the assertion is now `!continue_session`, which is the router case §10.4
> names. The revival test kept its subject by giving both its agents history,
> so the transcript probe is the only thing separating them.

The tests, written first, no real model turn, every transcript tree a
tempdir fake:

1. the claude locator answers the stem of the file that appeared after its
   snapshot; a pre-existing file is never a candidate;
2. two new files → `None`, indefinitely (never a guess); one file → cached,
   and a later second file does not change the answer;
3. the codex locator refuses a rollout whose header cwd is another checkout
   — two codex sessions at once, the misattribution case — and reads the id
   from the header of the one that matches;
4. `holds_conversation` per provider: true for the fake tree holding the
   id, false once the file is gone;
5. argv, per PTY harness, all three arms: `--resume <id>` with no
   `--continue` beside it; the probe-gated guess; neither — claude in flag
   shape, codex in subcommand shape with the `--config` overrides still
   ahead of it;
6. through the daemon: a PTY session whose injected locator resolves is
   captured by one sweep tick, and the next spawn's argv carries the name —
   §8.5's test 9, now walked by the PTY carrier;
7. through the daemon: a recorded id whose file is gone is cleared at the
   reservation and the spawn falls back — the poisoned-respawn case, now
   costing zero failed restarts;
8. the spawn rule, three ways: a verified id resumes; history plus a
   transcript continues; a fresh agent record over a cwd with an old
   transcript gets NEITHER — the 2026-08-29 misdelivery, pinned;
9. adoption: the adopted entity's first spawn gets NEITHER, and neither does
   an agent added back to an old adopted branch after every agent on it was
   removed (amended 2026-08-31 — these two pinned the pickup, and now pin
   its absence);
10. the alternatives hold shape: `session_locator` answers `Some` exactly
    where `has_terminal` is true, and `None` where the session announces
    its own id — asked over every provider, like the terminal test.

> **Live-observed 2026-08-29, first restart on the deployed step 10.** The
> reviving spawn took the spawn rule's middle arm exactly as designed (no
> recorded id, agent with history → `--continue`, own conversation), and it
> answered the first of the two open live questions: claude's TUI resumes
> **in place** — same session id, no new transcript file — so the locator
> correctly stayed `None` and the fallback stood, the benign outcome §10.2
> predicted for resume-in-place. Still open: whether a resumed codex appends
> to its old rollout or cuts a new one; the locator tolerates both, and the
> observed answer belongs here after the first real codex restart.

### Step 11 in detail — background tasks are visible, and the agent stays Working

Grounded in live probes (2026-08-29), not in guesses about the protocol: a
headless claude that starts background work announces it on the same stdout
stream, as `system` lines — `task_started`, `task_updated`,
`task_notification`, and `background_tasks_changed` carrying the set of live
tasks — all four observed live and recorded in the step-6 probe transcripts.
**Headless carrier only**: the conversation is the human's ONLY visibility
into a headless agent, while a PTY harness paints its background churn into
the terminal it already has — so `ClaudeHarness` and `CodexHarness` are
untouched and the reader below is `adk.rs`'s alone (`read_system`, which
today returns unless the subtype is `init`, grows the four arms).

The failure this step ends is the open headless-looks-idle finding: a
headless agent whose turn closed while background work ran reported
`Waiting` — the rail dot went dark over an agent mid-work, the digest's
`working` followed it, and nothing in the conversation said the work
existed.

**The payloads are pinned at implementation, not here.** The four subtypes
and the reading below are what the probes showed; the field names are not
normative. Before writing the reader, the implementer re-verifies the four
events against the installed CLI — its `--help` / stream-json docs, or a
no-model-turn probe — and records the verified lines into `adk::fake`, which
is what the tests then hold the reader to. If a shape differs from the
probes (a moved field, a delta where the probes read a roster), the rules
below stand and only the reading moves.

#### 11.1 The task set — reconciled, not bookkept

`ProtocolState` gains the live-task set: a map from the task's id to its
human-readable description. Events move it, **membership transitions mint**,
and `background_tasks_changed` is the source of truth.

- **`background_tasks_changed` replaces the set** with the roster it
  carries, every time. Not a merge: the event is the harness's own statement
  of what is live, and a reconciled set cannot drift from it — a task Build
  somehow never saw start is inserted, a task whose end never got its own
  event is removed. (Should the pinned payload turn out to carry a delta
  rather than the roster, the set applies the delta; the transition rule
  below is unchanged.)
- **`task_started` inserts its task** — the minting trigger for a start, and
  the reason status flips to `Working` without waiting for the next roster.
- **`task_updated` carrying a terminal status removes its task**; one that
  does not is progress, and touches membership not at all.
- **`task_notification` carrying a terminal status removes its task**, and
  one that does not never changes membership. (Amended 2026-08-30 under this
  section's own escape hatch — the rules stand, the reading moved. A
  FOREGROUND Bash command is a task too, and the live child closes it with a
  notification ALONE: no `task_updated`, no roster, ever. Taking every
  notification for chatter held that task for the life of the session and
  pinned the agent `Working` while it sat idle — the exact inverse of the
  failure this step closes.) Either way it is also the task saying something
  worth reading.

**Minting follows the transition, not the event name.** One row per
transition, however many events describe it:

- every insertion mints a started `TaskUpdate` — whether `task_started` or a
  roster the task first appeared in did the inserting;
- every removal mints an ended one — `failed` when the removing event said
  so, `finished` otherwise — so a roster that quietly drops a task still
  closes it in the timeline, and the timeline never shows work that started
  and never ends;
- a `task_started` followed by a roster listing the same task mints ONCE,
  because the second event moved nothing;
- `task_notification` mints its text, and then its ending row when the
  status it carried removed the task; a `task_updated` that changes neither
  membership nor carries new human-readable text mints nothing — a progress
  counter ticking is not a meaningful change. Text that only repeats the
  task's own name mints nothing either: a foreground notification's `summary`
  IS the description, and a row reading `X: X` says nothing the ending row
  did not.

**The summary lines**, in the shape the tool summaries set — one line,
clipped by `one_line` at `TOOL_SUMMARY_LIMIT`, because this is operational
text rather than the agent speaking: `started — <description>`,
`finished — <description>`, `failed — <description>: <error>`, and a
notification as `<description>: <text>`.

A `result` line does not touch the set: tasks outliving the turn is the
entire point. Nothing else clears it either — reconciliation, terminal task
events, and the session ending are the only exits.

#### 11.2 The fifth activity kind

Additive per §8, exactly as the four were: `AgentActivity::TaskUpdate
{ summary }` in `session.rs`, `ThreadEventKind::TaskUpdate` with wire token
`task_update`, class `Status`, intrinsic like the other four, and on
`ThreadEventKind::ALL` — so every rule tested over the roster of kinds (the
class split, the Issue mirror, the counted predicate) covers it with no new
code. It is minted by the reader above, rides the existing activity pump
(`record_agent_activity` gains the arm), and lands in the thread as an
ordinary `Status` row: no unread badge, no notification, and no slot bought
against §6.3's two bounds — `message = 0`, `attention = 0`, so it rides free
under the shipped predicate and **nothing about persistence changes**
(§6.3's paging already lets activity ride free).

#### 11.3 Working while tasks live

`AdkSession::status()` reports `Working` while a turn is open **or** the
reconciled set is non-empty — `live_status()` becomes `turn_open ||
!tasks.is_empty()`, then `Waiting` / `Starting` as today. The consequences,
each stated because each is a place the shipped code already decides
something:

- **The digest's `working` follows with no new code.** `agent_is_working`
  has read `status()` since step 2, so the rail dot keeps pulsing and the
  feed row's working clock keeps running over an agent whose turn closed
  with tasks live.
- **The idle sweep needs NO new sweep code — and a test proving it.** §11
  q4's demotion rule already short-circuits on `Working`, so a session
  holding a live task is never demoted however long it is quiet. The
  required test: a session that is quiet past the threshold, turn-closed,
  and holding a live task is not demoted — and the same session IS demoted
  once the roster empties and the clock runs out, which is what proves the
  set clears rather than pinning `Working` forever.
- **`can_interrupt` stays tied to an open turn.** The interrupt stops a
  TURN — §8.1's guard already answers a turn-less press with the satisfied
  no-op — and a background task is not one, so `can_interrupt()` becomes
  `capability && turn_open`, read under the same lock the guard reads.
  `working: true, can_interrupt: false` is therefore a **legal digest** — it
  always was: the PTY has shipped that pair since the field landed — and the
  shipped SPA already renders it as the plain Send, since its gate is
  `working && can_interrupt`. For every state a client could observe before
  this step the answer is unchanged: `Working` implied an open turn on this
  carrier, and the composer never offers the control to a non-working
  agent. §3's one-value equivalence narrows (amended there): a refusal still
  implies `false`, and §8.5 test 5's assertion becomes that direction plus
  "`false` with a turn open implies a refusal".
- **`Ended` still wins.** `status()` reads the exit code before the live
  state, so a child that exits with tasks open reports `Ended { code }`,
  roster notwithstanding — and the death rites are untouched: the activity
  stream closes with the child's stdout, the pump marks the tab not live
  and runs `record_agent_session_end`, and nothing waits on, drains, or
  mourns the tasks.

#### 11.4 SPA

`task_update` renders exactly as the other four kinds do — a folded, quiet
row, one entry added to `core/thread.js`'s activity map — with its own quiet
label, "Background task". Nothing else: no badge, no panel, no gating, and
every other kind, known or unknown, renders as it did.

> **Shipped.** The entry is `task_update: { label: "Background task", icon:
> "⧉", activity: true }` and there is no other change: the fold, the head's
> first-line preview, the fold-survives-the-repaint patch and the
> unknown-kind fallback are the shipped code, reached with no new branch.
> The label does NOT carry the harness's name — `activityHtml` swaps a
> leading `Agent` for the provider, and "Background task" has none to swap,
> which is the wanted answer: the row says what a task is doing, and
> "Claude Code background task" would say nothing more.

#### 11.5 The fake harness, and the tests

`adk::fake` grows recorded lines for the four events — a start, a terminal
update, a notification, a roster, and an empty roster — plus the FOREGROUND
pair a second probe turned up: a start and the terminal notification that is
the only word that task's ending ever gets. All recorded from the probes
rather than typed from this spec, under the module's standing single-quote
rule. Every test below runs against that child; none runs a model turn.

1. a `task_started` mints one started row and flips a turn-closed session to
   `Working`; the `background_tasks_changed` listing the same task mints
   nothing further;
2. reconciliation is the source of truth: a roster dropping a task the
   timeline saw started mints exactly one finished row, and with the set
   empty and no turn open the session reports `Waiting`;
3. a `task_updated` with a terminal status mints failed once; the roster
   that later omits the id mints nothing more;
4. a `task_notification`'s text is minted, clipped to one line; a
   `task_updated` changing neither membership nor text mints nothing; and a
   FOREGROUND task — a `task_started` closed by a terminal
   `task_notification`, with no roster and no `task_updated` ever arriving —
   mints its started and ended rows and lets the session report `Waiting`
   again, which is the fence against the pin that reading cost;
5. through the daemon: the idle sweep leaves a quiet, turn-closed session
   holding a live task alone (quiet clock aged past the threshold, no new
   sweep code), and demotes the same session once its roster empties;
6. the digest reads `working: true, can_interrupt: false` off a tasks-only
   session — the legal pair, pinned;
7. `Ended` wins: a child that exits with tasks open reports `Ended` and the
   death rites run exactly as they do today;
8. in the SPA: `task_update` folds shut with its own label, and every other
   kind renders untouched.

### Step 12 in detail — a tool call and its answer are one row

*Specified 2026-08-30.* The reader already pairs every call to its answer:
`ProtocolReader.calls` (`adk.rs:670`) maps the protocol's `tool_use` id to
what became of the call, because a `tool_result` names the call it answers by
id and nothing else. Then the pairing is thrown away at the last moment — the
call and the answer are minted as two unrelated rows, and the human reads the
join the reader already computed by eyeballing adjacent lines. Step 12 carries
the pairing outward: **one thread row per tool call, minted at the call,
updated in place when its answer arrives.** Headless carrier only, like step
11 — the PTY reports no activity, so nothing else moves.

#### 12.1 Events gain mutation

`ThreadEvent` joins `ThreadMessage`'s `updated_sequence` machinery, additively:

```rust
pub struct ThreadEvent {
    // … id, sequence …
    /// Drawn from the same counter as `sequence` and bumped when the event
    /// mutates in place — today that is a tool call's answer arriving — so
    /// the cursor protocol re-ships the newer copy of an already-held row.
    /// Defaults to 0 (never mutated) on every record persisted before this
    /// field, which leaves old rows untouched: `latest_sequence` is a max.
    #[serde(default)]
    pub updated_sequence: u64,
    // …
}
```

**One arm changes, and everything else inherits it.** The paths were read,
and this is the list:

- `ThreadItem::latest_sequence` (`thread.rs:919`) — the Event arm becomes
  `event.sequence.max(event.updated_sequence)`, mirroring the Message arm,
  and its doc line "Events never mutate in place" dies. Every consumer below
  consults the event's bump through this one reading:
  - `resident_after` / `wire_value_after` (`thread.rs:2301/2272`) — the
    forward delta re-ships the updated row to a client whose cursor is past
    its creation, exactly as it re-ships a message marked seen;
  - `wire_value_after_including_history` (`thread.rs:2281`) — the store-row
    admission filter, same reading;
  - `Thread::last_sequence` (`thread.rs:2012`) — the cursor high-water mark:
    the bump moves `thread_last_sequence` on the wire, which is what lets the
    SPA's delivered-equals-newest check pass once the delta lands;
  - the load's `next_sequence` repair (`thread.rs:1397`) — already a max over
    `latest_sequence`, so a reload clears event bumps too and no counter
    value is ever spent twice;
  - the store's `write_agents` dirty check (`store.rs:992–1004`) — compares
    stored `(sequence, updated_sequence)` against `item.latest_sequence()`,
    so the updated row is rewritten and its `updated_sequence` **column**
    (`store.rs:422` — it has existed since the store landed, written for
    every item, events included) moves with it; `THREAD_CURSOR_SQL`
    (`store.rs:523`) and `THREAD_LAST_SEQUENCE_SQL` (`store.rs:534`) then
    serve the mutation with **no SQL change**.
- **Paths that must NOT consult it, held by their own docs:** `unread_since`,
  `unread_attention_below` and `last_attention_sequence`
  (`thread.rs:2160/2185/2205`) read creation sequence deliberately — a
  mutation bump is not the conversation speaking again — and a tool row is
  `Status` on both sides of its update anyway.
- **Hashing and deduping: read for, and there is none.** Nothing in the
  bridge hashes or dedups events; the one equality-adjacent read is the
  has-this-event-kind match (`event.event == kind`, `app.rs:26494`), which a
  bump does not touch. On the client, the cursor already merges mutations by
  latest sequence generically: `itemCursorSequence`
  (`core/thread.js:169–174`) reads `updated_sequence || 0` off **every**
  item, `mergeArrivals` replaces the held copy keyed by creation sequence,
  and `theWindowMayTake` keeps a mutation from under the window's floor out
  of it — all three shipped for messages and written over items, so the
  client cursor needs **no change**.

The bump is spent through `Thread::next()` the way `resolve_doc_comment`'s is
(`thread.rs:1761`). Two small additive changes carry it: `push_event` returns
the minted sequence (callers that ignore it stand unmodified), and a new
`Thread::resolve_tool_call(sequence, outcome, answer)` finds the `ToolUse`
row by creation sequence, appends the answer to its summary, sets its
outcome, and bumps `updated_sequence` — returning `false` for a row that is
not resident, so the caller can fall back to minting (below).

#### 12.2 The pairing carried outward

**The activity variants.** `AgentActivity::ToolUse` gains the protocol's call
id; `ToolResult` stops being a row of its own and becomes the completion
signal for the call with the same id:

```rust
/// The agent called a tool. `call_id` is the protocol's own id for the call
/// (`tool_use.id`) — the name its answer will arrive under.
ToolUse { call_id: String, summary: String },
/// A tool answered: the completion signal for the `ToolUse` carrying the
/// same id. `summary` is the one-line answer text, possibly empty.
ToolResult { call_id: String, outcome: ToolOutcome, summary: String },

pub enum ToolOutcome { Ok, Error, Unanswered }
```

The reader's own half barely moves: `read_tool_use` (`adk.rs:956`) emits the
id it already holds; `read_tool_result` (`adk.rs:968`) emits the completion
instead of a summary row — outcome `Error` when the `tool_result` block's
`is_error` is true (re-verified against the installed CLI and recorded into
`adk::fake` at implementation, under §11's escape hatch), `Ok` otherwise,
answer text `one_line`-clipped as today. Build's own MCP calls stay silent at
both ends, exactly as now.

**The stored outcome.** `ThreadEvent` gains a second additive field, absent
on every event that is not a completed tool call — the thread's own mirror of
the harness enum, in the manner of the kind mapping:

```rust
/// What a tool call's answer reported, on the `ToolUse` row it completes.
#[serde(default, skip_serializing_if = "Option::is_none")]
pub outcome: Option<ToolCallOutcome>,   // wire: "ok" | "error" | "unanswered"
```

**The pump keeps the open-call map.** `spawn_activity_pump` (`app.rs:17992`)
holds `open_calls: HashMap<String, u64>` — call id → the minted row's
creation sequence — as task-local state, so it is per session by
construction: a new session is a new pump with an empty map. The call id
never reaches the thread; the pairing lives and dies with the session.

- On `ToolUse`: mint as today (`record_agent_activity` hands back the
  sequence `push_event` now returns), insert into the map, outcome absent —
  the row **is** the pending state.
- On `ToolResult` whose id the map holds: remove it and update the row in
  place through `resolve_tool_call` — the summary gains the answer as a
  suffix line, `\n→ <answer>` (omitted when the answer is empty; the outcome
  field carries the state either way, and a legacy client just reads a
  slightly longer summary), the outcome field is set, and **no `tool_result`
  row is minted**.
- On `ToolResult` whose id the map does not hold — a `ToolUse` lost to
  broadcast lag, or a row the resident tail no longer has — it falls back to
  minting a standalone `tool_result` row exactly as today. The `tool_result`
  **kind stays**: stored rows must render forever, and the orphan fallback
  keeps a producer alive for the degraded path. New sessions on the happy
  path simply stop producing it.

#### 12.3 Unanswered calls close honestly

A row must never pretend an answer arrived — and it must not dangle as
pending forever either, because pending is a claim too ("this is still
running"). So an unanswered call **closes by update**: outcome `unanswered` —
a terminal state distinct from ok and error — with the suffix naming what
ended it. Two closers:

- **A `result` line ends the turn, and the reader drains its map.**
  `read_result` (`adk.rs:901`) already owns the boundary; it grows a drain:
  every `Minted` entry still in `calls` emits
  `ToolResult { call_id, outcome: Unanswered, summary: "" }`, and
  `BuildsOwn` entries drop silently, as their answers always did. The pump
  words the update `→ no answer — turn ended`. On a normal turn the protocol
  answers every call before its result, so the drain finds the map empty;
  the interrupt's `error_during_execution` was expected to be the case with
  leftovers — but the live leg showed otherwise (2026-08-30, claude 2.1.x):
  the CLI answers the interrupted call itself, with an `is_error` rejection
  ("The user doesn't want to proceed…"), before the result, so the
  interrupted call closes as `error` and the drain finds the map empty
  there too. The drain stays as the net beneath a wire that does not answer
  — a crashed child, an older CLI — pinned by the fake. The
  map is therefore **one map drained at every turn boundary**, not a
  per-turn structure — the next turn starts against an empty map by
  construction, on both sides: the reader's drain empties `calls`, and the
  emitted completions empty the pump's `open_calls`.
- **The death rites close the rest.** On the activity stream closing
  (`app.rs:18021`), the pump closes every entry still in `open_calls` —
  a crashed child's calls, plus any completion broadcast lag swallowed —
  as `→ no answer — session ended`, **before** `record_agent_session_end`,
  so the timeline reads calls-closed-then-session-ended rather than a
  session ending over calls that still claim to run.

A daemon crash leaves its open rows pending with no marker, and that is the
honest answer: no answer ever arrived, nothing is fabricated afterward, and
the session-ended row the next boot's lineage carries sits beneath them
saying why.

#### 12.4 Volume, and §6.3

A tool-heavy session mints one row per call instead of two — roughly half its
activity rows. The counted predicate is untouched by construction and pinned
by test: a `ToolUse` row is `Status` with `message = 0, attention = 0` when
minted and after every update (the store upsert rewrites the same column
values), so completion flips neither `counted()` nor `attention_reason()`,
buys no slot against either §6.3 bound, moves no unread count, and pulls
nobody in.

#### 12.5 SPA

The `tool_use` fold renders one line with three states, in the shipped fold
idiom (`activityHtml`, `core/thread.js:726`) — the head keeps its icon, its
label and the call's first line as preview, and gains a trailing state mark:

- **pending** — no `outcome` field: today's row exactly, no mark. Absence is
  the pending state, which is also what every legacy row and every legacy
  daemon produces.
- **ok** — a dim `✓` on the head; the answer lives in the fold body, where
  the full summary (call, then `→ answer`) already renders.
- **error** — `✕` in the blocked color **on the mark alone**: the row stays
  toneless, because activity asks the reader for nothing and a failed tool
  call still doesn't — the agent deals with it, and the agent calling the
  human is what `Blocked` is for.
- **unanswered** — `⊘`, with the `no answer — …` line in the body saying
  which boundary closed it.

An unknown outcome token renders as pending (no mark), the additive-wire
discipline read in the client's direction. Legacy `tool_result` rows keep
their `EVENT_META` entry and render exactly as today. The preview is the
summary's **first line**, so the head stays stable when the answer suffix
lands; the fold-survives-the-repaint patch already covers a repaint under an
open fold, and a test holds it across an outcome arriving specifically.

#### 12.6 The fake harness, and the tests

`adk::fake` grows the recordings the tests need — probe-recorded against the
installed CLI, never typed from this spec, under the module's single-quote
rule: an error `tool_result` (`is_error: true`), a second call pair with a
distinct id (for out-of-order interleave), and a `tool_result` naming an id
no call announced. The interrupt leg reuses the recorded `TOOL_USE` +
`FAILED_RESULT` pair it already has. Every test runs against the fake; none
runs a model turn.

1. the happy pair: a `tool_use` then its `tool_result` puts ONE row in the
   thread — kind `tool_use`, summary `call\n→ answer`, outcome `ok`,
   `updated_sequence > sequence` — and no `tool_result` row;
2. an error answer updates the same row with outcome `error` and the block's
   text;
3. two calls answered out of order each land on their own row — the id
   pairs, not adjacency;
4. a Build-own MCP call stays unminted through both halves of the pairing;
5. an orphan `tool_result` mints a standalone `tool_result` row exactly as
   today — the kind's producer of last resort;
6. the interrupt: a call left open by an `error_during_execution` result
   closes as `→ no answer — turn ended`, outcome `unanswered`, and the next
   turn's call pairs into a fresh row, proving the drain emptied both maps;
7. the death rites: a stream closing over an open call closes it as
   `→ no answer — session ended` before `record_agent_session_end`, and the
   rites otherwise run unchanged;
8. the cursor re-ships: `wire_value_after` with a cursor past the row's
   creation carries the updated row — the event mirror of
   `wire_value_after_reships_a_message_marked_seen_after_the_cursor`;
9. persistence: `write_agents` rewrites the bumped row and no other,
   `thread_items_after` re-serves it, and a reload's `next_sequence` clears
   the bump so no counter value is spent twice;
10. compatibility: a stored event without the two fields loads with
    `updated_sequence` 0 and `outcome` `None`, `latest_sequence ==
    sequence`, and the record round-trips byte-identical — old rows are
    untouched by machinery they predate;
11. §6.3 invariance: `counted()`, `attention_reason()` and the two hoisted
    columns are identical before and after an outcome lands, and the unread
    count is unmoved;
12. in the SPA: the three states render off `outcome`, `unanswered` shows
    its no-answer line, a legacy `tool_result` row renders as today, an
    unknown outcome token renders as pending, and a fold the reader opened
    survives the outcome landing under it.

### Step 13 in detail — the account chooses Claude Code's carrier

*Specified 2026-08-30.* Since step 6 the SPA has offered three start cards —
"Claude Code", "Claude Code (headless)", "Codex" — which puts a carrier
question in front of every start and the word "headless" in front of every
human. Step 13 removes both. **An account setting chooses Claude Code's mode;
creating a chat offers only "Claude Code" or "Codex", and the setting decides
which carrier "Claude Code" means.** The default mode is headless and is
labeled just "Claude Code" — the word "headless" appears in no user-facing
string, anywhere, after this step. The other option is "Claude Code TUI".
Codex gets the same setting field wired but hard-locked to TUI, because no
codex headless exists. And *Claude is Claude*: every bubble and label for both
claude carriers reads "Claude Code" — the account-level setting is what
guarantees TUI and headless agents never sit side by side needing to be told
apart.

#### 13.1 The setting

Settings already live on the bridge: `settings.get` / `settings.set`
(`app.rs:6461/6467`) serve `projects_dir`, and `persist()` (`app.rs:3529`)
writes it to the config file beside `router_model` and the project list. The
mode joins that file, additively:

```json
// settings.get — the full answer after this step
{ "projects_dir": "/Users/…/Projects",
  "claude_mode": "headless",        // or "tui"; absent config = "headless"
  "codex_mode":  "tui" }            // always; not stored, synthesized
```

`ClaudeMode` is an enum beside `AgentProvider` in `models.rs` —
`{ #[default] Headless, Tui }`, serde `lowercase` — with one method,
`carrier()`: `Headless → ClaudeAdk`, `Tui → Claude`. `AppState` holds a
`claude_mode: ClaudeMode`, loaded from the config key (absent = default,
which IS the stated default: headless), written back by `persist()`.
`codex_mode` is not a field at all — it is a hard-locked wire answer, always
`"tui"`, so there is nothing to migrate when a codex headless someday exists:
the lock comes off and the field starts being stored.

`settings.set` becomes field-wise: each known field is optional and only the
ones present are applied — `projects_dir` exactly as today when named, so an
old client sending only it is byte-identical; `claude_mode` accepting
`"headless"` or `"tui"` and refusing anything else with
`unknown claude_mode {value:?} (expected "headless" or "tui")` (the wire
tokens are machine vocabulary — the settings page's select can only send
valid ones, so no human is shown this sentence with "headless" in it);
`codex_mode` accepting only `"tui"`, refused otherwise with exactly:
**`codex_mode accepts only "tui" — Codex has no other mode yet`**. A set
naming no known field is refused (`settings.set: nothing to set`). Every
accepted set persists and returns the full `settings_get()` answer, as today.

#### 13.2 Resolution at spawn — where "claude" becomes a carrier

Every persisted `ModelChoice` is minted at ONE chokepoint:
`model_choice_from(params)` (`app.rs:14796`), the free function all nine
choice-minting sites call — `agent.add` (7990), `plan.create` (9357),
`run.create` (10495), `run_create_in_worktree` (10566), `run_stage_dispatch`
(11568), `run.adopt` (12184), the branch-dispatch validate (13179) and its
agent-add (13249), and `agent_start` (17336). Resolution lives there and
nowhere else: the function gains a parameter,
`model_choice_from(params, claude_means: AgentProvider)`, where
`claude_means = self.claude_mode.carrier()` (a one-line
`AppState::claude_carrier()`), and its provider match becomes:

- **absent / empty / `"claude"`** → `claude_means`. The generic token and
  the no-preference silence mean the same thing — "Claude Code" — and the
  setting is what Claude Code means.
- **`"claude_adk"`** → `ClaudeAdk`, honored as-is. A client that names the
  concrete carrier gets the concrete carrier, whatever the setting says.
- **`"codex"`** → `Codex`, always — the hard lock, restated where it bites.

Eight of the nine sites are `AppState` methods and pass `self`'s answer;
`agent_start` parses before taking the lock today, so its parse moves under
the lock it already takes — the "parsed before anything is touched" comment
survives, since the parse still precedes every mutation. Two neighbors
resolve through the same rule: `default_agent_provider` (`app.rs:8804`, the
router's carrier) returns `self.claude_carrier()` instead of
`AgentProvider::default()` — the router is a headless-shaped job and "the
default provider" now means the setting's answer — and nothing else in the
daemon interprets the token (`from_wire`'s only other caller,
`require_shell_kind` at `app.rs:104`, asks *is this any provider* and does
not care which).

**What never resolves: persisted records.** `AgentProvider`'s serde
`#[default]` stays `Claude`, and
`old_choices_without_a_provider_deserialize_as_claude` keeps pinning it — a
record written before the provider field was TUI and stays TUI. Resolution
is wire-parse-time only, never deserialize-time, so existing entities never
migrate and every resume path stays concrete: the respawn/nudge paths read
`entity_model_choice` (`app.rs:3707`) or the roster's own persisted choice
and never pass through `model_choice_from`.

**The wire consequence, owned honestly.** The fields are additive — no token
is removed, no shape changes — but the token `"claude"` (and an absent
provider) changes MEANING at start-time: it used to name the TUI carrier and
now names whatever the account's `claude_mode` says, which is headless by
default. A fresh bridge that used to start a TUI starts headless; an OLD
client sending `"claude"` gets the setting's answer, which is the intended
new meaning, not a compatibility accident. One visible edge: `agent.start`
naming `"claude"` on an idle entity whose persisted choice is the other
claude carrier re-carriers it through `set_entity_model_choice`
(`app.rs:3749`) — that is the setting having its say at the only moment it
may, and the existing while-live refusal stands unchanged. `models.list`'s
`default_provider` keeps serving `"claude"`: the generic token is now the
honest answer, since what it starts is the setting's business.

#### 13.3 Claude is Claude — the labels

The word "headless" survives in exactly two user-reachable strings today, and
both die:

- **`AdkHarness::label()`** (`harness/adk.rs:59`) becomes `"Claude Code"`.
  That one change fixes the `models.list` catalog (`provider_catalogs` reads
  `harness.label()`) and the carrier-switch refusal in
  `set_entity_model_choice`, which prints labels. The catalog **keeps all
  three concrete providers** — it is machine truth: an entity persisted on
  `claude_adk` still needs its models/efforts served under its own id, and
  removing an entry would break old clients — but two of its entries now
  share the label "Claude Code", which only an old client's defaults select
  ever renders side by side (functional, merely duplicate-labeled; a new
  client never shows the third entry at all). No new wire field: the SPA
  already ships its own startable list, so a `startable` flag on the catalog
  would be a second copy of a decision the client owns.
- **`STARTABLE_PROVIDERS`** (`spa/src/core/modelPicker.js:10`) drops the
  `claude_adk` entry: exactly two cards, `claude` "Claude Code" and `codex`
  "Codex". `DEFAULT_START_PROVIDER` stays `"claude"`.

With the entry gone, every SPA read of a concrete `claude_adk` token must
alias rather than echo. One helper in `modelPicker.js` —
`genericProviderId(id)`: `claude_adk → claude`, else identity — used by:

- `providerLabel` (`agentRailModel.js:23`): alias before the lookup, so a
  headless agent's bubble reads "Claude Code 1", never the wire token;
- `harnessLabel` (`core/thread.js:483`): `claude_adk` joins the `claude` arm
  → "Claude Code";
- the Agent tab's lead card (`surfaceTabs.js:176/183`): a worktree whose
  last session was `claude_adk` leads with the generic `claude` card labeled
  "Claude Code" — and *starts* generic, because a restart is a start and the
  setting decides what Claude Code opens (without the alias the current
  `.find(...).label` would throw on the unknown id);
- `loadAgentDefaults` (`core/agentDefaults.js`): a stored provider of
  `claude_adk` from the three-card era loads as `claude`;
- the Account page's defaults select (`views/settings.js:135`): the catalog
  it paints is filtered to ids in `STARTABLE_PROVIDERS`, so the third entry
  never reaches a picker.

#### 13.4 The Account page

The Settings page has two control idioms: `projects_dir` renders as a code
line with a "Change…" button opening the browse sheet — the idiom for a
filesystem path — and the Agent defaults panel renders `<select>`s in
`field-row`/`field` markup, saved on every change with a "Saved." note,
because "a preference with a commit step is a preference people forget to
commit". A two-value mode is the second kind. A new panel between Agent
defaults and Appearance:

```
🚂 How agents run
   dim note: "Which program each agent opens as. Saved on your bridge, for
   every device."
   field "Claude Code" — <select>: "Claude Code" (value headless, default)
                                   / "Claude Code TUI" (value tui)
   field "Codex"       — <select disabled>: "Codex TUI"
                         dim note: "the only mode Codex has yet"
   dim #modesaved note, "Saved." after each successful set
```

Mounted like `mountAgentDefaults`: `settings.get` fills the select (absent
`claude_mode` on an old bridge reads as headless — the default — and the
panel still renders), `onchange` calls
`App.call("settings.set", { claude_mode: value })`, a refusal lands in the
panel's error line, and unlike the defaults panel this one round-trips the
bridge — it is an account setting, not a browser-local one, which is why it
does not live inside the Agent defaults panel.

#### 13.5 The tests

Bridge (`cargo test`, no model turns — everything drives `AppState::handle`
or the fake harness):

1. `settings.get` on a fresh state answers `claude_mode` `"headless"` and
   `codex_mode` `"tui"`;
2. `settings.set { claude_mode: "tui" }` persists: a state reloaded from the
   same config file answers `"tui"`, and `projects_dir` is unmoved;
3. an unknown `claude_mode` is refused with the expected-values sentence and
   changes nothing;
4. `settings.set { codex_mode: "tui" }` passes (idempotent); any other value
   is refused with exactly the no-other-mode sentence;
5. old-client shape: a set naming only `projects_dir` behaves byte-identically
   to today; a set naming no known field is refused;
6. `plan.create` naming `"claude"` under the default setting persists a
   `ModelChoice` whose provider is `ClaudeAdk`; under `claude_mode: "tui"`,
   `Claude`; with the provider absent, the same two answers;
7. `"claude_adk"` named concretely is honored regardless of the setting, and
   `"codex"` is `Codex` regardless of the setting;
8. changing the setting migrates no existing entity: `entity_model_choice`
   before and after a `settings.set` is identical, and a respawn spends the
   persisted concrete provider;
9. `agent.start` naming `"claude"` on an idle TUI entity re-carriers it to
   the setting's answer, and the while-live refusal still refuses;
10. no user-facing string says "headless": every `provider_catalogs()` label
    and the switch-refusal message are asserted clean;
11. the router: `default_agent_provider` follows the setting.

SPA (`npm test`):

1. `STARTABLE_PROVIDERS` is exactly `claude` + `codex`, no label containing
   "headless";
2. `providerLabel("claude_adk")` and `harnessLabel` on a `claude_adk` session
   both read "Claude Code";
3. the lead card for a worktree that last ran `claude_adk` is labeled
   "Claude Code" and starts provider `"claude"`;
4. `loadAgentDefaults` normalizes a stored `claude_adk` to `claude`, and the
   defaults select renders only startable providers from a three-entry
   catalog;
5. the modes panel: `settings.get` paints the selection, a change calls
   `settings.set` with the chosen `claude_mode` and shows "Saved.", an
   absent `claude_mode` renders as the default, and the Codex control is
   disabled.

### Step 14 in detail — agents lock to their harness, and branches start with none

*Specified 2026-08-30. Branch `build/new-agent-flow`.* Step 13 shipped the
account setting and then taught the SPA to treat the two claude carriers as one
agent — a restart sent the generic token and the setting chose the carrier
again. Living with it showed the cost: a TUI agent restarted under a headless
setting is a NEW process on a different carrier wearing the same bubble, and
whether its chat history survives depends on a resume probe the human never
sees. Step 14 reverses that half and keeps the good half. **An agent is locked
to the harness it was created on, forever — the harness is part of the agent's
identity, so its conversation provably stays its own. The account setting stops
choosing a carrier at every start and instead names the DEFAULT harness a new
agent is created on. Branches start with no agents at all; the first send
creates one, and the system creates one itself only when it must deliver.**

Three of step 13's decisions are superseded, by name:

- **Carrier-follows-setting is dead.** The generic token `"claude"` no longer
  resolves through the setting; nothing re-carriers an entity at start time;
  the restart-is-a-start lead card that "starts generic" is gone. A restart
  respawns exactly the harness the agent is locked to.
- **The label fold is reversed.** Two carriers under one name made sense only
  while the account hid one of them. With both creatable side by side they
  need names: **"Claude Code"** is the headless carrier (`claude_adk`),
  **"Claude Code TUI"** is the terminal one (`claude`), **"Codex"** is codex.
  The word "headless" stays banned from every user-facing string — the default
  carrier simply owns the plain name.
- **`claude_mode` is replaced by `default_harness`**, with the old keys kept
  as compat aliases (§14.1).

Unmoved from step 13: the setting lives on the bridge, persisted in the config
file, served by `settings.get`/field-wise `settings.set`; persisted choices are
concrete and never migrate; resolution of wire tokens happens at
`model_choice_from` and nowhere else.

#### 14.1 The setting: one default harness

`AppState.claude_mode` (`app.rs:1844`) becomes `default_harness:
AgentProvider`, default `ClaudeAdk`. The wire value is a concrete provider
token — the same vocabulary every other provider field speaks, so there is no
second enum to keep in sync:

```json
// settings.get — the full answer after this step
{ "projects_dir": "/Users/…/Projects",
  "default_harness": "claude_adk",   // or "claude" / "codex"; absent = claude_adk
  "claude_mode": "headless",         // compat alias, derived: "tui" iff default_harness = claude
  "codex_mode":  "tui" }             // compat alias, synthesized as before
```

`settings.set` stays field-wise and gains `default_harness`, validated through
`AgentProvider::from_wire` and refused with
`unknown default_harness {value:?} (expected "claude_adk", "claude" or
"codex")`. The compat story is read-and-write, chosen because it is the
smallest thing that keeps step-13 clients whole:

- **`settings.set { claude_mode }` keeps working**: `"headless"` maps to
  `default_harness = ClaudeAdk`, `"tui"` to `Claude` — the exact
  `ClaudeMode::carrier()` mapping (`models.rs:93`), which survives as one
  small function beside `CODEX_ONLY_MODE` after the `ClaudeMode` enum and its
  `AppState` field are deleted. `codex_mode` keeps its idempotent accept and
  its exact refusal sentence.
- **`settings.get` keeps serving both old keys**, derived from
  `default_harness` (a codex default answers `claude_mode: "headless"`, which
  is the old default and the honest "not tui"). An old Account page still
  paints and still saves.
- **The config file** mints `default_harness` and `persist()` (`app.rs:3550`)
  writes only it. Loading reads `default_harness` first and falls back to a
  stored `claude_mode` through the same mapping — a bridge upgraded in place
  keeps its choice with no migration step, and the old key is simply never
  written again.

**The Account page shows ONE select.** `agentModePanelHtml` /
`mountAgentMode` (`core/agentMode.js`) collapse to a single field — label
"Default agent", options rendered from **`STARTABLE_PROVIDERS`** via the
existing `providerOptionsHtml`, so the select and the new-agent view's cards
share one options array and cannot drift. The Codex select, its lock note,
`CLAUDE_MODES` and `CODEX_MODES` are deleted. The panel keeps its
paint-from-`settings.get`, save-on-change, "Saved." idiom; it saves
`{ default_harness: value }` and falls back to `{ claude_mode }` only if the
bridge refuses the new key (an old bridge under a new client — the two claude
options still work there; a chosen Codex default is refused with the bridge's
sentence, honestly).

#### 14.2 Locked agents — resolution without a mode

`model_choice_from` (`app.rs:14865`) keeps its parameter and changes what it
means: `model_choice_from(params, default: AgentProvider)` where the callers
pass `self.default_harness`. The match becomes:

- **absent / empty** → `default` — no preference means the account's default
  harness;
- **any named token** → `AgentProvider::from_wire` — `"claude"` is the TUI
  carrier again, concretely; `"claude_adk"` and `"codex"` as themselves.

`claude_carrier()` (`app.rs:8867`) is deleted with the enum. Its nine callers
pass `self.default_harness`. `default_agent_provider` (`app.rs:8861`, the
router's carrier) stops following the setting and pins
`AgentProvider::ClaudeAdk`: the router is a headless-shaped job and there is
exactly one headless carrier — a Codex default must not strand routing on a
TUI. `agent_start`'s re-carriering edge from step 13 (naming `"claude"` on an
idle entity re-carriers it) dies by construction: every token is concrete, so
`set_entity_model_choice` only ever sees a provider move the human explicitly
named, and its while-live refusal narrows to exactly that case (§14.6).

**Labels.** `ClaudeHarness::label()` (`harness/claude.rs:34`) becomes
`"Claude Code TUI"`; `AdkHarness::label()` (`harness/adk.rs:63`) stays
`"Claude Code"`; `CodexHarness::label()` (`harness/codex.rs:31`) becomes
`"Codex"` — one vocabulary, three names, and the settings select, the
new-agent cards, the rail bubbles and every refusal that prints a label all
read them from the same place (`provider_catalogs` / `AgentProvider::label`).
The step-13 test that asserted both claude labels equal is inverted: every
catalog label is asserted **distinct**, and still free of "headless".

**SPA vocabulary.** In `core/modelPicker.js`: `STARTABLE_PROVIDERS` becomes
three entries — `claude_adk` "Claude Code", `claude` "Claude Code TUI",
`codex` "Codex" — and `PROVIDER_LABELS` matches. `CARRIED_AGENTS`,
`genericProviderId`, `oneProviderPerName` and `defaultAmong` are deleted:
every carrier is an agent now, so there is nothing to alias or fold, and
`startableCatalogProviders` reduces to an id filter against
`STARTABLE_PROVIDERS`. `DEFAULT_START_PROVIDER` dies too — what a start leads
with is the account's answer, read off `models.list`, whose
`default_provider` (`app.rs:5732`) starts serving `self.default_harness` (and
whose top-level `models`/`efforts` compat fields serve that default's
catalog). `loadAgentDefaults` (`core/agentDefaults.js`) stops aliasing a
stored `claude_adk` — it is a first-class preference again. Every deleted
helper's call sites (`agentRailModel.js:23` `providerLabel` import path,
`thread.js:483` `harnessLabel`, `surfaceTabs.js`, `views/settings.js`) read
the concrete id straight.

#### 14.3 Agentless branches, and the primary at index 0

**Creation mints no agent.** Of the three `AgentRoster::with_first` sites
(`orchestrator.rs:913`, `1483`, `2442`):

- **`create_plan` (913) is untouched.** An issue carries exactly one agent
  and its agent IS its conversation; nothing in this step is about issues.
- **`adopt_run` (2442) mints an empty roster.** Adoption is git and records;
  no one is being spoken to. The `pending_continuation` grant and the
  adoption-time choice params stop minting an agent and instead wait for the
  first one. *(The pickup that grant stood for is retired outright as of
  2026-08-31 — see §10.4 — and `pending_continuation` is deleted with it: an
  adopted branch's first agent starts a conversation of its own.)*
- **`dispatch_run` (1483) keeps minting one**, with the dispatch's own
  choice: a dispatched build is case (b) below — the system is about to
  deliver, and minting at dispatch IS the auto-add, done where the choice is
  in hand.

**The roster may be empty, and index 0 is the primary.** `AgentRoster` drops
its non-empty invariant: `first()`/`first_mut()` and the `Deref`/`DerefMut`
to the first agent's thread (`agent.rs:118–189`) are deleted — a `Deref` that
can panic is a trap every new call site walks into. In their place:

- `primary()` / `primary_mut()` → `Option<&Agent>` — the agent at index 0;
- `resolve(None)` → the primary, or a refusal
  (`no agent on {owner} — send a message to create one`) for read paths;
- `is_empty()` becomes the real answer.

Issues always construct `with_first`, so plan paths unwrap `primary()` with
an expect naming the invariant that still holds there
(`"an issue always holds its one agent"`).

**The auto-add door is one helper.** `AppState::ensure_primary_agent(entity_id)
-> Result<agent_id>`: if the roster is empty, add an agent with
`ModelChoice { provider: self.default_harness, ..Default::default() }` (falling
back to the entity's persisted choice when one names a provider), persist, and
return index 0's id. Every path where the system must deliver calls it instead
of reaching for `first()`:

| Site | Today | Change |
|---|---|---|
| `thread_post` (`app.rs:10946`) | `roster.resolve(addressed)` | no `agent_id` + empty roster → `ensure_primary_agent` (a post must be heard) |
| `agent_start` (`app.rs:17420`) | `resolve_agent` | same door — a start with no agent creates the default one |
| `PendingAgentTurn::for_run` (`app.rs:900`) | `agents.first()` | callers pass the id from `ensure_primary_agent` / the primary |
| `for_recovery` (`app.rs:959`) | `agents.first()` | `primary()` — a recovery only exists for an entity that ran, but an empty roster refuses rather than panics |
| the router's branch delivery (`default_agent_provider` caller, `app.rs:8815`) | roster's first | `ensure_primary_agent` on the routed-to entity |
| `run.message` (`app.rs:12040`) / `run_stage_fix`-family posts | Deref `post_user` | `ensure_primary_agent`, then post to it |
| `run_request_changes` (fn at `app.rs:11553`) | `resolve(addressed)` (`11567`) + `first()` compare (`11568`) | `resolve` unchanged; the `addresses_first_agent` compare reads `primary()` |
| branch dispatch (`app.rs:13318`) | `first()` or `add` | empty roster takes the `add` arm — same code, one less special case |
| the idle sweep (`app.rs:6161/6174`) | tab of `agents.first()` | skip entities whose roster is empty (nothing can be idle that does not exist) |
| `issue_session` (fn at `app.rs:16522`, the free fn read before a session-ending verb) | `active.agents.first()` (`16524`) | `primary()` with the issue expect — issues always have one. (`retire_issue_session`, `app.rs:8161`, only delegates to `retire_agent` and needs nothing.) |
| `offering_thread` (fn at `app.rs:10927`) | the ISSUE roster's `first().thread` (`10941`) | `primary()` with the issue expect; the run arm's `resolve` is unchanged |
| `thread_post`'s `ImplementationTarget` (fn at `app.rs:10946`) | the implementation run's `first().id` / `.choice` (`11015–11016`) | `primary()` — an empty implementation roster yields no target, so the Issue's post stays on the Issue |
| `close_turn_of_dead_agent` (fn at `app.rs:3013`) | `active.agents.first().id == agent_id` (`3037`) | `primary().is_some_and(…)` — false on an empty roster, so the death is recorded on the agent's own thread |

**Entity-level events on an empty roster are not minted.** The Deref sites
that push lifecycle/git events onto "the" thread (`app.rs:2344`, `10229`,
`10283`, `10392`, `11951`, `12091`, and the doc-comment/user-post family) go
through `primary_mut()` and skip when there is no one to tell. Owned
honestly: an agentless branch's history lives in git and the store — the
places that actually record it — and the first agent starts a fresh
conversation anyway (step 10's rule), opening with the human's first message
rather than a backlog of events nobody was there to hear. No event minted on
an agentless branch today is attention-classed except `Merged`, and the human
who merges an agentless branch did it with their own hands.

**Restore tells "never had agents" from "predates agents".**
`AgentRoster::restore` (`agent.rs:153`) currently manufactures a derived
first agent whenever the stored list is empty. That stays correct only for
the pre-agent-era migration, so the store passes whether the record carried
an agents field at all: absent → the derived-id migration exactly as today;
present-but-empty → an empty roster, restored as such. `derived_agent_id`
survives solely for that migration; every agent created after this step is
minted (`new_agent_id`), first or not.

#### 14.4 Remove-all

Every guard protecting the last/first agent goes:

- **`AgentRoster::remove`** (`agent.rs:273`): the `index == 0` refusal is
  deleted — remove is a position lookup and a `Vec::remove`, any index.
- **`agent_remove`** (`app.rs:8080`): the issue refusal stays (an issue's
  agent is the issue); the "FIRST agent is not removable" doc paragraph is
  rewritten to the new rule. `retire_agent` already does the last rites and
  needs nothing.
- **SPA `canRemoveAgent`** (`agentRailModel.js:153`): drops
  `agents.length < 2` and the `agents[0].id === agentId` check — it becomes
  "a branch, and the id names an agent on it". `removeAgentConfirm` is
  unchanged; its outline already says exactly what removal costs.

Removing the last agent leaves a working entity: `railBubbles` already
renders the ghost bubble for an empty roster, `selectAgentId` already answers
`null`, and `paintPanel` already paints the "New agent" head — the chat panel
under it becomes the new-agent view (§14.5). The rail's `agentlessOnce`
say-it-twice guard stays: a poll hiccup still says it once, and a real
remove-all says it every tick.

#### 14.5 The new-agent view

The chat tab of an entity with no agent (an adoptable checkout, an adopted
branch nobody has spoken to, a branch whose agents were all removed) shows,
inside `#rail-body` where the timeline would be:

- **the three harness options**, rendered with the existing
  `providerCardsHtml(STARTABLE_PROVIDERS, chosenId)` (`modelPicker.js:92`) —
  the same cards, chrome and `data-provider` wiring the Agent tab's offer
  uses, no lead card; `chosenId` starts as the account default
  (`models.list`'s `default_provider`) and a press on a card just moves the
  highlight;
- **the live composer** below, exactly as it stands — placeholder "Send a
  message to start an agent here…", attachments working (they already adopt
  on upload).

**A send is one action to the human and three existing verbs on the wire** —
no call gains a field, which is the DRY answer: the rail's `post()`
(`agentRail.js:789`) already composes adopt-if-needed + `thread.post` +
`agent.start`, and `addAgent()` (`agentRail.js:862`) already composes
`agent.add` with a choice. The empty-roster send runs, in order:
`ensureEntity()` (adopts; `run.adopt` now mints no agent), `agent.add
{ entity_id, provider: chosenId, …model/effort from §14.6 }`, `thread.post
{ agent_id }`, `agent.start { agent_id }` — then selects the new bubble. A
failure between calls leaves exactly the states that already exist (an idle
agent with no message is the `+` bubble's normal product). Bridge-side,
`ensure_primary_agent` remains the door for callers that never name a choice
— an old client's bare `thread.post` still works, on the default harness.

**The other two start paths keep their shape.** The rail's `startAgent`
(`agentRail.js:938`) — `ensureEntity()` then `agent.start { id, agent_id?,
provider? }` — and `adoption.js`'s `startAdoptedAgent` (`adoption.js:118`,
which seeds `run.adopt`'s `provider` param and forwards the same token on the
start; today only `test/agentStart.test.js` drives it) are unchanged code. What
changes underneath them is only the bridge's reading: for an entity that HAS
agents the start respawns the harness the named agent is locked to — a
provider named alongside still moves the entity's record, but it cannot
re-carrier an agent that already exists; for one with none, the named provider
persists through `set_entity_model_choice` and `ensure_primary_agent` mints the
primary on it, while a bare start mints on the default harness. `agent.add`
onto an empty roster persists its own choice the same way, so a branch and its
primary never disagree about what the branch runs. So `run.adopt` no longer
minting an agent costs these paths nothing — the adopt-time `provider` param
survives as the entity's persisted choice and the first agent is created on it.

#### 14.6 The composer's model menu

The composer row gains one control on the LEFT of `composer-bar`, opposite
the send button: a mini menu button reading the current selection ("Model ·
effort", or "Default model" when nothing is chosen). Pressing it opens one
menu — built with the split-button idiom's menu half, `mountSplitMenu` /
`splitmenu` markup (`core/splitButton.js:74`), the same machinery the
interrupt send wears — listing:

- the agent's own catalog's models (`catalogForProvider` +
  `modelOptionsHtml`'s data, `modelPicker.js`), current one marked;
- the effort levels below them (`effortOptionsHtml`'s data), shown only
  while `effortSupported` holds for the chosen model, and reconciled through
  `reconcileAgentChoice` (`core/agentChoice.js`) when the model changes.

No provider entry — the agent is locked, so the menu asks only what is still
a question. `composerHtml` (`composer.js:155`) gains the slot behind an
optional `modelMenu` config the way `attachable` works; surfaces that pass
nothing render exactly today's row.

**The write is a new thin verb, because no existing one fits and the spec
says so rather than pretending:** the only RPC that persists a choice today
is `agent.start` (which spawns) — read out of the handle table
(`app.rs:5729–17086`). So: **`agent.choose { entity_id, model?, effort? }`**,
which parses through the existing `model_choice_from` (provider field
refused: `agent.choose: the agent is locked to {label} — model and effort
only`), keeps the entity's persisted provider, and lands in the existing
`set_entity_model_choice` (`app.rs:3765`). That function's while-live refusal
narrows to provider moves only — which is precisely the "live session
untouched until its next start" rule: a model edit under a live session
persists and simply waits for the next spawn to spend it
(`entity_model_choice` at `app.rs:17431` already does the spending).

**With no agent yet**, the menu configures what the first send will create:
the selection is held in the rail's draft state and travels on §14.5's
`agent.add` params (`modelParams`, `modelPicker.js:187`). Nothing is written
to the bridge until the agent exists — there is no entity choice worth
writing for a checkout that may never be adopted.

#### 14.7 The stopped TUI resumes

A stopped TUI session's chat tab needs nothing: a send already resumes — the
turn respawns the locked harness, which resumes by recorded session id (§10
step 10). The TUI pane's offer changes: `mountAgentTab`'s `renderChoices`
(`surfaceTabs.js:180`) — the lead card plus the card row — is replaced by
**one button, "Resume"**, when the pane's agent exists: it calls
`onStart()` with no provider at all, which the rail's `startAgent`
(`agentRail.js:938`) forwards as a bare `agent.start { id, agent_id }` — the
locked harness, the same conversation. `STARTABLE_PROVIDERS`,
`DEFAULT_START_PROVIDER`, `providerCardsHtml` and the `ranProvider` lead
logic leave this file entirely; the busy/disabled/failure handling
(`startAgent`'s single-flight and the standing-offer reset) is kept for the
one button. The rail only mounts the TUI pane for an existing agent with a
terminal, so the pane never needs a card picker again — creating agents is
the chat tab's job now (§14.5).

#### 14.8 Opus 5

`ClaudeHarness::models()` (`harness/claude.rs:38`) gains, between Fable 5 and
Opus 4.8:

```rust
ModelOption { id: "claude-opus-5", label: "Claude Opus 5",
              supports_effort: true, efforts: &EFFORT_LEVELS },
```

Both claude carriers share it by construction — `AdkHarness::models()`
delegates (`adk.rs:67`). The existing
`unknown_but_sane_model_id_passes_through` test already proves old bridges
accept the id; the catalog test grows the assertion that both carriers list
it.

#### 14.9 DRY inventory — what gets reused, never copied

| Need | The one implementation | Reused by |
|---|---|---|
| harness options + labels | `STARTABLE_PROVIDERS` (`modelPicker.js`) | new-agent cards, Account select, tests |
| option markup | `providerOptionsHtml` / `providerCardsHtml` | Account select / new-agent view |
| card→id wiring | `data-provider` + delegated click (the Agent-tab idiom) | new-agent view |
| menu open/close/outside-press | `mountSplitMenu` (`splitButton.js`) | composer model menu (and the interrupt send, as today) |
| catalog reads | `catalogForProvider`, `modelInCatalog`, `effortSupported`, `reconcileAgentChoice`, `modelParams` | composer menu, Account defaults panel, issue sheet |
| composer markup | `composerHtml`'s one template, extended with the optional left slot | every conversation surface |
| send composition | the rail's `ensureEntity` / `post()` / `addAgent()` / `startAgent()` | new-agent send (no parallel path) |
| wire-token parse | `AgentProvider::from_wire` / `model_choice_from` | `settings.set`, `agent.choose`, every minting verb |
| choice persistence | `set_entity_model_choice` | `agent.choose`, `agent.start` |
| auto-add | `ensure_primary_agent` | `thread.post`, `agent.start`, the router, `run.message`, recovery |
| last rites | `retire_agent` | `agent.remove` (unchanged) |
| labels | `Harness::label()` via `AgentProvider::label` | catalogs, refusals, digests |

#### 14.10 The tests

Bridge (`cargo test`, no model turns — everything drives `AppState::handle`,
the roster, or the fake harness):

1. fresh state: `settings.get` answers `default_harness` `"claude_adk"`,
   compat `claude_mode` `"headless"`, `codex_mode` `"tui"`;
2. `settings.set { default_harness: "claude" }` persists across a reload,
   leaves `projects_dir` unmoved, and derives `claude_mode` `"tui"`; an
   unknown value is refused with the expected-tokens sentence and changes
   nothing;
3. compat: `settings.set { claude_mode: "tui" }` still lands
   (`default_harness` reads `"claude"` after), `codex_mode` keeps its
   idempotent accept and exact refusal; a config file holding only the old
   `claude_mode` key loads the mapped default, one holding both prefers
   `default_harness`;
4. `model_choice_from`: absent provider → the default harness; `"claude"` →
   `Claude` concretely whatever the setting says; `"claude_adk"` / `"codex"`
   as themselves;
5. `run.adopt` mints no agent: the roster is empty, `branch.get` answers
   `agents: []`, and no event panics on the way;
6. `thread.post` with no `agent_id` on an empty roster creates one agent on
   the default harness, delivers to it, and the agent sits at index 0;
   `agent.start` on an empty roster does the same;
7. dispatch still mints its agent with the dispatch's own choice;
8. `agent.remove` takes the FIRST agent, and the last: the entity survives,
   `agent.list` answers empty, the session is retired; a post after
   remove-all mints a fresh agent whose conversation starts empty (resumes
   nothing — step 10's rule, asserted through the locator);
9. the router's `default_agent_provider` is `ClaudeAdk` under every setting;
10. labels: the three catalog labels are `"Claude Code"`,
    `"Claude Code TUI"`, `"Codex"`, pairwise distinct, none containing
    "headless"; the switch-refusal message prints the locked names;
11. `agent.choose` persists model/effort on an idle entity; under a LIVE
    session it persists without touching the session and the next start
    spends it; a provider field is refused with the locked sentence;
    `agent.start` naming a different provider while live still refuses;
12. `claude-opus-5` is in both claude carriers' catalogs, `supports_effort`
    true;
13. restore: a stored record with no agents field and a legacy thread still
    migrates onto the derived first agent; one with an empty agents array
    restores empty;
14. the idle sweep skips an agentless working entity instead of panicking.

**Step-13 tests whose premise step 14 deletes** — each goes with the behaviour
it asserted, and the new test that covers what survives is named beside it, so
none of this coverage is dropped silently:

- `claude_and_silence_both_resolve_to_the_carrier_the_account_chose`
  (`app.rs:18938`) — asserted `"claude"` and silence BOTH land on the account's
  carrier. Rewritten as new test 4: silence still follows the default harness,
  `"claude"` is concretely the TUI carrier.
- `the_router_runs_on_the_carrier_the_account_chose` (`app.rs:19038`) —
  asserted the router follows `claude_mode`. Replaced by new test 9: the router
  pins `ClaudeAdk` under every setting.
- `agent_start_naming_claude_re_carriers_an_idle_entity_onto_the_accounts_answer`
  (`app.rs:31653`) — the idle re-carriering that §14.2 kills by construction.
  Deleted; new test 4 covers the token's new meaning and new test 11 covers what
  `agent.start` may still change on an idle entity.
- `a_generic_start_cannot_re_carrier_a_live_agent` (`app.rs:31684`) —
  **deleted, not rewritten in place.** It starts a LIVE TUI run with
  `provider: "claude"` and asserts a refusal. Under step 14 that token names the
  carrier the entity is already on, so `set_entity_model_choice`'s
  same-choice short-circuit (`app.rs:3770`) passes it through and no refusal is
  produced — the assertion would be not merely stale but backwards. The live
  refusal it was protecting is kept, on the scenario that still exists, by new
  test 11's "`agent.start` naming a DIFFERENT provider while live still
  refuses", which also keeps the no-"headless"-in-the-refusal assertion.
- the settings tests `settings_report_the_claude_mode_and_the_locked_codex_one`
  (`app.rs:18785`), `a_chosen_claude_mode_survives_a_reload_and_leaves_the_projects_dir_alone`
  (`18802`) and `an_unknown_claude_mode_is_refused_and_changes_nothing`
  (`18845`) are rewritten onto `default_harness` as new tests 1–3, which keep
  their `claude_mode` assertions as the compat leg.

SPA (`npm test`):

1. `STARTABLE_PROVIDERS` is exactly the three concrete providers with the
   three names; `providerLabel("claude_adk")` is "Claude Code",
   `providerLabel("claude")` is "Claude Code TUI"; no alias helper survives;
2. `canRemoveAgent` answers true for a branch's only agent and its first,
   false on issues and for unknown ids;
3. the empty-roster chat panel renders the three cards with the account
   default highlighted; picking a card and sending composes `agent.add`
   (chosen provider) → `thread.post` → `agent.start`, in order, and selects
   the new agent;
4. the composer's model menu renders at the row's left from the agent's own
   catalog, offers no provider, drops effort for a model that does not
   support it, and calls `agent.choose`; with no agent it feeds the pending
   `agent.add` params instead and calls nothing;
5. the stopped-TUI pane offers exactly one Resume button whose press calls
   `agent.start` with the agent id and no provider;
6. the Account panel is one select whose options read exactly the three
   names, saving `default_harness` with the "Saved." note, and falling back
   to `claude_mode` when the bridge refuses the new key;
7. `loadAgentDefaults` returns a stored `claude_adk` unaliased.

### Step 15 in detail — a row is its content, and two agents to choose from

*Specified 2026-08-31. Branch `build/chat-polish`.* Two screenshots of the
shipped work say the same thing about two different surfaces: the interface
spends its space announcing categories instead of showing content. The
expanded activity list reads "Claude Code called a tool Bas…", "Claude Code
narrated The de…", "Background task started — P…" — the kind label eats the
line and the meat is what gets truncated, with the outcome mark parked off to
the right past the timestamp. And the new-agent view offers three cards —
"Claude Code", "Claude Code TUI", "Codex" — putting a carrier question in
front of every human that step 13 already judged, rightly, to be an
account-level question. Step 15 fixes both. **An activity row is its content:
the icon carries the kind, the outcome mark rides the line directly after the
text, and a tool call reads as the thing it ran — `Bash cargo test`, never a
JSON object. And the harness choice is two cards, "Claude Code" and "Codex";
the account setting decides which carrier a new Claude Code agent gets.**
Step 14's lock is untouched: an agent keeps the harness it was created on,
forever — the setting has its say once, at creation, and never again.

#### 15.1 Per-tool summaries, minted at the source

`tool_call_summary` (`adk.rs:1062`) renders a call as its single string
argument when there is exactly one, and as compact JSON otherwise — which is
how the timeline came to read `Bash {"command":"cargo test","description":…}`
the moment a call carried a second field. The JSON arm dies. The new rule,
still in the one free function:

- **A table names each tool's one human argument** — the meat key, matched on
  the protocol's tool name:

  | tool | meat key |
  |---|---|
  | `Bash` | `command` |
  | `Read`, `Write`, `Edit` | `file_path` |
  | `NotebookEdit` | `notebook_path` |
  | `Glob`, `Grep` | `pattern` |
  | `WebFetch` | `url` |
  | `WebSearch` | `query` |
  | `Task` | `description` |

  A listed tool whose key holds a string mints `{tool} {value}`. Everything
  else the call carried is **dropped** — including Bash's `description`,
  deliberately: the description is the model's paraphrase and the command is
  the record, and a row is one quiet line, not two claims about the same act.
- **The fallback never prints braces.** A tool not in the table — an MCP tool,
  a tool newer than this table — mints `{tool} {first string-valued field of
  the input object}`, in the object's own order; with no string field
  anywhere, the tool name alone. This generalizes the shipped
  single-string-arg rule (a lone string arg IS the first string field) and
  reverses the old "anything else keeps its shape as compact JSON" stance by
  name: that stance held that picking one of three arguments would misdescribe
  the call, but the screenshot shows what JSON buys instead — a line of
  punctuation nobody can scan. The row is a scent; the diff and the fold body
  are the record. A truncated-but-human line beats braces every time.
- **The tool name still leads.** `Bash cargo test`, `Read bridge/src/app.rs` —
  the screenshot's density argues for dropping it, but the icon carries only
  "tool call", not which tool, and `Edit foo.rs` versus `Read foo.rs` is a
  distinction worth five characters. One shape for every call, which is
  exactly the shape single-string calls already mint, so half the persisted
  rows already read this way.
- **Old rows keep their minted text, and the SPA prettifies nothing.** A
  display-only regex over legacy `{tool} {json}` summaries is cheap, but it is
  a second half-copy of the parse that would then need to agree with the mint
  forever. Persisted summaries are records; `tool_call_summary` is the single
  author; a row minted before this step reads as it was minted. Same call on
  clipping: `one_line` / `TOOL_SUMMARY_LIMIT` (`adk.rs:182/1172`) apply
  unchanged after the meat is chosen.

#### 15.2 Label-free rows, and the mark on the line

The kind labels go. Every activity row — expanded in an open run, and the
collapsed run line — is its content:

- **`activityHtml`** (`core/thread.js:767`) drops the
  `thread-activity-what` span. Its head text becomes
  `activityMeat(event, meta, agentLabel)` (`thread.js:801`) — the exact
  function the collapsed run line already reads, so "the line a row shows" has
  ONE implementation and the expanded row and the run head can never disagree.
  `activityMeat`'s own fallback already answers the no-summary edge: an event
  with no summary shows its label, because a blank line is worse.
- **The head's order is icon, meat, mark, time.** `toolOutcomeHtml`
  (`thread.js:744`) moves from after `timeHtml` to directly after the preview
  span, in both `activityHtml` and `activityRunHtml` (`thread.js:821`) — the
  outcome is a fact about the content, so it sits with the content, and the
  timestamp goes back to being the line's quiet right edge. The marks
  themselves (`TOOL_OUTCOME_MARKS`, the pending-is-absence rule, error tone on
  the mark alone) are unchanged.
- **The label survives as metadata, not as text.** The icon span trades
  `aria-hidden` for `role="img"` + `aria-label` carrying the kind's label on
  the expanded row, so a screen reader still hears "Claude Code called a
  tool"; the run head's icon stays decorative. `EVENT_META`'s activity labels
  (`thread.js:18`) are kept for exactly these two jobs — aria and the
  no-summary fallback — and nothing else.
- **`task_update` mints flip description-first.** `started — Deploy watch`
  was written for a row whose label said whose line it was; label-free it
  leads with its least informative word. The mints become
  `{description} — started` (`read_task_roster`/`read_task_started`,
  `adk.rs:749/776`), `{description} — finished` and
  `{description} — failed: {error}` (`ended_summary`, `adk.rs:1137`). The
  notification row (`{description}: {said}`, `adk.rs:853`) is already
  description-first and is untouched. Rows persisted in the old wording
  render forever as minted, same rule as tool summaries. Every test that
  spelled the old words moves with the mint — the seven fixture legs, the live
  `#[ignore]`d background-task leg whose `task_rows` helper matches on a
  `started — ` PREFIX, and `task_description`'s doc comment — all named in
  §15.5, because a wording flip that leaves a hand-run test hanging is a broken
  test nobody sees fail.
- **Non-activity events are untouched.** `session_started`, `committed`,
  `done` and the rest keep their bold labels — they are the conversation's
  record of things that happened, not the working ticker, and their rows are
  the label.

#### 15.3 Two cards, and the setting is the mode

The user-facing harness vocabulary is two agents: **"Claude Code"** and
**"Codex"**. Whether Claude Code opens headless or as the TUI is the
account's `default_harness` setting — which already holds the answer, because
its three concrete tokens fold the two questions (default agent × Claude
Code's carrier) into one stored field. Nothing bridge-side moves:

- **The setting, the wire, and the Settings page are untouched.**
  `default_harness` (`app.rs:1864`), `settings_get`/`settings_set`
  (`app.rs:6579/6593`) with the `claude_mode`/`codex_mode` compat aliases,
  `model_choice_from` (`app.rs:15080`, absent → default, named → concrete),
  `ensure_primary_agent`, the roster's `turn_choice` lock, and the Account
  page's one three-option select (`core/defaultHarness.js`) all stand as
  step 14 built them. The three-option select IS the mode control: choosing
  "Claude Code TUI" as the default is what makes a new Claude Code agent a
  TUI one. The two-control alternative (default agent + a separate Claude
  Code mode) would exist to express one combination — a Codex default whose
  Claude Code card creates TUI agents — and that combination buys a second
  stored field, a second panel, and a second compat story for a preference
  nobody has voiced. Cut, and owned: under a Codex default, "Claude Code"
  means the plain name's own carrier, `claude_adk`.
- **The SPA sends the concrete token, and the bridge stays literal.** Of the
  two revivals on offer — the bridge re-learning a generic `"claude"`
  resolution at the mint chokepoint (step 13's machinery), or the client
  naming the carrier — the client naming it is the one with a single
  authority: the authority is `default_harness`, the client reads it through
  `models.list`'s `default_provider` (already fetched wherever a picker
  paints, `app.rs:5827`), and the wire keeps step 14's every-token-concrete
  rule, so no token means two things across verbs and the lock machinery
  never meets an ambiguous name. One helper in `modelPicker.js`:

  ```js
  /** The two agents a person can create, with the account's answer folded in:
   *  the Claude Code card carries the TUI carrier only when the account's own
   *  default IS the TUI carrier. */
  export function creatableAgents(defaultProviderId) {
    const claudeId = defaultProviderId === "claude" ? "claude" : "claude_adk";
    return [{ id: claudeId, label: "Claude Code" }, { id: "codex", label: "Codex" }];
  }
  ```

- **One narrowing — `creatableCatalog` — and every startable list takes it.**
  There is exactly ONE caller of `startableCatalogProviders`
  (`modelPicker.js:36`) today — the Agent defaults panel
  (`views/settings.js:140`) — and the other startable lists do not filter at
  all: `agentChoicePanelHtml` builds its Agent select from
  `providersOf(catalog)` (`agentChoice.js:24/56`, the raw catalog) and
  `assignmentPanelHtml` from `full.providers` (`issueRender.js:97/126`, the
  normalized catalog, equally raw). Since step 14 the catalog carries all
  three concrete providers under three distinct labels, so changing only the
  filter's contents would leave "Claude Code TUI" standing as a third option
  in both of those selects — the exact outcome this step exists to end. So the
  filter is replaced by a catalog-in / catalog-out narrowing, and each list is
  moved onto it by name. `startableCatalogProviders` is deleted; its argument
  was a providers array, and the account's answer travels on the catalog
  (`default_provider`), so a caller holding only the array cannot ask the
  question:

  ```js
  /** The catalog a create surface offers: exactly the two agents, each carrying
   *  the models the bridge listed for the carrier behind it. Two entries even
   *  before models.list answers — creating an agent needs only a harness. */
  export function creatableCatalog(catalog) {
    const served = (catalog && catalog.providers) || [];
    const providers = creatableAgents(catalog && catalog.default_provider).map((agent) => {
      const listed = served.find((provider) => provider.id === agent.id) || {};
      return { ...agent, models: listed.models || [], efforts: listed.efforts || [] };
    });
    return { ...catalog, providers };
  }
  ```

  The list is BUILT from `creatableAgents` rather than filtered out of the
  catalog: the offer is exactly two whether or not `models.list` has answered
  yet (the rail paints its cards before the round trip), and the labels are the
  client's own vocabulary, so an older bridge that calls both claude carriers
  the same thing still cannot print one name twice — the property the deleted
  filter owned. The narrowed catalog's `default_provider` needs no rewriting:
  `creatableAgents` returns the `claude` id exactly when the account's default
  IS `claude`, so the default is always one of the two entries.

  Every call site, each moved by name:

  1. **new-agent cards** (`agentRail.js:690`):
     `providerCardsHtml(creatableCatalog(catalog || {}).providers, chosen)` —
     card markup and `data-provider` wiring unchanged;
  2. **the rail's `newAgentChoice`** (`agentRail.js:354`): the
     `said.provider || catalog.default_provider || STARTABLE_PROVIDERS[0].id`
     chain becomes `chosenProviderId(creatableCatalog(catalog || {}), said)` —
     one clamp instead of a bespoke fallback, so a stored `"claude"` under a
     `claude_adk` account highlights the Claude Code card instead of
     highlighting none;
  3. **the compose/dispatch panel** (`agentChoicePanelHtml`,
     `agentChoice.js:45`, called from `composeView.js:201/307` and
     `toolbar.js:417`): narrows ONCE at the top —
     `const offered = creatableCatalog(catalog || {})` — and every read below
     it (`chosenProviderId`, `catalogForProvider`, the Agent select's options)
     takes `offered`. `providersOf` goes with its last caller. Both callers are
     create/dispatch surfaces; a LOCKED agent's own picker is the composer's
     model menu (`modelMenuOptions`), which never asks the harness question and
     is untouched;
  4. **that panel's params** (`agentChoiceParams`, `agentChoice.js:82`, called
     from `composeView.js:408` and `toolbar.js:498`): narrows the same way and
     sends the CLAMPED id —
     `modelParams(models, choice.model, choice.effort, choice.provider ? chosenProviderId(offered, choice) : "")` —
     so a stale stored `"claude"` cannot ride out on the wire under a select
     that painted "Claude Code", while an empty provider still means the
     harness's own default;
  5. **the issue assignment select** (`assignmentPanelHtml`,
     `issueRender.js:97/126`): `creatableCatalog(normalizeModelCatalog(catalog))`
     is what `full` becomes, and its hand-rolled
     `assignment.provider || full.default_provider || "claude"` — a third copy
     of the fallback chain, ending in a hardcoded token — becomes
     `chosenProviderId(full, assignment)`, imported from `agentChoice.js`,
     which imports only `text.js` and `modelPicker.js`, so no cycle;
  6. **that select's own dispatch** (`implementParams`, `issueModel.js:241`,
     called from `issueView.js:416/553`): takes the catalog the panel painted
     from instead of a pre-picked models array and delegates to
     `agentChoiceParams(normalizeModelCatalog(catalog), assignment)`, so the
     Implement send carries the CLAMPED id for the same reason item 4 does.
     `issueView`'s `providerModels` — a fourth hand-rolled provider lookup,
     and the one that kept the stale token alive on this path — goes with it;
  7. **the Agent defaults panel** (`views/settings.js:140`):
     `const offered = creatableCatalog(catalog)` replaces the spread plus
     filter.
- **A stored preference naming the other claude carrier clamps, through code
  that already exists.** `chosenProviderId` (`agentChoice.js:28`) answers the
  named provider only when the offered list holds it, else the catalog
  default — and once every surface reads the narrowed catalog, the offered list
  never holds the other carrier, so a browser-local `"claude"` preference under
  a `claude_adk` account default paints AND dispatches as the Claude Code entry
  with no new alias helper. `loadAgentDefaults` stays unaliased (§14.10 test 7
  stands): the stored token is a record, the offer is where it clamps.
- **`STARTABLE_PROVIDERS` survives as the full vocabulary** — three entries,
  three distinct labels — for what it still owns: `providerLabel` (a TUI
  agent's bubble still reads "Claude Code TUI"; the lock makes side-by-side
  carriers real, so their names stay distinct per step 14), and the Account
  select's options via `defaultHarnessOf`/`providerOptionsHtml`. "headless"
  stays banned from every user-facing string; "Claude Code" keeps "Code".
  `agentRail.js` stops importing it — both of its uses became
  `creatableCatalog`/`chosenProviderId` — so the three-name list is left with
  the two jobs that are honestly about naming a harness, not offering one.
- **The lock, the auto-add, and agentless branches are unchanged by
  construction.** `agent.add` gets a concrete provider from the card;
  `ensure_primary_agent` and the router already spend `self.default_harness`
  concretely; `turn_choice` respawns the locked harness. No bridge line
  changes in this half of the step.

#### 15.4 DRY inventory — what gets reused, never copied

| Need | The one implementation | Reused by |
|---|---|---|
| the line a row shows | `activityMeat` (`thread.js`) | expanded row head AND collapsed run head |
| the outcome mark | `toolOutcomeHtml` | both heads, position moved in both |
| a call's summary | `tool_call_summary` + the meat-key table (`adk.rs`) | every minted row; the SPA never re-parses one |
| clipping | `one_line` / `TOOL_SUMMARY_LIMIT` | tool summaries, task rows, unchanged |
| a task row's ending words | `ended_summary` | the terminal patch and the terminal notification |
| the two-agent list | `creatableAgents` (`modelPicker.js`) | `creatableCatalog`, and nothing else calls it directly |
| the catalog a create surface offers | `creatableCatalog` (`modelPicker.js`, replacing `startableCatalogProviders`) | new-agent cards, rail highlight, compose/dispatch panel AND its params, issue assignment AND its Implement dispatch, Agent defaults panel |
| a choice as create/dispatch params | `agentChoiceParams` (`agentChoice.js`) | the compose/toolbar panel AND `implementParams` — one clamp, so no surface can paint one agent and send another |
| card / option markup | `providerCardsHtml` / `providerOptionsHtml` | unchanged, fed the narrowed catalog |
| offer clamping | `chosenProviderId`'s membership fallback, over the narrowed catalog | rail highlight, panel paint, panel params, issue assignment — three bespoke fallback chains deleted |
| full label vocabulary | `STARTABLE_PROVIDERS` / `providerLabel` | bubbles, Account select, refusals |
| the account's answer | `default_harness`, read as `models.list.default_provider` | `creatableAgents` at every paint; the bridge's own auto-add |

#### 15.5 The tests

Bridge (`cargo test`, no model turns — `tool_call_summary` and the task mints
are free functions, and the pump legs drive `adk::fake`):

1. `Bash` with `command` and `description` mints `Bash {command}` exactly —
   no description, no brace;
2. each listed tool mints `{tool} {meat}`: `Read`/`Write`/`Edit` the
   `file_path`, `Glob`/`Grep` the `pattern`, `WebFetch` the `url`,
   `WebSearch` the `query`, `Task` the `description`,
   `NotebookEdit` the `notebook_path`;
3. an unlisted multi-field tool mints its first string-valued field after the
   name, and the summary contains no `{`;
4. an unlisted tool with no string field mints the bare tool name; a listed
   tool whose meat key is absent falls through to the same fallback;
5. a 500-character command clips at `TOOL_SUMMARY_LIMIT` with the ellipsis;
6. task rows mint description-first: roster insert and `task_started` mint
   `{description} — started`, roster removal `{description} — finished`, a
   failed terminal patch `{description} — failed: {error}`, and the
   notification row (`{description}: {said}`) is unchanged;
7. through the fake (recording probe-recorded, per the module's rule): a
   `Bash` `tool_use` lands a thread row whose summary is the command line.

The two-card half adds no bridge test because it changes no bridge line: step
14's harness tests (§14.10) are exactly what still guards it.

**Bridge tests the task-mint flip rewrites** — §15.2 changes the words a task
row is minted with, so every test that spelled the old words is part of the
change, named here so none of it is discovered by a red run:

- the seven task legs that assert full summaries against fixtures
  (`adk.rs:1987–2299`):
  `a_started_task_mints_once_and_keeps_a_turnless_session_working`,
  `a_roster_that_drops_a_task_closes_it_once_and_the_session_waits_again`,
  `a_terminal_task_update_fails_the_task_once`,
  `a_task_notification_is_minted_and_a_progress_patch_is_not`,
  `a_foreground_tasks_notification_closes_it_and_the_session_waits_again`,
  `a_failed_foreground_notification_fails_and_a_stopped_one_finishes`,
  `the_probes_own_order_mints_one_row_per_transition` — each expectation flips
  from `format!("started — {DESC}")` to `format!("{DESC} — started")` and so
  on. The assertions keep their shape; only the wording moves, and item 6
  above is what pins the new wording deliberately rather than incidentally;
- the live `#[ignore]`d leg
  `real_adk_session_reports_a_background_task_and_stays_working`
  (`adk.rs:~3150`), which counts rows with
  `line.starts_with(&format!("task_update: {prefix}"))` for the prefixes
  `"started — "`, `"finished — "` and `"failed — "`. Description-first mints
  never start with those words, so this leg would hang on its first
  `wait_until` when it is next run by hand. Its helper becomes a marker match —
  `line.starts_with("task_update: ") && line.contains(marker)` for `" — started"`,
  `" — finished"` and `" — failed"` — which keeps the `task_update:` guard,
  survives the failed row's trailing `: {error}`, and does not care where in
  the line the words land;
- `task_description`'s doc comment (`adk.rs:1095`), which teaches the rule
  with `started — bi1jfa1kd` / `started — ` examples: rewritten in the new
  wording, since a comment that contradicts its function is a future bug.

SPA (`npm test`):

1. an expanded activity row renders no label span; its head text is the
   summary's first line; the icon carries the kind's label as `aria-label`;
2. an activity event with no summary shows the label as its line;
3. in both the expanded head and the run head, the outcome mark follows the
   preview and precedes the time — the order asserted on the markup;
4. a legacy row whose persisted summary is `Tool {"a":1}` renders verbatim;
5. the new-agent view renders exactly two cards, "Claude Code" and "Codex";
   under `default_provider` `"claude"` the Claude Code card carries
   `data-provider="claude"`, under `"claude_adk"` and `"codex"` it carries
   `"claude_adk"`; the send composes `agent.add` with the card's concrete
   provider;
6. `creatableCatalog` alone: two entries out of a three-provider catalog, ids
   `["claude_adk", "codex"]` under a `claude_adk` or `codex` default and
   `["claude", "codex"]` under a `"claude"` default, labels always
   `["Claude Code", "Codex"]`; each entry carries the models the catalog listed
   for that id; an empty catalog (`models.list` not answered) still yields the
   two entries with empty model lists; the rendered options contain no
   "Claude Code TUI" and no "headless";
7. the compose/dispatch panel and the issue assignment panel each render an
   Agent select of exactly the two entries; a stored provider `"claude"` under
   a `claude_adk` default paints the Claude Code entry as selected, and
   `agentChoiceParams` for that same stale choice sends
   `provider: "claude_adk"` — while a choice with no provider sends no
   `provider` key at all;
8. the same pair for the issue path, asserted together in
   `test/issueRender.test.js`: the assignment panel paints
   `value="claude_adk" selected` under a stale `"claude"` and no "Claude Code
   TUI" option, and `implementParams` for that same assignment sends
   `provider: "claude_adk"` — an assignment naming no agent still sends no
   `provider` key;
9. the rail's `newAgentChoice` under that same stale `"claude"` highlights the
   Claude Code card (one card carries `chosen`, never zero);
10. the Account select still offers the three distinct names (unchanged), and
    `providerLabel("claude")` still reads "Claude Code TUI".

**SPA tests this step deletes or rewrites**:

- `"keeps the catalog entries a picker may offer, under one vocabulary"`
  (`test/modelPicker.test.js:78`) goes with `startableCatalogProviders`. What
  it actually asserted — the client's own labels win over the bridge's, so an
  older bridge cannot print one name twice — is carried by item 6's label
  assertion, which makes the same claim about the function that replaced it;
- the three-name and "never says how a harness runs" tests above it
  (`modelPicker.test.js:41–75`) stand unchanged: `STARTABLE_PROVIDERS` is
  still the naming vocabulary;
- `test/threadActivity.test.js`'s `"started — run the full suite"` fixtures
  need no flip. They are persisted rows as a browser receives them, and §15.2's
  rule is that a row renders as it was minted — so they go on asserting the
  paint. One of them doubles as documentation of the old label ("Background
  task — started — …") and its comment is restated for a label-free row.

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

6. ~~**What may a spawn resume?**~~ **Answered 2026-08-29, specified as §10
   step 10.** In order: a persisted session id that still verifies against
   the harness's own transcript tree resumes that exact conversation; an
   agent record with recorded history but no id keeps today's probe-gated
   `--continue` guess; a brand-new agent record resumes **nothing**, on
   every carrier — the worktree's old conversation belongs to whoever had
   it, and a fresh agent inheriting it is the misdelivery this rule ends
   (hit live 2026-08-29: a new headless agent adopted the worktree's old
   conversation). ~~The one deliberate inheritance survives by derivation
   rather than by a new flag: an adopted entity on which no session has
   ever opened grants its agent the pickup.~~ **Amended 2026-08-31: there is
   no deliberate inheritance. Adoption grants nothing either, because Build
   cannot show the history the human's own session carries — see §10.4.**
   And the id itself is polymorphic: captured from `init`
   where the child announces one, located in the harness's durable records
   where it does not, persisted through one record path, spent as
   `--resume <id>` / `codex resume <id>`, and never, under any carrier,
   scraped off a screen.

---

## 12. Revision history

- **2026-08-31, the issue's Implement dispatch joined the clamp.** Review found
  the one create path the SPA half missed, which is why the entry below is
  wrong to call the `+` bubble the last of them: `assignmentPanelHtml` painted
  the clamped agent while `implementParams` still spent `assignment.provider`
  raw, so an issue holding `"claude"` under a `claude_adk` account showed
  "Claude Code" and created a TUI agent on Implement. `implementParams` now
  takes the catalog the panel paints from and delegates to `agentChoiceParams`,
  which deletes `issueView`'s `providerModels` — the fourth hand-rolled
  provider lookup — and leaves one function answering "what does this choice
  send" for every create surface (§15.3 item 6, §15.5 item 8).
- **2026-08-31, step 15's SPA half built.** As specified, with three additions
  the build found. `syncValues` (`assignmentOverlay.js`) writes the held
  assignment back over the painted markup, so a stored `"claude"` emptied the
  Provider select the narrowing had just clamped — a select is now only handed
  a value one of its options carries, which is the same "the offer is where it
  clamps" rule the panel markup follows. The `+` bubble
  (`agentRail.pressAddBubble`) spends the browser-local agent defaults
  straight on the wire and was the one create path that could still dispatch
  the carrier no surface offers; it takes `chosenProviderId` over the narrowed
  catalog, guarded so an empty preference still sends no `provider` at all. And
  the Default agent panel's copy gained one sentence — choosing Claude Code TUI
  there is what gives a new Claude Code agent a terminal of its own — because
  the two-card offer leaves that select as the only screen that can say it; the
  control itself is untouched, and "headless" stays unsaid.
- **2026-08-31, step 15 specified — a row is its content, and two agents to
  choose from** (branch `build/chat-polish`). Two screenshots showed the kind
  labels eating the activity lines and the three-card new-agent view putting a
  carrier question back in front of every human. Activity rows drop every
  label: the expanded head reads through `activityMeat` — the same function
  the collapsed run line reads, so the line has one implementation — with the
  outcome mark moved inline after the text in both heads, the label surviving
  only as the icon's `aria-label` and the no-summary fallback, and
  `task_update` mints flipped description-first (`{description} — started`),
  which rewrites the seven task legs' fixtures and the live background-task
  leg's prefix matcher — both listed in §15.5 rather than left to a red run.
  Tool summaries are minted per tool at `tool_call_summary`: a meat-key table
  (Bash → `command`, file tools → `file_path`, `Glob`/`Grep` → `pattern`, …),
  Bash's description dropped, the name still leading, and the JSON fallback
  replaced by first-string-field-or-bare-name — braces never; legacy rows
  render as minted, the SPA prettifies nothing. The harness choice becomes
  two cards, "Claude Code" and "Codex", via one `creatableAgents` helper and
  one `creatableCatalog` narrowing that REPLACES `startableCatalogProviders` —
  named at all five call sites, because two of them (the compose/dispatch panel
  and the issue assignment select) never filtered the catalog at all and would
  otherwise keep offering a third card; the account's existing three-token
  `default_harness` IS the mode setting (its claude token decides what the
  Claude Code card creates; under a Codex default the plain name means
  `claude_adk`), so the bridge, the wire, the compat aliases and the Settings
  page are untouched, the SPA keeps sending concrete tokens, and step 14's
  lock, auto-add and agentless-branch machinery stand unchanged. This
  supersedes §14.5's three-card view and revives the substance of step 13's
  mode setting without its generic-token resolution.
- **2026-08-31, a new agent never resumes — adoption's pickup is deleted.**
  §10.4's rule 2 carried a second disjunct: an entity whose record said
  `adopted` and on which no session lineage had ever opened granted its agent
  the `--continue` guess, so an adopted branch's first agent opened on the
  conversation the human had been having in that checkout. It is gone, and the
  reason is what Build can SHOW: a new agent's conversation view starts at
  sequence 1, so an agent resumed onto a session Build never heard answers out
  of a history nothing on screen holds. The derivation was unstable besides —
  the lineage it read lives on the roster's threads, so removing every agent
  from an old adopted branch made it read as freshly adopted again and hand the
  next agent an entire prior session (observed live 2026-08-31). What remains is
  a deletion, not a mechanism: `may_pick_up_a_conversation` is one reading of
  the agent's own `thread.sessions`, and `pending_continuation` — the flag the
  disjunct was derived from the retirement of — is deleted from `ActiveRun` and
  `PersistedRun` (nothing read it; `#[serde(default)]` was never on the load
  path for it and a dropped field is ignored on read). `adopted` keeps its other
  readers: prune rules, boot-recovery parking, the release verb, the SPA's
  option sets. Rules 1 and 4 are untouched — a verified id still `--resume`s,
  and an agent with recorded history still `--continue`s ITS OWN conversation
  across the crash window. Tests: the two that pinned the pickup now pin its
  absence (the adopted branch's first spawn, and an agent added back after
  remove-all on an old adopted branch — the live case), beside the unchanged
  resume-by-name and continue-by-history arms.
- **2026-08-31, the harness lock holds on every respawn.** Review found the
  lock §14 exists to hold broken by §14.5's own flagship flow: the lock is
  per-AGENT, but `agent.start`, `PendingAgentTurn::for_run`/`for_run_agent`/
  `for_plan`/`for_recovery` all spawned from the ENTITY's choice. Pick the
  "Claude Code TUI" card on a branch adopted on the default and the first
  delivery was right while the pane's Resume — and every later turn —
  reopened the other carrier; the same wrong source broke the Resume of any
  non-primary agent on a mixed-harness branch. Fixed in the roster, where the
  lock lives: `AgentRoster::turn_choice(agent_id, entity)` answers what a turn
  addressed to one agent spends — that agent's harness, carrying the entity's
  model and effort (what §14.6's menu edits), or the agent's own selection
  whole where the entity is set to a harness whose vocabulary its model id is
  not in — and every path addressing an existing agent reads it, `thread.post`
  included. `agent.add` onto a branch with NO agents now persists its choice
  through `set_entity_model_choice`, so the harness picked in the new-agent
  view is the branch's too and `agent.choose`'s refusal names the lock the
  human is actually under. A start that NAMES a provider still moves the
  entity's record; what it can no longer do is re-carrier an agent that
  already exists. Tests: add-with-provider-then-bare-start (which nothing
  covered), a start naming the second agent of a mixed branch, and a queued
  turn — named agent and primary alike — on the agent's own harness.
- **2026-08-31, step 14 read back for reuse.** Six consolidations, no behaviour
  moved. Dropping the roster's `Deref` had left 111 sites walking by hand from
  an entity to the thread its agent owns: `AgentRoster::sole_thread` /
  `sole_thread_mut` name the walk for an issue, and the tests' `primary_thread`
  gained the mutable twin the writing sites needed. The choice between the
  owning Issue's conversation and a run's own — with the mint door for the
  second — was copied into `on_run_agent_done` and
  `consume_run_stage_revision`; it is `run_report_conversation` now.
  `settings_set`'s local holding what `claude_mode` resolved to is named for
  the harness it carries. On the client, the rail derived the new agent's
  harness in three separate expressions and now resolves it once;
  `mountAgentTab`'s internals stopped calling their one Resume button a
  "picker", and the rail stopped handing its start a provider the pane no
  longer passes; and the lead card's leftovers went with it —
  `providerCardHtml` (folded into its only caller), the description slot no
  caller fills, the disabled-card styling nothing disables, and the
  `.chooser-head` rule the rail had to override to undo.
- **2026-08-30, step 14's SPA half built.** As specified, with three naming
  corrections worth recording. The Account panel's module is
  `core/defaultHarness.js` (`defaultHarnessPanelHtml` / `mountDefaultHarness` /
  `defaultHarnessOf`), renamed from `core/agentMode.js` because the setting is
  no longer a mode: it names the harness a new agent is created on. The
  composer's menu markup is `menuButtonMarkup` — the menu half of the split
  button standing alone, extracted beside `splitButtonMarkup` so the rows have
  one implementation — and its catalog reads (`modelMenuOptions`,
  `modelMenuLabel`, `modelMenuSelection`) live in `core/agentChoice.js`, beside
  `reconcileAgentChoice`, which they use. `agent.choose` is sent with both
  fields always (`model`, `effort`), empty meaning the harness default, which
  is exactly what `model_choice_from` already reads an empty string as. The
  `lead` card option of `providerCardsHtml` is deleted with the offer that used
  it, and the rail's placeholder no longer moves under a poll — gaining an
  agent rebuilds the panel around a conversation.
- **2026-08-30, step 14's citations corrected.** A read-back against the
  source fixed four misnamed call sites in §14.3's migration table and §14.5 —
  the `.first()` site at `app.rs:16524` is `issue_session`, not
  `retire_issue_session` (which only delegates to `retire_agent`); the
  `resolve` + first-compare at `11567–11568` is `run_request_changes`, not
  `run_stage_dispatch`; the Issue-swap trio at `10941`/`11015`/`3037` is
  `offering_thread`/`thread_post`/`close_turn_of_dead_agent`, now three rows
  with their three different empty-roster answers; and §14.5 named
  `createStartingAdoptingCall`, which does not exist — the real paths are the
  rail's `startAgent` and `adoption.js`'s `startAdoptedAgent`. §14.10 gains the
  roll of step-13 tests step 14 deletes, chief among them
  `a_generic_start_cannot_re_carrier_a_live_agent` (`app.rs:31684`), whose
  assertion inverts once `"claude"` is concrete. No decision changed.
- **2026-08-30, step 14 specified — agents lock to their harness, and
  branches start with none** (branch `build/new-agent-flow`). Supersedes
  step 13's carrier-follows-setting half: an agent is locked to the harness
  it was created on, so its conversation provably stays its own — nothing
  re-carriers at start time, and the label fold reverses into three names,
  "Claude Code" (the headless carrier), "Claude Code TUI", "Codex", with
  "headless" still banned from user-facing strings. The account setting
  becomes `default_harness` (a concrete provider token, default
  `claude_adk`), persisted in place of `claude_mode`, with the old
  `claude_mode`/`codex_mode` keys kept as derived read/write compat aliases
  and the config file falling back to a stored `claude_mode` on load; the
  Account page collapses to one select sharing `STARTABLE_PROVIDERS`'
  vocabulary. `model_choice_from` resolves absent → the default and every
  named token concretely; the router pins `ClaudeAdk`. Branches start with
  NO agents: adoption mints an empty roster (dispatch keeps minting, since
  it delivers), `AgentRoster` drops its non-empty invariant and its
  first-agent `Deref` for `primary()`/`Option`, entity-level events on an
  empty roster are skipped, and `ensure_primary_agent` is the one door
  through which the system auto-adds (default harness) when it must deliver
  — `thread.post`, `agent.start`, the router, recovery. Remove-all: the
  roster's index-0 refusal and the SPA's `canRemoveAgent` guards go;
  removing the last agent leaves the entity on the new-agent view — the
  empty chat tab renders the three harness cards (`providerCardsHtml`,
  account default highlighted) over the live composer, and a send composes
  the existing `agent.add` → `thread.post` → `agent.start`, no call gaining
  a field. The composer gains a combined model+effort menu at the row's left
  (split-button menu machinery + the modelPicker catalog helpers), writing
  through the new thin `agent.choose` into `set_entity_model_choice`, whose
  while-live refusal narrows to provider moves — a live session is untouched
  until its next start; with no agent the menu configures the first send's
  `agent.add`. The stopped-TUI pane's card offer becomes one Resume button
  (bare `agent.start` — same harness, same conversation). Opus 5 joins the
  shared claude catalog (`claude-opus-5`, "Claude Opus 5", effort-capable).
  §10 gains step 14 with the DRY inventory and the test list; no test runs a
  model turn.
- **2026-08-30, step 13 specified — the account chooses Claude Code's
  carrier.** An account setting on the bridge decides which carrier "Claude
  Code" means: `settings.get`/`settings.set` gain `claude_mode`
  (`"headless"` | `"tui"`, absent = headless, persisted in the config file
  beside `projects_dir`) and a hard-locked `codex_mode` (always `"tui"`,
  synthesized not stored, any other value refused with `codex_mode accepts
  only "tui" — Codex has no other mode yet`); `settings.set` becomes
  field-wise optional, old clients naming only `projects_dir` unchanged.
  Resolution lives at the one minting chokepoint — `model_choice_from` gains
  a `claude_means` parameter fed by the setting — so `"claude"` and an
  absent provider resolve to the concrete carrier (`ClaudeAdk` by default),
  `"claude_adk"` is honored as-is, `"codex"` is always `Codex`, persisted
  choices stay concrete, existing entities never migrate, and resumes never
  re-resolve (serde default stays `Claude`). The owned semantic shift: the
  token `"claude"` changes meaning at start-time from "the TUI carrier" to
  "whatever the setting says" — additive fields only, but a fresh bridge now
  starts headless, and an old client sending `"claude"` gets the new
  meaning on purpose. Claude is Claude: `AdkHarness::label()` becomes
  "Claude Code" (fixing the catalog and the switch refusal in one change;
  the catalog keeps all three concrete entries for machine truth),
  `STARTABLE_PROVIDERS` collapses to two cards, and every SPA read of a
  concrete `claude_adk` token aliases through `genericProviderId` —
  `providerLabel`, `harnessLabel`, the Agent tab's lead card (which now
  starts generic), `loadAgentDefaults`, and the Account page's
  catalog-filtered defaults select — so the word "headless" appears in no
  user-facing string. The Settings page gains a bridge-persisted modes panel
  in the save-on-change select idiom: "Claude Code" / "Claude Code TUI" for
  claude, Codex shown locked to TUI with a short note. §10 gains step 13
  with the test list; no test runs a model turn.
- **2026-08-30, activity renders grouped.** A maximal run of consecutive
  activity rows between two things somebody said (or any non-activity row)
  collapses to a single dim line — a counter and the newest item's summary
  first line, no label — that opens onto the per-row folds as they render
  today, eight rows tall and scrolling past that. SPA-only (`core/thread.js`,
  `styles.css`): no wire, store or bridge change. The run is a `<details>`
  keyed by its first item's sequence, so it keeps its identity and its open
  state while its tail grows under a live agent.
- **2026-08-30, step 12 specified — a tool call and its answer are one row.**
  The reader's own pairing (`ProtocolReader.calls`, which has matched every
  `tool_result` to its `tool_use` by id since step 6) stops being discarded
  at minting: one thread row per tool call, minted at the call, updated in
  place when its answer arrives. `ThreadEvent` joins `ThreadMessage`'s
  `updated_sequence` machinery additively (serde default 0, the store column
  already exists, `latest_sequence`'s Event arm becomes a max — and every
  consulting path was read and is listed in §10 step 12.1: the forward
  delta, the history merge, the high-water mark, the load's counter repair
  and the store's dirty check all inherit the one arm; the unread family
  deliberately stays on creation sequence; no code hashes or dedups events;
  the SPA cursor already reads `updated_sequence` off every item, so the
  client cursor needs no change). `AgentActivity::ToolUse` gains `call_id`;
  `ToolResult` becomes the completion signal — `call_id` plus
  `ToolOutcome::{Ok, Error, Unanswered}` and the one-line answer — and the
  pump keeps a per-session `open_calls` map (call id → minted row sequence),
  updating the row through `Thread::resolve_tool_call`: summary gains
  `→ <answer>`, a new additive `outcome` field ("ok"/"error"/"unanswered")
  carries the state, and no `tool_result` row is minted on the happy path.
  The kind stays for stored rows and for the orphan fallback (a result whose
  call was lost mints standalone, as today). Unanswered calls close by
  update, never by a fabricated answer: the reader drains its map at every
  `result` line (`→ no answer — turn ended` — the interrupt case; both maps
  empty at every turn boundary by construction) and the death rites close
  the rest (`→ no answer — session ended`) before `record_agent_session_end`.
  Tool-heavy sessions mint roughly half the rows; the counted predicate is
  untouched (`message = 0, attention = 0` on both sides of the update,
  pinned). The SPA's `tool_use` fold gains a trailing state mark — nothing
  while pending, `✓` ok, `✕` error on the mark alone, `⊘` unanswered — with
  the answer in the fold body, legacy `tool_result` rows rendering as
  always, and unknown outcome tokens reading as pending. §5 and §8 amended;
  §10 gains step 12 with the fake's new recordings and the test list, none
  running a model turn.

- **2026-08-30, step 11's review fixes — a foreground command is a task too.**
  Running the whole live family turned up a defect the background probe could
  not see, and the fix took §11.1's own escape hatch: the rules stand, the
  reading moved. A FOREGROUND Bash command gets `task_started` like any other
  task, and is closed by a `task_notification` carrying a terminal status —
  **alone**: no `task_updated`, no `background_tasks_changed`, ever
  (raw-wire probes against claude 2.1.236, both a `completed` and a `failed`
  one, now recorded in `adk::fake` as `FOREGROUND_TASK_*`). Both lines arrive
  together when the command finishes, so the shipped reader inserted the task
  and had nothing that could ever remove it: the session reported `Working`
  for the rest of its life, the sweep never demoted it, and the rail dot
  pulsed over an idle agent — the inverse of the failure this step closes,
  and what made `real_adk_session_steers_mid_turn` hang waiting for
  `Waiting`. So a terminal notification IS a membership removal and mints the
  ending row; its text still mints first, unless the text merely repeats the
  task's own name, which is exactly what a foreground notification's
  `summary` is. `stopped` joined the terminal statuses — the same probe run
  turned it up on a task the child killed. An interrupted foreground command
  emits no task lines at all, so a stopped turn leaves nothing in the set.
  Separately, `real_adk_session_interrupts_mid_tool`'s premise was rebuilt:
  the installed CLI now BLOCKS a standalone `sleep`, the model reruns it in
  the background, and nothing is parked — so the leg parks the turn in
  `python3 -c "import time; time.sleep(90)"`, which the CLI runs in the
  foreground, and asserts the stop-and-steer settles inside sixty seconds (it
  takes about five). All three live legs pass.

- **2026-08-29, step 11's bridge half shipped — the payloads are pinned.**
  One probe against claude 2.1.236 (a headless turn that backgrounded
  `sleep 12 && echo woke`) produced all four events, and their shapes are
  now recorded in `adk::fake` — `TASK_ROSTER` / `TASK_STARTED` /
  `TASK_ROSTER_EMPTY` / `TASK_UPDATED_DONE` / `TASK_NOTIFICATION` — which is
  what the tests hold the reader to. What the probe settled:
  `background_tasks_changed` carries the **roster** (`tasks: [{task_id,
  task_type, description}]`, empty when nothing is live), so §11.1's
  replace-the-set rule stands as written rather than its delta fallback;
  `task_started` carries `task_id` + `description`; `task_updated` carries
  `task_id` + a `patch` of `{status, end_time}` and NO description of its
  own; `task_notification` carries the human-readable line in `summary`. The
  live child emitted them roster-first — roster, `task_started`, `result`,
  empty roster, `task_updated`, `task_notification` — so it is the ROSTER
  that inserts and removes, and `task_started` / `task_updated` mostly move
  nothing, which is exactly the one-row-per-transition rule doing its job.
  Two readings the shapes forced: a `task_updated` that moves no membership
  mints nothing (its patch carries no human-readable line to mint — a
  `description` in it renames the task for later rows instead), and a
  notification that lands after the roster already closed its task mints its
  own text without a name the set no longer holds. Also amended: a terminal
  status is a NAMED set (`completed`/`failed`/`error`/`cancelled`/`killed`/
  `timed_out`) so an unrecognised one leaves the task for the roster to
  close, and `cancelled`/`killed` read as `finished`, not `failed`.

- **2026-08-30, step 11's SPA half shipped — step 11 is complete.** §11.4
  cost exactly what it was specified to cost: one entry on
  `core/thread.js`'s activity map, `task_update` labelled "Background task",
  and no other line of client code. It folds shut like the other four, its
  head previews the summary's first line, a fold the reader opened survives
  the repaint under it, and an unknown kind still renders as the plain row
  it always did. The label carries no provider name, because
  `activityHtml` only swaps a leading `Agent` and this label has none —
  the row names the task, not the harness.

- **2026-08-29, step 11 specified — background tasks are visible, and the
  agent stays Working.** The headless-looks-idle finding, closed by design:
  the stream's `system` task events (`task_started`, `task_updated`,
  `task_notification`, `background_tasks_changed` — all observed live
  2026-08-29; payload shapes to be pinned at implementation against the
  installed CLI or a no-model-turn probe, never taken from this spec)
  reconcile a live-task set on `ProtocolState`, with
  `background_tasks_changed` replacing the set as the source of truth and
  membership transitions minting the fifth activity kind — `TaskUpdate`,
  wire `task_update`, class `Status`, intrinsic like the other four, riding
  the existing pump, thread rows and §6.3 free ride, with started / finished
  / failed always minted and notification text when it carries something
  human-readable. `AdkSession::status()` reports `Working` while a turn is
  open OR the set is non-empty, so the digest's `working` follows with no
  new code and the idle sweep's existing short-circuit stops demotion with
  no new sweep code (a required test holds a quiet, turn-closed session
  with a live task undemoted, and demoted once the roster empties).
  `can_interrupt` stays tied to an open turn — `capability && turn_open`,
  making `working: true, can_interrupt: false` a legal digest the shipped
  SPA already renders as the plain Send — `Ended` still wins on exit with
  tasks open, and the death rites are untouched. Headless carrier only: a
  PTY's terminal already shows its churn. The SPA folds `task_update` like
  the other kinds under its own quiet label, and `adk::fake` grows the
  recorded task-event lines so every test runs without a model turn. §5 and
  §8 gain the fifth kind; §3's one-value note is amended; §10 gains step 11
  with the test list.
- **2026-08-29, step 10 shipped — every carrier names its conversation, and a
  fresh agent starts fresh.** `6d937fc` (the locators over each harness's own
  transcript tree, `holds_conversation`, and both PTY carriers' three-way
  resume argv — claude in flag shape, codex as a subcommand behind the
  `--config` overrides), `178085d` (`Carrier::Terminal` carries a locator,
  `PtySession` answers `session_id` by delegating to it, built at the
  reservation before the child exists so the snapshot cannot contain the
  child's own file), `480d3a8` (the ordered spawn rule: a recorded name
  verified and cleared where it fails, the cwd guess gated on the agent's own
  history or an adopted entity whose lineage has never opened, fresh
  otherwise — routers included) and `55e7f10` (the capture: the idle sweep
  asks each live session with the lock released and writes through
  `note_named_conversation`, the one record path both carriers now use, plus
  the byte pump's final reading at close, which records and never clears).
  The 2026-08-29 misdelivery is pinned by a test: a brand-new agent record
  over a checkout holding an old transcript gets neither `--resume` nor
  `--continue`. No wire or SPA change, as designed.
- **2026-08-29, step 10 specified — the polymorphic session id, and the
  fresh spawn.** Every carrier can now answer `session_id`, from its own
  durable artifacts and never from a screen: the `Harness` supplies a
  `SessionLocator` over its transcript tree (claude: the filename under the
  munged-cwd projects dir IS the id; codex: the dated global rollout whose
  header cwd matches — cwd-matched because recency alone misattributes,
  and answered only when exactly one candidate exists, because the locator
  never guesses). The spawn reservation builds the locator before the child
  exists and installs it into `PtySession`; the idle sweep's 5 s tick
  captures the answer with the state lock released and writes it through
  `record_agent_resume_id` — the step-8 path — with one final reading at
  the byte pump's close, which records and never clears. The id is spent
  per carrier (`ClaudeHarness` gains `AdkHarness`'s `--resume` match;
  `CodexHarness` gains `resume <id>`, a subcommand at the argv tail where
  `resume --last` sits today) and is verified against the same tree before
  it is spent (`Harness::holds_conversation`), so a dead id costs zero
  restarts. And the spawn rule is sharpened into §11 q6: id → exact resume;
  history → the probe-gated guess; a brand-new agent record → fresh on
  every carrier, with adoption's pickup preserved by `adopted` plus an
  empty lineage — the vestigial `pending_continuation`'s meaning, derived
  instead of revived. No wire or SPA change: resume is invisible to the
  client, confirmed against the digest and the record's serde shape. §3's
  `session_id` doc is amended (the transcript tree is a durable record, not
  a scrape; the screen stays forbidden), and §10 gains step 10 with the
  test list.
- **2026-08-29, §6.3 complete — the client half was a test.** `c0d1b89`. The
  SPA needed no change, proven rather than assumed: `createThreadCache` is
  the whole of the client's paging, it bounds a window by `thread_total` and
  by a page's seek, and it counts nothing — so an oversized page merges like
  any other. Three tests hold it against pages cut by a JS mirror of the
  daemon's `page_span` (a 360-item first page against a limit of 60; the ×10
  ceiling ending pages early on a fifteen-tool-call-a-turn conversation, walked
  back to the start with every sequence seen once; a delta carrying a turn's
  tool calls not tripping the gap check), and all three fail if an item-count
  bound is restored. §6.3 gains the verdict; step 9, and with it §6, is done.
- **2026-08-29, §6.3 shipped — step 9.** `51c6d0b` (`ThreadItem::counted()`,
  held equal to the store's `message = 1 OR attention = 1` across every kind),
  `48d3933` (schema v3: `message` hoisted by the v1→v2 precedent, one partial
  index over the counted predicate, one classifier writing both columns so a
  v1 database reaches v3 in one open, and the plan pinned to the index by
  name), `50f2cb7` (the packet: the starved-tail gate, the store read, the
  merge below `resident_from_sequence`, and composition moved to
  `deliver_pending_agent_turns` — with `conversation_prompt` losing its thread
  entirely and the packet's conversation becoming one rule, which fixes a
  planned implementation reading its packet off the run's own empty thread)
  and `a7efeb8` (the counted page, its ×10 ceiling, and the store's three-seek
  page). The SPA's oversized-page merge test followed in `c0d1b89`.
- **2026-08-29, §6.3 designed.** One hoisted column funds both remedies: an
  item is *counted* when it is a message or an attention-classed event
  (`ThreadItem::counted()` in Rust, `message = 1 OR attention = 1` in SQL —
  schema v3 hoists `message` by the exact v1→v2 `attention` precedent, with
  one partial index and the query-plan test extended). The catch-up packet
  stays 40 messages and gains a store read only when the tail is starved: a
  two-integer gate keeps the all-resident thread byte-identical and SQL-free,
  the merge follows `wire_value_after_including_history` (store rows admitted
  only below `resident_from_sequence`), and the packet composes at
  `deliver_pending_agent_turns` instead of being baked at transition time —
  late-bound, the router opting out, a warm recovery losing its redundant
  copy, and a store error falling back to the tail-built packet rather than
  dropping the turn. A page's `limit` buys counted items, activity rides free
  between them, and a hard ceiling of ten items per unit of budget bounds the
  pathological all-activity page (and the `thread_limit: 1` polls at ten a
  tick); `has_more`, `oldest_sequence` and `thread_total` keep their wire
  meanings and pages stay contiguous, so the shipped SPA needs no change —
  held by reading `absorbOlderPage` and the `thread_total` gap check, with
  the Client check stage owing only an oversized-page merge test. §10 gains
  step 9.
- **2026-08-28, the stale press.** The final review's one defect, fixed in
  `7e6c0d5`: `interrupt()` recorded its pending request without asking whether a
  turn was open, so a press that landed after the turn's own result had closed
  it leaked into the turn queued behind it — which then reported `Working` with
  nothing running and lost its own error. §8.1 gains the guard and §8.5 test 12.
- **2026-08-28, step 8 complete — the composer's split send.** §8.4 as
  designed, in `07980b3` (`agentCanInterrupt`, the whole condition in one
  reader), `8d114b4` (the send control's two shapes, and `primaryId` so it
  keeps its id in both) and `7eb0398` (the wiring: the flag on the post, the
  in-place swap, and `mountSplitMenu` shared out of `mountSplitButton` because
  the composer's press restores its own button rather than leaving it
  disabled). Step 8, and with it the migration order, is done.
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

# Agent Session Interface — Spec

**Status:** Draft — steps 0–5 shipped; steps 5a–7 specified, not started (see §10)
**Last updated:** August 23, 2026
**Branch:** `build/agent-polymorphism`

---

## 1. What this is

Build currently has one kind of agent: a CLI wrapper in a full PTY. Claude's ADK
and Codex's app server are not that. They expose a **session protocol** — an
event stream of reasoning, tool uses and messages — with no terminal anywhere in
it. Pi and OpenCode are the same shape.

The `Harness` / `HarnessSession` split already on this branch made the *launch*
polymorphic and put the session behind a trait. It did not make the terminal
optional: `HarnessSession` still requires `subscribe() -> bytes`, `resize`,
`write_input` and `pid`, because `PtySession` was the only implementation there
has ever been.

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

`deliver` (`bridge/src/app.rs:17132`) is documented as "the one pipe from Build
to a worktree's agent", and every turn Build sends goes through it. But it ends
in `HarnessSession::write_prompt`, which means bracketed-paste framing, a
sanitised prompt, and a submit key written 1500 ms later
(`REAL_TUI_SUBMIT_DELAY`). Those are keystroke mechanics. A session protocol
takes a turn as a value.

### Status — inferred, and the inference is wrong for an event stream

`agent_is_working` (`bridge/src/app.rs:893`) was four conjuncts — step 2 has
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
}
```

- **`quiet_for`** exists because the idle sweep asks a minutes-scale question
  that `status()` does not answer. `mark_idle_tasks` (`app.rs:5784`) and
  `agent_last_painted_at` (`app.rs:13443`) read `idle_for` today — the PTY's
  paint clock. The name changes because the meaning generalizes: a PTY answers
  from its last byte (behavior byte-identical to `idle_for`), a protocol
  session from its last read protocol event. This is not status duplicated:
  `Working`/`Waiting` is a 30-second-window judgement; the sweep's
  quiet-for-five-minutes anomaly clock is a different instrument, and both
  carriers can hold one honestly.

- **`exited_within`** is `deliver`'s crashed-versus-wedged wait
  (`app.rs:17257`, `PROMPT_WRITE_EXIT_GRACE`): a failed write to a harness
  that exits within the grace is a crash the sweep will explain, not an error
  to surface. Both carriers are subprocess-backed today, and any carrier that
  is not still has a truthful degenerate answer (`status()` already `Ended`).

- **`epitaph`** exists because `HarnessExit` (`app.rs:16364`) explains a crash
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

- **May `send_turn` block?** Yes, and the trait doc must say so: the PTY's
  implementation writes a framed paste and then sleeps `REAL_TUI_SUBMIT_DELAY`
  (1500 ms) before the submit key, so a call can hold its thread for seconds;
  a protocol implementation returns as soon as the turn is written to the
  child's stdin and never waits on the model. The contract: **callers must
  not hold the app-wide state lock across `send_turn`** — which the daemon
  already honours, because `deliver_pending_agent_turns` drains the queue
  after the verbs release the lock. Writing it down is what keeps a future
  caller from re-learning it from a deadlock.

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
   packet (`bridge/src/orchestrator.rs:658`), so the densest of the four is not
   carried by this path anyway.

**Known gap, fix deferred — noted 2026-08-23.** Reason 1 no longer holds for
outcomes reported through `done`: `post_completion` (`thread.rs:1604`)
deliberately posts no message — the event is the only record — and
`last_completion` is only set when a structured report exists, which a blocked
`done` does not carry. So a messages-only packet tells a replacement agent
nothing about why its predecessor blocked. This spec stays a refactor and does
not fix that here.

**The fix, designed 2026-08-23 — §10 step 7.** An outcome is a **status
attached to a message**, not a separate event.

- **What changes where it is minted.** `record_report_in_thread`
  (`app.rs:16301`) stops pushing `Done` / `Blocked` / `RunFailed` for a
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
fsync, rename — and `save_issue_implementation` (`bridge/src/store.rs:615`) is a
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

## 7. Terminal-coupled surfaces — the inventory

Everything that must become conditional. This is the actual size of the work.

### Bridge

| Site | Today | Change |
|---|---|---|
| `Tab.session` (`app.rs:652`) | `Box<dyn HarnessSession>` | `Box<dyn AgentSession>` |
| `Tab.screen` (`app.rs:653`) | always a `TermScreen` | `Option<TermScreen>` — no grid without a terminal |
| `agent_is_working` (`app.rs:893`) | reads `idle_for` | reads `status()` |
| `agent_attach` (`app.rs`) | attaches a grid, defaults 40×120 | refuses, with a reason, for a session with no terminal |
| `term.input` / `term.resize` | assume a PTY | refuse for an agent tab with no terminal |
| `spawn_tab_pump` | pumps bytes into `TermScreen` | when there is no terminal, posts activity into the conversation instead |
| `agent_digest` (`app.rs:7601`) | `"working": bool` | add `"has_terminal": bool`; keep `working` |
| `catch_up_markdown` (`thread.rs:2270`) | last 40 items by recency, events included | messages only, the limit counting messages (§6.1) — **shipped** |
| `Thread::items` (`thread.rs:1167`) | resident tail of 200 items, older items paged from SQLite | unchanged — activity rows ride the same tail and pages |

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
// agent digest — one new field
{ "id": "agent-…", "working": true, "has_terminal": true, … }
```

```json
// thread items — four new event kinds, in the envelope every event already has
{ "type": "event",
  "data": { "id": "event-41", "sequence": 41, "event": "tool_use",
            "created_at": "…", "summary": "Read bridge/src/app.rs" } }
```

The class is **not** a field. It is intrinsic to the kind and asked for through
`ThreadEventKind::class` / `ThreadItem::attention_reason`, which is how the
existing 32 kinds work — serializing it would be a second copy of an answer the
kind already gives, free to drift from it. What a client reads off the wire is
the kind; what the kind means for attention is decided in one place.

`agent.attach` gains a typed refusal for a session with no terminal, so an old
client asking gets a sentence rather than a hang. This follows the precedent set
by `require_shell_kind` (`app.rs:94`): refuse loudly and say where the thing
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
5a. **Prep: the daemon speaks `AgentSession` only.** `Tab.session` becomes
   `Box<dyn AgentSession>`, every remaining daemon call on the wider trait
   moves behind `AgentSession` / `TerminalView` or into `PtySession`, the
   `HarnessSession: AgentSession` supertrait bridge is dropped, and
   `has_terminal` for a not-yet-started agent comes from the provider. Pure
   refactor: the wire is unchanged and every behaviour byte-identical. Detail
   below.
6. **Then, and only then, add a provider with no terminal** — the ADK, as
   `bridge/src/harness/adk.rs`. By this point it is a new file, not a
   migration. Detail below.
7. **Outcomes become message statuses** — the §6.1 deferred fix, as designed
   there. Independent of step 6 (it repairs the catch-up packet for every
   carrier) and ordered after it only because step 6 is what makes the gap
   bite daily; it may land first if ADK slips.

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
| `write_prompt` | `deliver` (`app.rs:17249`), `nudge_live_agent_tab` (`app.rs:16009`) | `send_turn(Turn)` — the value the trait has carried since step 1 |
| `ready_within` | `ensure_agent_tab`, once, after `Tab::spawn` | into the PTY arm of the carrier-choosing spawn (`open_session`, `harness/mod.rs`) — readiness is how a *terminal* opens, still run outside the state lock |
| `idle_for` | `mark_idle_tasks` (`app.rs:5815`), `agent_last_painted_at` (`app.rs:13450`) | `quiet_for()` (§3) — PTY answer unchanged |
| `exited_within` | `deliver` (`app.rs:17257`) | `exited_within()` (§3) |
| `has_exited` | 9 sites (`agent_is_live`, `term_input`/`term_resize` liveness, the idle sweep, `agent_digest`, single-agent guards, the nudge) | `matches!(status(), AgentStatus::Ended { .. })` — the enum already says it |
| `exit_code` | idle sweep (`app.rs:5810`) | the code inside `AgentStatus::Ended` |
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
  (`app.rs:7860`) asks `harness_for(agent.choice.provider)` when there is no
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
  (`spawn_tab_pump`, `app.rs:17352`) is the precedent: a task spawned beside
  the tab that owns the session's output and, on stream close, marks the tab
  dead and closes the conversation's session lineage
  (`record_agent_session_end`, `app.rs:2888`). Step 6 adds the second pump
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

- **The MCP socket wiring is identical, by construction.** The argv carries
  the same per-agent `--mcp-config` the orchestrator scaffolds today, the
  env carries the same `BRIDGE_MCP_SOCKET` / `BRIDGE_MCP_TOKEN`, and `done` /
  `post_thread_message` / `read_unread_messages` / `search_conversation`
  arrive over the same unix socket. §2's dividend cashes out: the from-agent
  half of the interface needs zero work.

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

---

## 11. Decisions needed before step 1

1. ~~**How large is the thread's recent activity window?**~~ Answered by the
   store migration: a conversation keeps a resident tail of 200 items in memory
   (`RESIDENT_CONVERSATION_TAIL`, `store.rs:477`), first loads and scroll-back
   ship 60 items per page (`DEFAULT_THREAD_PAGE`, `thread.rs:961`), and older
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
   - **Adoption** (`run_adopt`, `app.rs:11788` → `Orchestrator::adopt_run`,
     `orchestrator.rs:2364`; `adopt_implementation`, `orchestrator.rs:1474`)
     is git and records: checkpoint commit, `.build` scaffold, roster,
     lifecycle events. No session exists at adoption and none is consulted.
   - **The primary-checkout super-worktree** (`run_adopt` with
     `primary: true`, `describe_primary_checkout` in `worktree.rs:595`, the
     `owns_primary_checkout` / `primary_run_of` guards, `app.rs:6373`) is
     derived ownership over a directory; its lifecycle guards never touch a
     session.
   - **The drop-in itself** (`deliver` → `ensure_agent_tab`,
     `app.rs:17017`) is the first place a carrier exists, chosen at
     `open_session` — everything before it (`scaffold_agent_worktree`, the
     per-provider `has_transcript` probe, the session-token mint) is
     path-and-provider work that holds for a headless child with the same
     cwd.
   The one PTY-flavoured residue found is `agent_last_painted_at`
   (`app.rs:13443`) reading the paint clock for external worktree cards;
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
   for the idle sweep (`mark_idle_tasks`, `app.rs:5784`, threshold
   `BRIDGE_IDLE_SECONDS`, default 300 s; the demotions land through
   `on_plan_idle` / `on_run_idle`, `orchestrator.rs:1072` / `1998`):
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

---

## 12. Revision history

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

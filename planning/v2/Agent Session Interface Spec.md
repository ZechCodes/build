# Agent Session Interface — Spec

**Status:** Draft — steps 0–3 shipped, steps 4–6 not started (see §10)
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

`Thread::catch_up_markdown` (`bridge/src/thread.rs:2237`) currently takes the
last N items by recency, N = 40, filtered only for completion messages:

```rust
for item in self.items.iter().rev().take(limit).rev() { … }
```

Nothing about class. A session that emitted forty tool calls before restarting
would hand its replacement forty tool calls and **none of the human's messages**
— the exact context the packet exists to carry. Filtering to messages fixes that
by construction rather than by tuning a ratio, and it does not need revisiting
when a fifth activity kind is added later.

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
outcomes reported through `done`: `post_completion` (`thread.rs:1583`)
deliberately posts no message — the event is the only record — and
`last_completion` is only set when a structured report exists, which a blocked
`done` does not carry. So a messages-only packet tells a replacement agent
nothing about why its predecessor blocked. This spec stays a refactor and does
not fix that here. The fix decided for later: an outcome is a **status attached
to a message**, not a separate event. A blocked agent sends a message carrying a
blocked status; the event-only record in `post_completion` is replaced by that
message, and the messages-only packet then carries outcomes by construction,
with no event lines re-admitted.

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
| `catch_up_markdown` (`thread.rs:2237`) | last 40 items by recency, events included | messages only (§6.1) |
| `Thread::items` (`thread.rs:1167`) | resident tail of 200 items, older items paged from SQLite | unchanged — activity rows ride the same tail and pages |

### SPA

| Site | Today | Change |
|---|---|---|
| `agentRail.js:177` | Chat / TUI switch | **TUI button shown only when `has_terminal`** |
| thread rendering (`core/thread.js`) | messages + lifecycle events | renders the four activity kinds; folded by default |
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
// thread items — four new event kinds, class "status"
{ "kind": "event", "event": "tool_use", "class": "status",
  "sequence": 41, "summary": "Read bridge/src/app.rs" }
```

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
> Step 4's dependency on the store is discharged — the store migration shipped
> (`Store Migration Spec.md`), so activity kinds land as ordinary thread rows
> and the jsonl log in §6.2 is not needed. Step 4 is also cheaper than specced:
> paging already exists, so nothing has to be built to keep a long conversation
> off the wire.

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
4. **Add the four activity kinds**, classed `Status`, *with* the
   messages-only catch-up packet fix (§6.1) in the same change. Nothing emits
   them yet. Persistence needs no work: an activity item is an ordinary thread
   row, appended as one `INSERT` and paged like every other item — the jsonl
   log §6.2 designed is superseded by the store migration. The catch-up fix
   must not be split out: it exists precisely because the kinds do.
5. **SPA: hide the TUI button when `has_terminal` is false**, and render the four
   kinds in the thread, folded by default.
6. **Then, and only then, add a provider with no terminal.** By this point it is
   a new file, not a migration.

Steps 1–5 add no providers and change no behaviour. If ADK slips they are still
worth having: step 2 alone removes "quiet for 30 seconds" from being the only
thing Build can say about an agent, and step 4's class fix is a latent bug in the
catch-up packet regardless of who fills the thread.

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

3. **Does a no-terminal agent still get a worktree?** ADK and the app server both
   operate on files, so yes — but it is worth stating, because "agent" and
   "worktree with a PTY in it" have been the same thing until now. Adoption,
   `run.adopt` and the primary-checkout super-worktree all assume an agent can be
   dropped into.

4. **What does `done` mean when the harness reports turn boundaries?** Today
   `done` is the only completion signal and quiescence is the fallback ("silence
   is an anomaly, never completion"). With real turn boundaries the fallback
   could become precise — but `done` carries the structured report, so it should
   stay the contract and turn boundaries should only sharpen idle detection.

---

## 12. Revision history

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

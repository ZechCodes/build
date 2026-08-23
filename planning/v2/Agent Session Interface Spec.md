# Agent Session Interface — Spec

**Status:** Draft — step 0 shipped, steps 1–6 not started (see §10)
**Last updated:** August 22, 2026
**Branch:** off `main`

---

## 1. What this is

Build currently has one kind of agent: a CLI wrapper in a full PTY. Claude's ADK
and Codex's app server are not that. They expose a **session protocol** — an
event stream of reasoning, tool uses and messages — with no terminal anywhere in
it.

The `Harness` / `HarnessSession` split already on this branch made the *launch*
polymorphic and put the session behind a trait. It did not make the terminal
optional: `HarnessSession` still requires `subscribe() -> bytes`, `resize`,
`write_input` and `pid`, because `PtySession` was the only implementation there
has ever been.

This spec defines the interface that sits between the UI and the harness when
the byte stream is set aside, and says what happens to every surface that
currently assumes one exists.

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

`agent_is_working` (`bridge/src/app.rs:893`) is four conjuncts:

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

## 6. What activity in the conversation costs

Rewritten 2026-08-23. This section said "the two things this breaks" and named
the catch-up packet and an unbounded thread. The store work answered the second
and changed the shape of the first — and created a third that could not have
been foreseen when a conversation was a `Vec` that was always entirely present.

### 6.1 Activity floods the resident tail — the new one

A conversation is no longer fully resident. A boot reads the newest
`RESIDENT_CONVERSATION_TAIL` items (200, `store.rs:477`) and remembers how many
it left behind; everything older is read back from the database on demand.

Lifecycle events arrive a handful per run, so a 200-item tail is comfortably a
conversation's working set today. **Activity is not like that.** A single build
session emits hundreds of tool calls. Put those in the conversation and the
resident tail becomes *entirely activity* — every message the human and the
agent exchanged pushed out of memory by the agent narrating its own tool use.

Two things break the moment that happens, and both are silent:

- **The catch-up packet empties.** `catch_up_markdown` (`thread.rs:2237`) reads
  `self.items` — the tail. §6.2 below filters it to messages, which is right,
  but filtering a tail that holds no messages yields **nothing**. A resumed
  agent would be handed an empty conversation and told to carry on.
- **The first page shows no conversation.** `DEFAULT_THREAD_PAGE` is 60
  (`thread.rs:961`). Opening a conversation would paint 60 tool calls, with the
  last thing anyone actually said somewhere below them.

**So activity must not be counted against either bound.** Concretely:

1. The catch-up packet is built from a **store query for messages**, not from
   the resident tail. The database already distinguishes them —
   `thread_items.attention` is set at write time, and every activity kind is
   `Status` — so this is a `WHERE`, not a scan.
2. The page the client opens on is measured in **conversation**, not items:
   activity between two messages travels with them and is folded, rather than
   consuming the budget that decides how far back the human can see.

This is the one genuinely new requirement the store work creates, and it is why
step 4 is bigger than "add four enum variants".

### 6.2 The catch-up packet is messages only

**Decided 2026-08-20. Not implemented** — `catch_up_markdown` still takes the
last 40 items by recency and includes any event carrying a summary. It is part
of step 4, not a regression.

The packet a resumed agent is handed carries **only messages to and from the
agent**. Filtering by class would have worked and needed revisiting the next
time a kind was added; filtering to messages is right by construction.

What it drops, deliberately: in practice four places attach a summary to an
event — `Done`, `Blocked`/`ReviewBlocked`, `RunFailed`/`IdleUnreported`/crash
reasons, and revision/approval. Those are Build's observations *about* the
agent rather than the conversation, and the structured completion report is
appended separately by `conversation_prompt` (`orchestrator.rs:644`), so the
densest of them survives regardless.

Read with §6.1: messages-only is necessary and not sufficient. The filter has
to run over a query, not over the tail.

### 6.3 Storage — answered by the store work

This section argued a per-agent jsonl log, because appending to the JSON store
rewrote every conversation on the Issue. That store is gone. A thread item is a
row, an append is one `INSERT`, a page is a `LIMIT`, and the unread badge is a
`COUNT` over an indexed `attention` column. See `Store Migration Spec.md`.

**Nothing extra is needed to store activity.** It is rows, like everything else
on a conversation, and the badge is already correct for it: activity is `Status`
class, so it writes `attention = 0` and never counts toward what the human is
being called to. That much of step 4 is free.

## 7. Terminal-coupled surfaces — the inventory

Everything that must become conditional. Line numbers re-checked 2026-08-23
against `main`.

### Bridge

| Site | Today | Change |
|---|---|---|
| `Tab.session` (`app.rs:652`) | `Box<dyn HarnessSession>` | `Box<dyn AgentSession>` |
| `Tab.screen` (`app.rs:653`) | always a `TermScreen` | `Option<TermScreen>` — no grid without a terminal |
| `agent_is_working` (`app.rs:893`) | reads `idle_for` | reads `status()` |
| `deliver` (`app.rs:17132`) | ends in `write_prompt` | ends in `send_turn` |
| `agent_attach` (`app.rs`) | attaches a grid, defaults 40×120 | refuses, with a reason, for a session with no terminal |
| `term.input` / `term.resize` | assume a PTY | refuse for an agent tab with no terminal |
| `spawn_tab_pump` | pumps bytes into `TermScreen` | with no terminal, posts activity to the conversation instead |
| `agent_digest` (`app.rs:7751`) | `"working": bool` | add `"has_terminal": bool`; keep `working` |
| `catch_up_markdown` (`thread.rs:2237`) | last 40 items of the tail, events included | messages, from a store query (§6.1, §6.2) |
| `DEFAULT_THREAD_PAGE` (`thread.rs:961`) | 60 items | 60 units of conversation; activity rides along (§6.1) |

### SPA

| Site | Today | Change |
|---|---|---|
| `agentRail.js:185` | Chat / TUI switch | **TUI button shown only when `has_terminal`** |
| thread rendering (`core/thread.js`) | messages + lifecycle events | renders the four activity kinds, folded by default |
| `surfaceTabs.js` | mounts the agent's PTY pane | unchanged — simply not reached for a no-terminal agent |
| `terminal/manager.js` | one shared socket, demuxed by `term_id` | unchanged |
| `console.js` | the human's own shells | unchanged; the console was never the agent's |

Two are **explicitly unchanged**, because the first draft of this spec had them
changing: `surfaceTabs.js` mounts a PTY pane and a no-terminal agent never asks
it to, so there is nothing to make conditional — the rail just does not offer
the button. And the console hosts the human's own shells in the checkout; a
worktree still has terminals even when its agent does not.

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

Paging **older** activity out of the jsonl log (§6.2) is a separate, ordinary
read — a client asking for history it has scrolled back to, not a client
resyncing. It has no bearing on reconnect, which only ever needs the recent
window the thread already carries.

---

## 10. Migration order

Each step compiles, ships and is green on its own.

> **Where this stands, 2026-08-22.** Step 0 — the `Harness` trait, one
> implementation per provider, reached only through `harness_for` — shipped in
> `a26acd2` (on `main`) and is documented in `Harness Refactor.md`. That is the
> launch side.
> **None of steps 1–6 below have been started**: the session side is still
> `HarnessSession`, which every implementation must satisfy including the four
> terminal calls, so a provider with no terminal still cannot exist.
>
> Step 4's dependency on the store is discharged — the store migration shipped
> (`Store Migration Spec.md`), so activity kinds land as ordinary thread rows
> and the jsonl log in §6.2 is not needed. Step 4 is also cheaper than specced:
> paging already exists, so nothing has to be built to keep a long conversation
> off the wire.

1. **Introduce `AgentSession` + `TerminalView`**; `PtySession` implements both,
   `terminal()` returns `Some(self)`. Nothing is optional yet. No behaviour
   change.
2. **Move status behind `status()`.** `agent_is_working` reads the enum; the PTY
   implementation synthesizes it from `idle_for`. The wire is unchanged.
3. **Make `Tab.screen` an `Option`**, add `has_terminal` to the agent digest, and
   add the typed refusals to `agent_attach` / `term.input` / `term.resize`. No
   session returns `None` yet — the paths are dead but exercised by tests.
4. **Add the four activity kinds**, classed `Status`. Storage is free (§6.3),
   but two things must land in the same change or the kinds break what is
   already working:
   - the catch-up packet becomes a **store query for messages** (§6.1, §6.2) —
     filtering the resident tail is not enough once activity can fill it;
   - the page the client opens on stops counting activity against its budget
     (§6.1), or opening a conversation paints tool calls instead of it.

   Nothing emits the kinds yet. This is the step that must not be split.
5. **SPA: hide the TUI button when `has_terminal` is false**, and render the four
   kinds in the thread, folded by default.
6. **Then, and only then, add a provider with no terminal.** By this point it is
   a new file, not a migration.

Steps 1–5 add no providers and change no behaviour. If ADK slips they are still
worth having: step 2 alone removes "quiet for 30 seconds" from being the only
thing Build can say about an agent, and step 4's class fix is a latent bug in the
catch-up packet regardless of who fills the thread.

---

## 11. Decisions still open

Re-checked 2026-08-23. The storage question is answered; three remain, and one
is new.

1. **Does `Turn` carry structure, or stay a string?** A PTY can only take text.
   ADK can take structured content — attachments, images, tool results. Making
   `Turn` a struct now costs little; making it one later touches every caller of
   `deliver`. **Gates step 1.**

2. **How is a page measured once activity is in it?** §6.1 says activity must
   not consume the budget that decides how far back a human can see, but not
   what replaces it — 60 messages with their activity folded beneath, or a
   separate activity budget per message. **Gates step 4**, and it is the one
   with a visible consequence: get it wrong and opening a conversation shows
   tool calls where the conversation should be. *(New — the resident tail and
   the 60-item page did not exist when this spec was written.)*

3. **Does a no-terminal agent still get a worktree?** ADK and the app server
   both operate on files, so yes — but it is worth stating, because "agent" and
   "worktree with a PTY in it" have been the same thing until now. Adoption,
   `run.adopt` and the primary-checkout super-worktree all assume an agent can
   be dropped into. **Gates step 6.**

4. **What does `done` mean when the harness reports turn boundaries?** Today
   `done` is the only completion signal and quiescence is the fallback ("silence
   is an anomaly, never completion"). With real turn boundaries the fallback
   could become precise — but `done` carries the structured report, so it should
   stay the contract and turn boundaries should only sharpen idle detection.
   **Gates step 6.**

**Answered, and recorded so it is not reopened:** activity storage. It is rows
on `thread_items` like everything else, the badge is already correct because
activity is `Status` class, and no second store is needed. See §6.3.

## 12. Revision history

- **2026-08-20, first draft.** Proposed a separate `AgentEvent` stream with its
  own ring, cursor and `agent.events` RPC, and a second rail mode named by
  capability (`TUI` or `Activity`).
- **2026-08-20, revised.** Activity moves into the conversation as typed
  `Status` events; the second tab disappears for event-stream harnesses and the
  terminal becomes purely an escape hatch for opaque CLI wrappers. Removed the
  event ring, the cursor protocol and the RPC. Added §6 — the catch-up packet
  and thread-retention consequences the move creates.
- **2026-08-23, reconciled with the shipped store.** §6 rewritten: the storage
  problem is answered, the catch-up decision is unimplemented rather than done,
  and a new one appeared that this spec could not have foreseen — a conversation
  is no longer fully resident, so activity can flood the 200-item tail and empty
  both the catch-up packet and the first page. §7 line numbers re-checked
  against `main`. §11 went from four open decisions to three plus one new.
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

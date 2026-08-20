# Agent Session Interface — Spec

**Status:** Draft for review — revised, see §12
**Last updated:** August 20, 2026
**Branch:** `build/agent-polymorphism`

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

`deliver` (`bridge/src/app.rs:16800`) is documented as "the one pipe from Build
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

## 6. The two things this breaks

Both are in code that predates agent activity being conversational, and both must
be fixed in the same change that introduces the new kinds.

### 6.1 The catch-up packet would evict the human

`Thread::catch_up_markdown` (`bridge/src/thread.rs:1975`) is what a resumed or
cold-started agent is handed as its context, through `conversation_prompt`. It
takes the **last N items by recency**, N = 40, filtering only completion
messages:

```rust
for item in self.items.iter().rev().take(limit).rev() { … }
```

Nothing about class. A session that emitted forty tool calls before restarting
would hand its replacement forty tool calls and **none of the human's messages**
— the exact context the packet exists to carry.

**Fix:** `catch_up_markdown` selects by class, not by recency alone. Messages and
`Attention` events are the packet; `Status` events fill what is left, if
anything. Small change, well covered by tests — but it lands with the new kinds,
not after them.

### 6.2 Threads are unbounded and persisted whole

`Thread::items` is a `Vec` with no cap, no retention rule and no trimming. It is
serialized in full to the store, and `thread.revision` reports `item_count` /
`thread_total` over it.

Lifecycle events arrive a handful per run. Tool-use events arrive at agent speed
— hundreds per session is ordinary. That is a change in the *rate* of growth by
two or three orders of magnitude, against a structure that was never bounded
because it never needed to be.

"Activity lives in the conversation" answers **where** scrollback is kept. It
does not answer **how much**, and that question is now load-bearing. Options,
cheapest first:

1. **Cap per session.** Keep the last N activity events per session lineage and
   drop older ones when the session ends. Messages and lifecycle events are never
   dropped. Bounded, simple, and matches what scrollback always did.
2. **Drop activity on session end.** Activity is live-only; the conversation
   keeps messages and lifecycle. Closest to today's PTY behaviour, and gives up
   reviewing what an agent did after the fact.
3. **Keep everything.** Simplest to build, unbounded on disk and on the wire. A
   long-lived issue's thread eventually becomes too big to ship.

Recommendation: **(1)**. It keeps post-hoc review — the reason to put activity in
the conversation at all — without an unbounded record.

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
| `catch_up_markdown` (`thread.rs:1975`) | last 40 items by recency | selects by class (§6.1) |
| `Thread::items` (`thread.rs`) | unbounded `Vec`, persisted whole | retention rule for activity kinds (§6.2) |

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
| Conversation (activity included) | thread revision | item sequence |

There is no third row. Activity reconnects the way the conversation does because
it *is* the conversation, which is the single largest thing this design buys: no
new cursor protocol, no new snapshot, no new flow control, and no live-only
window that a reconnecting client can fall out of.

---

## 10. Migration order

Each step compiles, ships and is green on its own.

1. **Introduce `AgentSession` + `TerminalView`**; `PtySession` implements both,
   `terminal()` returns `Some(self)`. Nothing is optional yet. No behaviour
   change.
2. **Move status behind `status()`.** `agent_is_working` reads the enum; the PTY
   implementation synthesizes it from `idle_for`. The wire is unchanged.
3. **Make `Tab.screen` an `Option`**, add `has_terminal` to the agent digest, and
   add the typed refusals to `agent_attach` / `term.input` / `term.resize`. No
   session returns `None` yet — the paths are dead but exercised by tests.
4. **Add the four activity kinds**, classed `Status`, *with* the `catch_up_markdown`
   class fix (§6.1) and the retention rule (§6.2) in the same change. Nothing
   emits them yet. This is the step that must not be split: the two fixes exist
   precisely because the kinds do.
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

1. **What is the retention rule for activity?** §6.2, options 1–3. The
   recommendation is a per-session cap. This one gates step 4 and nothing else,
   but step 4 cannot start without it.

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

- **2026-08-20, first draft.** Proposed a separate `AgentEvent` stream with its
  own ring, cursor and `agent.events` RPC, and a second rail mode named by
  capability (`TUI` or `Activity`).
- **2026-08-20, revised.** Activity moves into the conversation as typed
  `Status` events; the second tab disappears for event-stream harnesses and the
  terminal becomes purely an escape hatch for opaque CLI wrappers. Removed the
  event ring, the cursor protocol and the RPC. Added §6 — the catch-up packet
  and thread-retention consequences the move creates.

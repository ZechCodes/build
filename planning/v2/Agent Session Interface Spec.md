# Agent Session Interface — Spec

**Status:** Draft for review
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

    /// The agent's activity, from `after` onward. Cursor-based, like the
    /// conversation: a reconnecting client asks for what it has not seen.
    fn events(&self, after: EventCursor) -> broadcast::Receiver<AgentEvent>;

    /// End the session and release whatever it holds.
    fn end(&self);

    /// The terminal, if this session has one. `None` is a normal answer.
    fn terminal(&self) -> Option<&dyn TerminalView> { None }
}

/// A session the human can drop into. Only a PTY-backed session offers this.
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

### What happens to `HarnessSession`

It becomes `PtySession`'s business alone. `write_prompt`, `ready_within` and
`idle_for` stop being the daemon's vocabulary and become how the PTY
implementation *satisfies* `AgentSession`:

| `AgentSession` | PTY implementation | ADK / app server |
|---|---|---|
| `send_turn` | `ready_within` → framed paste → delayed submit | one protocol call |
| `status` | synthesized from `idle_for` (today's 30 s rule, unchanged) | reported turn boundaries |
| `events` | synthesized: `Started` / `Exited` only | reasoning, tool use, message, turn boundaries |
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

## 5. `AgentEvent`

The vocabulary an event-stream harness reports and a PTY harness mostly cannot.

```rust
pub enum AgentEvent {
    /// A session began. Carries the lineage the conversation records.
    Started { session_id: String, cold: bool },
    /// A turn began — the authoritative start of Working.
    TurnBegan { turn_id: String },
    /// The agent thought. Text, possibly streamed in parts.
    Reasoning { turn_id: String, text: String },
    /// The agent called a tool.
    ToolUse { turn_id: String, name: String, input: Value, id: String },
    /// A tool answered.
    ToolResult { turn_id: String, id: String, ok: bool, output: Value },
    /// The agent produced a message for the human. Distinct from
    /// `post_thread_message`, which is the agent choosing to be durable.
    Message { turn_id: String, text: String },
    /// A turn ended — the authoritative end of Working.
    TurnEnded { turn_id: String },
    /// The session is over.
    Exited { code: Option<i32> },
}
```

### Events are not the conversation

This is the distinction the doc exists to fix in advance, because getting it
wrong would quietly destroy the review product.

- The **conversation** (`Thread`) is what the human and the agent *said to each
  other on purpose*. It is durable, it is what a replacement session is handed
  as its catch-up packet, and it is what the reviewer reads.
- The **event stream** is what the agent *did*. It is the execution log. It is
  the direct replacement for the CLI tab, and it has exactly the CLI tab's
  status: always available, never the default view.

An agent's reasoning must not become thread messages. The scope doc already
settled this for the terminal — "the terminal remains the execution log; the
dedicated Conversation tab carries only durable user/agent messages and
lifecycle statuses" — and the event stream inherits that ruling unchanged.

### Retention

Events are **not durable across a daemon restart**, exactly as PTY scrollback is
not. `TermScreen` keeps a live vt100 model and a byte cursor; the event stream
keeps a bounded ring of recent events and a sequence cursor. Neither survives the
bridge going away, and neither needs to: the conversation does.

---

## 6. What replaces the CLI tab

**The rail already has the shape for this.** The agent rail's conversation panel
is a two-mode switch today (`spa/src/core/agentRail.js:177`):

```
[ Chat ] [ TUI ]
```

`Chat` is the conversation and the composer; `TUI` swaps the same panel onto that
agent's PTY. The change is that the second button is named by what the session
offers:

```
[ Chat ] [ TUI ]        — a session with a terminal
[ Chat ] [ Activity ]   — a session with an event stream
```

Same switch, same panel, same per-agent memory of which mode you left it in
(`panelModes`). `Activity` renders the event stream as a running list: reasoning
folded by default, tool uses as one line each with their result, messages inline.

This is the smallest possible UI story, and it holds design rule 3 — *the
terminal is the basement: always accessible, never the default view* — for a
harness that has no terminal. The basement is still there; it is a different
staircase.

### What an event-stream agent loses, and whether it matters

| Lost | Matters? |
|---|---|
| Typing directly at the agent (`write_input`) | The composer is the input path. Dropping into a TUI to type is a CLI-wrapper affordance. |
| Resize | Meaningless without a grid. |
| The PID | Used for diagnostics and kill-by-pid; the session's own `end()` covers the real need. |
| Byte-exact scrollback | Replaced by the event log, which is *more* legible, not less. |

None of these is load-bearing for the product's four design rules.

---

## 7. Terminal-coupled surfaces — the inventory

Everything that must become conditional. This is the actual size of the work.

### Bridge

| Site | Today | Change |
|---|---|---|
| `Tab.session` (`app.rs:652`) | `Box<dyn HarnessSession>` | `Box<dyn AgentSession>` |
| `Tab.screen` (`app.rs:653`) | always a `TermScreen` | `Option<TermScreen>` — no grid without a terminal |
| `agent_is_working` (`app.rs:893`) | reads `idle_for` | reads `status()` |
| `agent_attach` (`app.rs`) | attaches a grid, defaults 40×120 | must refuse, with a reason, for a session with no terminal |
| `term.input` / `term.resize` | assume a PTY | must refuse for an agent tab with no terminal |
| `spawn_tab_pump` | pumps bytes into `TermScreen` | pumps events into the event ring when there is no terminal |
| `agent_digest` (`app.rs:7601`) | `"working": bool` | add `"has_terminal": bool`; keep `working` |

### SPA

| Site | Today | Change |
|---|---|---|
| `agentRail.js:177` | Chat / TUI switch | second mode named by capability |
| `surfaceTabs.js` | mounts the agent's PTY pane | mounts pane **or** activity list |
| `terminal/manager.js` | one shared socket, demuxed by `term_id` | unchanged — events ride the same socket |
| `console.js` | the human's own shells | **unchanged**; the console was never the agent's |

The console is worth calling out as explicitly out of scope: it hosts the human's
shells in the checkout, and an agent — of any kind — was never one of them. A
worktree still has terminals even when its agent does not.

---

## 8. Wire contract

Additive. No field changes meaning.

```json
// agent digest — one new field
{ "id": "agent-…", "working": true, "has_terminal": true, … }
```

```json
// agent.events — new, cursor-shaped like thread.revision
→ { "method": "agent.events", "params": { "agent_id": "agent-…", "after": 0 } }
← { "events": [ … ], "cursor": 41 }
```

`agent.attach` gains a typed refusal for a session with no terminal, so an old
client asking gets a sentence rather than a hang. This follows the precedent set
by `require_shell_kind` (`app.rs:94`): refuse loudly and say where the thing
actually lives, never fall back to something different.

---

## 9. Reconnect

The existing rule is snapshot-plus-cursor per surface, never replay-from-zero. It
carries over directly:

| Surface | Snapshot | Cursor |
|---|---|---|
| Terminal | vt100 screen + byte total | `term.ack` flow control |
| Events | last N events in the ring | event sequence |
| Conversation | thread revision | item sequence |

The event stream is closer to the conversation than to the terminal here — it is
a sequence of discrete items with monotonic numbering, which is what
`ThreadMessage.sequence` / `updated_sequence` already implement. **Reuse that
protocol rather than inventing a second one.**

---

## 10. Migration order

Each step compiles, ships and is green on its own.

1. **Introduce `AgentSession` + `TerminalView`**; `PtySession` implements both,
   `terminal()` returns `Some(self)`. Nothing is optional yet. No behaviour
   change.
2. **Move status behind `status()`.** `agent_is_working` reads the enum; the PTY
   implementation synthesizes it from `idle_for`. The wire is unchanged.
3. **Make `Tab.screen` an `Option`** and add the typed refusals to
   `agent_attach` / `term.input` / `term.resize`. Still no session actually
   returns `None` — the paths are dead but exercised by tests.
4. **Add `AgentEvent` + the event ring + `agent.events`.** The PTY
   implementation emits `Started` / `Exited` only, which is honest.
5. **SPA: name the second rail mode by capability** and build the activity list
   against the PTY's two-event stream.
6. **Then, and only then, add a provider with no terminal.** By this point it is
   a new file, not a migration.

Steps 1–5 add no providers and change no behaviour. If ADK slips, they are still
worth having: step 2 alone removes the "quiet for 30 s means idle" guess from
being the only thing Build can say about an agent.

---

## 11. Decisions needed before step 1

1. **Does `Turn` carry structure, or stay a string?** A PTY can only take text.
   ADK can take structured content (attachments, images, tool results). Making
   `Turn` a struct now costs little; making it one later touches every caller of
   `deliver`.

2. **Is `Activity` scrollback-shaped or turn-shaped?** A flat running list is
   closer to the TUI it replaces; a list grouped by turn is more legible and more
   work. I lean turn-shaped, because turn boundaries are the thing the event
   stream knows and the terminal never did.

3. **Does a no-terminal agent still get a worktree?** ADK and the app server both
   operate on files, so yes — but it is worth stating, because "agent" and
   "worktree with a PTY in it" have been the same thing until now. Adoption,
   `run.adopt` and the primary-checkout super-worktree all assume an agent can be
   dropped into.

4. **What does `done` mean when the harness reports turn boundaries?** Today
   `done` is the only completion signal and quiescence is the fallback
   ("silence is an anomaly, never completion"). With real `TurnEnded` events, the
   fallback could become precise — but `done` carries the structured report, so it
   should stay the contract and `TurnEnded` should only sharpen idle detection.

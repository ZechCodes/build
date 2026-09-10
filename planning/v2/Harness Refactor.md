# Harness Refactor — what shipped

**Status:** Shipped — on `main` (`a26acd2`, merged as `d2c3242`)
**Last updated:** August 22, 2026
**Next:** `Agent Session Interface Spec.md` — the session side, not started

---

## 1. The question this answered

*Is there a polymorphic interface to each of the agent harnesses, so all logic
about an agent lives behind a well-defined API before ADK and the Codex app
server are added?*

The answer was **partly**. There was a seam, but it covered only **launch**, and
provider knowledge had leaked into five places that each matched on
`AgentProvider`:

| Where | What it decided |
|---|---|
| `build_agent` (`app.rs`) | ~85 lines of Claude-vs-Codex argv inline |
| `ModelChoice::harness_args` | effort → flag mapping, matched again |
| `default_transcript_probe` | a *second*, separate provider dispatch for resume |
| `mcp_tool_names` | existed only because Codex wants an allow-list up front |
| `pre_trust_worktree_for_claude` | a Claude-only filesystem side effect |

Plus hardcoded `"claude"` / `"codex"` strings in the RPC param parse and the
terminal-kind guard, and the same pair again in the SPA.

Two more of the same kind were folded in beyond the original five —
`ModelChoice::validate` and `provider_catalogs` — because leaving them meant a
new provider would still have to edit `models.rs`.

---

## 2. What the interface is

```rust
/// Everything Build knows about one coding-agent provider.
pub trait Harness: Send + Sync {
    fn provider(&self) -> AgentProvider;
    fn label(&self) -> &'static str;
    fn models(&self) -> Vec<ModelOption>;
    fn effort_levels(&self) -> &'static [&'static str];
    fn model_args(&self, choice: &ModelChoice) -> Vec<String>;
    fn spec(&self, choice: &ModelChoice, options: &SpawnOptions,
            context: &HarnessContext) -> HarnessSpec;
    fn prepare_workspace(&self, cwd: &Path) { }        // default: nothing
    fn has_transcript(&self, home: &Path, cwd: &Path) -> bool;
}

pub fn harness_for(provider: AgentProvider) -> &'static dyn Harness
```

`harness_for` is the only way to reach one. Nothing above it matches on a
provider.

### Layout

```
bridge/src/harness/
  mod.rs      Harness, HarnessContext, harness_for, open_session,
              REAL_TUI_SETTLE / REAL_TUI_SUBMIT_DELAY / INHERITED_AGENT_MARKERS
  session.rs  HarnessSession (the running side) + HarnessError
  claude.rs   ClaudeHarness — catalog, --effort, argv, workspace pre-trust, probe
  codex.rs    CodexHarness — catalog, model_reasoning_effort, argv, tool
              allow-list, rollout probe
bridge/src/pty.rs   PtySession: the subprocess implementation of HarnessSession
```

`open_session` in `mod.rs` is the single place a launch description becomes a
running agent, so a carrier that is not a subprocess is chosen there and nowhere
in `app.rs`.

### Adding a provider

Add an `AgentProvider` variant, add a module, add an arm to `harness_for`. The
compiler finds the rest — `AgentProvider::ALL` is the one enumeration, and
`wire_id` / `from_wire` replaced every hardcoded id string.

---

## 3. The asymmetry worth knowing

This is the finding that shaped everything after it, and it makes the remaining
work smaller than it looks.

**From the agent: already transport-free.** `post_thread_message`, `done`,
`read_unread_messages` and `search_conversation` all arrive over the MCP unix
socket. Build has *never* parsed PTY bytes to learn what an agent said — the
scope doc's "no scraping TUI output for state" rule was honoured, and the
dividend is that this half needs no work at all. An ADK session calls the same
tools over the same socket.

**To the agent: one chokepoint, no interface.** `deliver` (`app.rs`) is the one
pipe, but it ends in `write_prompt` — bracketed-paste framing plus a submit key
written 1500 ms later. Keystroke mechanics.

**Status: inferred, and the inference is wrong for an event stream.**
`agent_is_working` is four conjuncts, three of which read the PTY and the last
of which is a *guess*: painted within 30 s means working. That is the best a
terminal can do, and it is actively wrong for a session protocol — a model
reasoning for 45 seconds with no output reads as "waiting for you", and the rail
dot goes dark mid-turn.

---

## 4. Decisions

- **`PtyError` → `HarnessError`** (`Session` / `Io` / `NotFound`), and
  `OrchestratorError::Pty` → `::Harness`. An ADK session returning
  `PtyError::Pty(..)` would be a lie. 16 sites.
- **`models.list` keeps its top-level `models` / `efforts`** — now the default
  provider's catalog. The SPA falls back to them, and a wire change is outside a
  no-behaviour-change refactor.
- **Provider-specific tests moved to the module that owns the code** (the trust
  registry, the transcript probes). The argv tests stayed in `app.rs` because
  they exercise `build_agent`'s wiring *through* the seam — which is what would
  actually break.

## 5. What it did not do

- **No new providers.** This change adds none.
- **`HarnessSpec` is still exec-argv shaped** — binary, args, env. A
  non-subprocess provider needs a second launch shape beside it; left until
  there is a real one to shape it against.
- **The session side is untouched.** `HarnessSession` still requires
  `subscribe() -> bytes`, `write_input`, `resize` and `pid` of every
  implementation, so a provider with no terminal still cannot exist. That is
  what `Agent Session Interface Spec.md` is for, and none of it is started.
- **The SPA still hardcodes provider ids and labels** in
  `core/modelPicker.js` and `core/thread.js`. Those are fallbacks for a bridge
  predating `providers`; the bridge now serves the full list, so they can go.

## 6. Verification

1120 tests (1093 lib + 27 integration), up from 1087 at baseline, zero failures.
Behaviour parity rests on that suite: the argv contract tests assert binary,
model and effort flags, `--continue` placement, MCP wiring, and the paste-burst
and trust overrides, so a silent argv change would have failed. Clippy clean at
`-D warnings`, semgrep and gitleaks clean.

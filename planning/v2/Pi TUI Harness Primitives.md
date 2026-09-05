# Pi TUI Harness Primitives

**Status:** implementation-ready

## Decisions

Pi is a fourth `AgentProvider` with wire id `pi` and label `Pi`. It is an opaque, terminal-backed coding harness. It reuses the existing `AgentSession`, `TerminalView`, `PtySession`, prompt delivery, idle detection, terminal attachment, git observation, MCP server, and coding lifecycle. The only provider switch remains the construction arm in `harness_for`.

Pi has no built-in MCP client. Build supplies its tools through one Build-owned Pi extension loaded explicitly from outside every checkout. The extension discovers and forwards the existing MCP surface; it does not define another Build API.

## Primitives And Boundaries

| Primitive | Public interface | Owns and hides | Does not own |
|---|---|---|---|
| `AgentLaunch` | `AgentLaunch::prepare(...) -> Result<PreparedAgentLaunch, OrchestratorError>` | Ordered workspace scaffolding followed by fallible harness spec creation; returns the complete spec and PTY size needed to spawn outside the app lock | Provider-specific setup internals, tab construction, or fallback policy |
| `HarnessContext` | A spawn-only value containing a resolved private Build `state_root`, `mcp_socket`, and mandatory absolute `bridge_exe` | Stable machine paths every harness may consume; never serialized or sent to the SPA | Pi setup, argv, or session state |
| `SessionIdentitySource` | A terminal-carrier value: `Known(String)` or `Located(Box<dyn SessionLocator>)`; `HarnessSpec` exposes `known_session_id: Option<String>` | Whether a terminal session's conversation id is known by its launch or discovered from durable harness records | Resume policy, transcript parsing, or provider selection |
| `PiHarness` | The existing `Harness` interface for `AgentProvider::Pi` | Pi executable, TUI-only argv, extension preparation, launch-known session identity, model/thinking flags, and Pi-specific settle/submit timing | PTY mechanics, prompts, Build tool schemas, domain validation, lifecycle, or UI |
| `BuildPiExtension` | Build side: embedded extension artifact used only by `PiHarness::spec`; Pi side: one extension factory loaded with `--extension` | Content-addressed materialization, MCP child lifecycle, handshake, dynamic tool registration, JSON-RPC correlation, response conversion, and shutdown | Tool names, labels, descriptions, schemas, domain rules, owner lookup, or lifecycle transitions |
| `validate_router_choice` | `validate_router_choice(choice: &ModelChoice) -> Result<(), RouterChoiceError>` in `router.rs` | The rule that Pi cannot be a router while current Claude/Codex/non-Pi overrides remain valid | Coding-agent defaults or harness construction |
| `DefaultHarnessSetting` | Existing `settings.get`, `settings.set`, `models.list.default_provider`, and absent-provider coding dispatch | Persisting the account default and applying it when a newly created coding agent does not name a provider | Router selection, creation-card choices, or task-specific presentation |
| `DisplayedProviderSerialization` | Existing generic `agentChoiceParams(catalog, choice)` | Serializing the concrete provider selected by the same narrowed catalog the unchanged picker displays | Provider options, labels, markup, Pi visibility, or account-default persistence |

`PiExtensionBundle`, `PiBuildToolsExtension`, and `McpStdioClient` are not three public primitives. They are one deep `BuildPiExtension`: Rust materialization and the TypeScript MCP child client are private implementation halves that no caller sequences. `PiHarness::spec` asks for a ready extension path once; loading the extension gives Pi a complete tool surface once.

## Fallible Specification Boundary

The minimal shared API change is:

```rust
pub struct HarnessContext {
    pub bridge_exe: PathBuf,
    pub mcp_socket: PathBuf,
    pub state_root: PathBuf,
}

pub trait Harness {
    fn spec(
        &self,
        choice: &ModelChoice,
        options: &SpawnOptions,
        context: &HarnessContext,
    ) -> Result<HarnessSpec, HarnessError>;
}

pub type WarmBuilder = Arc<
    dyn Fn(&str, &ModelChoice, &SpawnOptions) -> Result<HarnessSpec, HarnessError>
        + Send
        + Sync,
>;
```

`AgentLaunch::prepare(...) -> Result<PreparedAgentLaunch, OrchestratorError>` is the deep orchestration boundary. It owns the required order: scaffold the workspace, create `SpawnOptions`, invoke the fallible fixed spec or `WarmBuilder`, then return `PreparedAgentLaunch { spec, pty_size }`. The app clones the owned `AgentLaunch` while holding its lock, releases the lock, calls `prepare`, and gives the complete result to `Tab::spawn`; no app caller sequences filesystem or provider setup. Existing harnesses wrap their already-infallible argv construction in `Ok`. `PiHarness::spec` internally materializes `BuildPiExtension`, resolves its private session directory, and returns the complete `HarnessSpec`.

`HarnessSpec` carries `known_session_id: Option<String>`. Pi sets it to `Some(options.owner_id.clone())`; existing harnesses leave it absent. `Tab::spawn` converts a present value to `SessionIdentitySource::Known`; otherwise it converts the existing provider locator to `SessionIdentitySource::Located`. `open_session` hands that source to `PtySession`, which returns a known value immediately or asks the locator. This preserves the existing invariant that every agent session names its conversation through `AgentSession::session_id`; it does not equate terminal capability with transcript discovery. Human shell sessions remain the only terminal sessions with no conversation identity.

`HarnessContext` is constructed once from the daemon's resolved state. `bridge_exe` must come from a successful absolute, canonical `current_exe`; the current PATH fallback to `"build-bridge"` is removed. `state_root` is the actual private Build state root associated with the configured store, not recomputed from `HOME`, and never enters an RPC payload.

A specification error propagates through `WarmBuilder` into `AgentLaunch::prepare` as `OrchestratorError`, then reaches the existing spawn reservation failure path. The reservation removes its in-flight marker and rotated token, no `Tab::spawn` occurs, and the caller receives the contextual error. There is no fallback to another provider, a tool-less Pi process, a worktree-local extension, or headless mode.

## Launch Contract

For coding owner `<agent-id>`, `PiHarness` launches the normal interactive program with no prompt in argv:

```text
pi
  --approve
  --no-extensions
  --extension <state-root>/harness/pi/extensions/<sha256>/build-tools.ts
  --session-dir <state-root>/harness/pi/sessions/<agent-id>
  --session-id <agent-id>
```

An explicit model adds `--model <id>`. An explicit effort adds `--thinking <level>`. No `--print`, `--mode json`, `--mode rpc`, or headless path is permitted. `--no-extensions` disables discovered global and project extensions while the explicit Build extension still loads. Preserving user-installed Pi extensions is outside this slice.

The launch environment contains the existing `BRIDGE_MCP_SOCKET` and rotated `BRIDGE_MCP_TOKEN`, plus `BUILD_PI_MCP_COMMAND` set to the mandatory absolute bridge executable and `BUILD_PI_MCP_OWNER` set to the coding agent id. Values are passed as argv/environment entries, never through a shell.

`PtySession` remains the running implementation. It waits for Pi's bracketed-paste readiness, sends each `Turn` as a framed paste, and submits Enter after the harness-owned delay. Pi timing is measured and set in `PiHarness`, not copied from another provider without a PTY test.

Pi's remaining `Harness` answers are exact: `has_transcript` and `holds_conversation` return `false`, `session_locator` returns `None`, and `prepare_workspace` is a no-op because `--approve` owns project trust. No transcript path, locator, workspace mutation, `--continue`, or recorded vendor id participates in a Pi launch.

## Build Extension

The Rust half embeds the extension source in `build-bridge`. `PiHarness::spec` materializes it content-addressed under `<state-root>/harness/pi/extensions`, using a `0700` parent, temporary file plus atomic rename, `0400` final mode, and a content-hash verification before returning the path. A malformed state root, containment failure, hash mismatch that cannot be repaired, or write/rename failure returns `HarnessError` from `spec`.

No extension, package manifest, dependency tree, token, or Pi session file enters an agent worktree. This is packaging hygiene, not a sandbox claim: Pi still runs as the user under Build's existing YOLO threat model.

The TypeScript half starts exactly one absolute `build-bridge mcp --task <agent-id>` child during awaited extension initialization. The child inherits the MCP socket and token. Before registering any tool, the extension completes this exact handshake:

1. Send `initialize` and validate its JSON-RPC success response.
2. Send the `notifications/initialized` notification with no id.
3. Send `tools/list` and validate its JSON-RPC success response.

For every listed MCP tool, the extension registers one Pi tool with:

- `name`: the MCP name unchanged;
- `label`: the MCP name unchanged, deterministically, because MCP currently has no separate label;
- `description`: the MCP description unchanged;
- `parameters`: the returned `inputSchema` object directly;
- `execute`: one generic forwarder sending `tools/call { name, arguments }`.

The tool list must be an array of objects with unique non-empty names, string descriptions, and object input schemas. Every response must be valid JSON-RPC 2.0, carry exactly the pending id for a request, contain exactly one of `result` or `error`, and have the expected result shape. Invalid JSON-RPC envelopes, malformed framing, and unknown, duplicate, or mismatched response ids are terminal client failures.

A `tools/call` result must contain a boolean `isError` and a content array. The adapter accepts only blocks shaped as `{ "type": "text", "text": <string> }`, extracts their text in order, and joins blocks with `\n`. With `isError: false`, the joined text is the successful Pi tool result. With `isError: true`, it is a Pi tool error; an empty joined value uses the fixed text `Build tool failed without an error message`. An unsupported or malformed content block is a failure of that call only: reject that Pi tool execution, leave the MCP client live, and continue serving other calls. A valid JSON-RPC `error` is likewise a call-level failure for its matching request. No malformed result is treated as success.

The extension authors no tool schema and performs no domain validation. Pi validates against the discovered JSON Schema before execution, while `DoneServer` remains the canonical parser and validator. `DoneReport`, `BridgeAction`, coding-surface scoping, owner lookup, token comparison, and lifecycle changes remain in the existing MCP/daemon path. The extension never opens the daemon socket itself and never reads `.build/mcp*.json`.

## MCP Child Lifetime

The child process, pending-call table, write serialization, stdout reader, stderr drain, deadlines, and exit status are private to `BuildPiExtension`'s runtime client.

Unexpected EOF, child exit, an invalid JSON-RPC envelope, malformed framing, or an unknown, duplicate, or mismatched response id atomically latches one terminal client failure, rejects every pending call, makes every future call fail immediately with that same cause, and sends `SIGTERM` to the parent Pi process after client cleanup. Build then observes ordinary PTY/process exit and runs the existing session death path. Pi is never left running without Build tools. Tool-level errors and malformed or unsupported content blocks are not terminal client failures.

Normal extension shutdown is one idempotent `close`: stop accepting calls, reject pending calls as session shutdown, close the MCP child's stdin, wait a bounded grace for clean exit, kill if still alive, and always wait for/reap the child. The extension's `session_shutdown` handler calls only this operation. No caller manages child handles or cleanup steps.

## TUI Session And Resume

The Build coding-agent id is the Pi session id. Pi's `--session-id` creates it when absent and opens that exact session when present. A per-agent `--session-dir` under private Build state prevents collisions with ordinary Pi sessions and bounds lookup.

- A new Build agent id starts fresh, including in a reused or adopted checkout.
- A process respawn for the same Build agent resumes exactly, without `--continue`, recency guesses, screen scraping, or transcript discovery.
- `--session-id <Build agent id>` is the sole resume mechanism; `PiHarness` ignores generic `continue_session` and `resume_session_id` hints.
- `HarnessSpec.known_session_id` carries the Build agent id; `Tab::spawn` converts it to `SessionIdentitySource::Known`, so `PtySession` exposes that same value through `AgentSession::session_id` from launch onward.
- `has_transcript = false`, `holds_conversation = false`, and `session_locator = None` prevent every generic transcript/resume path from competing with the stable binding.
- The extension rejects `/new`, `/resume`, `/fork`, and `/clone` through `session_before_switch` and `session_before_fork`. `/tree` and compaction stay within the same Pi session and remain allowed.

Ending a Build session kills and reaps Pi and closes/reaps its MCP child. Session and extension files survive bridge restarts; ending a session does not delete Pi history.

## Provider And Model Catalog

The first slice advertises Pi with `models: []`. Pi's catalog is local, credential-dependent, and multi-provider; Build does not freeze it into a curated list or shell out on each `models.list`. An empty catalog means Pi's configured model.

Pi advertises `off`, `minimal`, `low`, `medium`, `high`, `xhigh`, and `max` as effort levels and maps them directly to `--thinking`. Explicit safe model ids continue to pass through as `--model`; provider-qualified ids and a dynamic Pi model picker are deferred. The catalog contract permits a provider with no curated models.

Pi is coding-session-only. The router's default remains pinned to `ClaudeAdk`; `default_harness` does not select or alter it. `router::validate_router_choice` runs both when router configuration is loaded and immediately before a `RouterSession` is constructed. It rejects `AgentProvider::Pi` and accepts every currently valid non-Pi override unchanged. Construction cannot reach `harness_for(Pi)` for a router even if invalid state bypasses configuration loading.

## Settings-Only UI Boundary

Only Settings presents Pi. `mountDefaultHarness` loads both `settings.get` and `models.list`, derives its option list from `models.list.providers`, and accepts `settings.default_harness` only when that exact id exists in the returned provider list. Missing, malformed, or unknown values are shown as a Settings error rather than silently mapped to another provider. The Settings work only makes Pi available in this control and persists `pi` as the account default.

Generic `agentChoiceParams` now always serializes the concrete provider already selected and displayed by the unchanged creation picker, including when the source preference omitted a provider. The helper resolves through the same narrowed catalog used to paint the picker, so presentation and dispatch cannot disagree. This is a generic serializer invariant, not a Pi branch or a new UI primitive.

No Pi card, Pi label, Pi condition, or visible Pi branch is added outside Settings. Pi is not added to `STARTABLE_PROVIDERS`, `PROVIDER_LABELS`, `creatableAgents`, or `creatableCatalog`. There are no `modelPicker.js`, creation-view, rail, task, conversation, terminal, CSS, or HTML changes for Pi. Unchanged generic surfaces may naturally render a persisted `pi` token; that is existing fallback behavior, not a Pi presentation.

Visible creation-picker selections serialize their concrete Claude or Codex provider and override the account fallback. Selecting Pi as `default_harness` affects only bridge-side coding-agent creation paths where the client omits `provider`; paths using the visible creation pickers do not omit it and therefore do not start Pi. Omitted-provider creation paths use the Settings default. This constrained behavior is intentional in this slice, and no claim is made that every creation path uses Pi. The router remains independently validated and defaults to `ClaudeAdk`.

## Test Seams And Critical Failures

Tests are written first.

- `AgentLaunch::prepare` tests prove workspace scaffolding precedes fallible harness spec creation, the returned `PreparedAgentLaunch` contains the complete spec and PTY size, an injected `Harness::spec`/`WarmBuilder` error becomes `OrchestratorError`, and preparation runs outside the app lock.
- Spawn failure tests prove a preparation error clears the reservation/token, does not call `Tab::spawn`, and never falls back.
- `HarnessContext` tests require canonical absolute `bridge_exe`, the configured private `state_root`, and no PATH fallback or wire exposure.
- `PiHarness` tests pin TUI-only argv, extension/session paths, stable session id, environment, empty model catalog, and every ordinary effort mapping such as `high -> --thinking high`.
- Harness contract tests pin `has_transcript = false`, `holds_conversation = false`, `session_locator = None`, no-op workspace preparation, and launch-known identity equal to the Build agent id.
- Materialization tests pin atomic install, idempotence, hash repair, permissions, state-root containment, and each fallible filesystem operation.
- A fake Pi executable exercises PTY readiness, prompt paste, delayed submit, terminal input/resize, process exit, and exact respawn identity without a model call.
- Extension tests use a fake MCP child to pin `initialize -> notifications/initialized -> tools/list`, deterministic labels, raw schema reuse, call forwarding, response validation, concurrent calls, timeouts, malformed JSON, unknown/duplicate/mismatched ids, stderr saturation, and normal close/kill/reap.
- Result-conversion tests cover one text block, ordered newline joining, empty successful content, non-empty and empty `isError` results, malformed blocks, unsupported block types, and a matching JSON-RPC error. They prove block failures affect one call while envelope/id failures terminate the client.
- Child-death tests prove pending and future calls fail with the latched cause, Pi is terminated, and Build observes the normal session-exit path. Missing command, owner, socket, or token aborts initialization before Pi accepts a turn.
- MCP integration tests rotate the token and prove stale/wrong tokens fail and invalid `done` or message arguments return the existing canonical coding-tool errors.
- Resume tests prove `AgentSession::session_id` equals the launch's Build agent id immediately, one Build agent resumes exactly, another agent in the same checkout starts fresh, and Pi session-switch commands are refused.
- Router tests exercise both configuration loading and `RouterSession` construction: Pi is rejected before `harness_for`, the default remains `ClaudeAdk`, and every current non-Pi override remains accepted.
- Settings tests prove `mountDefaultHarness` derives and validates the catalog option and that `pi` round-trips and persists as the coding default.
- Generic serialization tests in `agentChoice`, `createWork`, compose, and issue dispatch may change to prove the provider already displayed by each unchanged picker is always sent, including when the source preference was empty. They also prove an explicitly displayed Claude or Codex choice overrides a Pi account default.
- Omitted-provider path tests prove bridge-side creation still uses `default_harness = pi` when no visible picker serialized a provider.
- Presentation tests prove no Pi option, label, branch, card, CSS, or HTML appears outside Settings; the creation pickers continue to display only their existing Claude/Codex choices.

All startup failures are fatal and contextual. Only specific errors that gain context are caught and rethrown. There is no silent fallback to another harness, a tool-less Pi session, `--continue`, or headless mode.

## Non-Goals

- Pi RPC, JSON, print/headless, SDK, router, or activity-stream integration.
- Parsing or scraping Pi TUI output for status, completion, identity, messages, or tool calls.
- Pi in creation cards, new-agent rails, issue assignment, task views, conversation UI, terminal UI, notifications, or any application surface outside the Settings default-harness control.
- Adding Pi to `STARTABLE_PROVIDERS`, `PROVIDER_LABELS`, or any task-specific presentation vocabulary.
- A Pi model picker, dynamic `pi --list-models` catalog, credential detection, provider-qualified model ids, or Build-managed Pi authentication.
- Installing the extension globally in `~/.pi`, adding project-local `.pi` files, loading agent-written extensions, or preserving arbitrary user Pi extensions in Build sessions.
- New Build tool schemas, duplicate domain validation, direct extension access to the daemon socket, or a Pi-only lifecycle path.
- Sandboxing or claiming that state outside the worktree is inaccessible to an agent running as the user.

## Blocking Decisions

None.

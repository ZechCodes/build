# Pi compaction qualification — #659

Observed 2026-10-18 against Pi **0.86.1**. The production **TUI under a PTY**
now executes Build's actual default extension export, real MCP initialization
and private telemetry-file sink. The actual Rust `PiTelemetry::follow` observer
reads that file and atomically publishes private acknowledgement receipts. A
synthetic in-process provider and an owned stdio MCP fixture make the
qualification credential-free. Seven TUI scenarios pass; six supplemental RPC scenarios cover finer callback/failure ordering.
This qualifies the pinned carrier and hooks below, not native enforcement of
Build's N-token policy, summary quality, arbitrary Pi versions or a production
provider.

## Isolation and reproducibility

Direct executable:
`/home/zech/.local/share/mise/installs/pi/0.86.1/pi/pi`.
SHA256: `9b939197c9ca98182981fc448ac6d441076d3c37360cf8dc649193e664fcf6f7`.
The direct isolated `--version` probe reported `0.86.1`, exit **0**.

The supplemental [RPC qualification driver](../../scripts/qualify-pi-compaction.mjs)
creates a temporary HOME, cwd, explicit Pi agent directory, extension and
settings for each scenario. Every Pi child receives a constructed environment
and `PI_OFFLINE=1`. It enables only the synthetic extension, disables built-in
tools, skills, prompt templates and themes, and does not persist sessions.
The registered provider returns deterministic assistant events in process;
its placeholder API key is synthetic and no provider request is made.
The driver imports only Build's `registerCompactionTelemetry` named export;
it never calls the MCP-launching default extension function. Its fifth-argument
receipt reader is a synchronous fake observer, used only for supplemental hook
ordering. The TUI proof below exclusively uses the real Rust observer.

No installed bridge, daemon, installer, browser, pairing, credentials, protected
user configuration, network configuration or firewall is used. Cleanup signals
only children created by the driver, by their child handles, then deletes its
temporary files. Each child is intentionally terminated with SIGTERM after its
assertions; the qualification gate is the driver exit code, not a claimed
successful normal Pi child exit.

From the repository root, with a known Node executable and the pinned Pi path:

```bash
qual_home=$(mktemp -d /tmp/build-659-qual-home.XXXXXX)
env -i HOME="$qual_home" PATH=/usr/bin:/bin nice -n 10 \
  /home/zech/.local/share/mise/installs/node/22.23.3/bin/node \
  scripts/qualify-pi-compaction.mjs \
  /home/zech/.local/share/mise/installs/pi/0.86.1/pi/pi \
  > /tmp/build-659-pi-qualification-node22.json \
  2> /tmp/build-659-pi-qualification-node22.err
qual_exit=$?
printf 'exit=%s\n' "$qual_exit"
```

The six-scenario RPC rerun on pinned Node **22.23.3** exited **0**, using the
current Build extension including its manual-settlement generation guard,
unsafe-number policy validation and eight-second Rust-observer receipt lease.
Initial development runs used Node 26.8.1;
the final command above is the recorded supported-Node qualification. Its
JSON output contains only synthetic
event metadata, context counts and generated session identifiers. The driver
is opt-in and is not executed by ordinary repository tests. It requires the
explicit pinned executable; it does not discover, download or install Pi.

## Full default-export TUI qualification

The [PTY driver](../../scripts/qualify-pi-compaction-tui.py) launches the exact
pinned binary in interactive mode. Its
[synthetic provider extension](../../scripts/qualify-pi-compaction-tui.ts)
imports a byte-for-byte temporary copy of `build-tools.ts` and calls its actual
default export with the real Pi API. It injects no runtime-configuration
dependency, fake event dispatcher or fake telemetry sink. The default export
initializes the existing `bridge/tests/fixtures/pi-mcp-child.mjs` through its
ordinary subprocess/JSON-RPC path and writes its ordinary private sidecar.

Every TUI scenario also launches the ignored Rust lib-test helper
`harness::pi_telemetry::tests::pi_qualification_observer` from an explicitly
provided source-built test artifact. The helper uses actual `PiTelemetry::follow`
with the scenario owner, thread and launch identity. It writes actual atomic
0600 `${log}.ack` receipts and an atomic report of enforcement, reason, status
and context resets. No fake acknowledgement is injected into the TUI extension.
The helper's standalone process is isolated with a constructed environment,
own temporary HOME and explicit log/report/stop paths. Each scenario asks it to
stop and requires exit 0; its 60-second deadline bounds abandoned probes.

For this probe, the coordinator explicitly authorized **only two fake values
in the constructed child environment**: `BRIDGE_MCP_SOCKET` names
`<owned temporary scenario directory>/unused-fake-mcp.sock`, and
`BRIDGE_MCP_TOKEN` is the fixed string `qualification-dummy-token`. No inherited
or live values are read, copied, printed or changed. The fake socket path is
unused: the fixture serves MCP over stdio. Assertions verify that the fixture
received exactly that path, dummy token and `mcp --task <owner>` arguments.
No socket server, installed bridge executable or daemon is involved.

The driver constructs HOME, Pi agent directory, cwd and settings beneath its
temporary root. `PI_OFFLINE=1` disables startup update checks. The CLI allows
only the fake `done` and `post_thread_message` tools; built-in shell and file
tools are unavailable. It supplies `--no-extensions` and only the explicit
qualification extension. The file starts at mode 0600, and the actual Build
sink is responsible for all telemetry writes. A delay launcher uses the exact
pinned Node 22.23.3 executable and the existing fake MCP fixture; it installs
nothing. Owned child PIDs are captured before shutdown and retained as pidfds
so cleanup cannot signal a recycled PID. The driver deletes its temporary
HOME, files and fixtures after every run.

Reproduce from the repository root, after compiling the bridge lib-test
artifact under the repository's normal clean-HOME/nice gates. Pass that exact
artifact's absolute path as `--observer`; never pass an installed bridge:

```bash
qual_home=$(mktemp -d /tmp/build-659-tui-home.XXXXXX)
env -i HOME="$qual_home" PATH=/usr/bin:/bin nice -n 10 python3 \
  scripts/qualify-pi-compaction-tui.py \
  --pi /home/zech/.local/share/mise/installs/pi/0.86.1/pi/pi \
  --node /home/zech/.local/share/mise/installs/node/22.23.3/bin/node \
  --observer /absolute/path/to/source-built/bridge-lib-test-artifact \
  > /tmp/build-659-pi-tui.json 2> /tmp/build-659-pi-tui.err
qual_exit=$?
printf 'exit=%s\n' "$qual_exit"
```

The complete seven-scenario TUI gate exited **0**. The exact exercised extension
SHA256 is `76e0b9f56e85e625543048859c3d752c659c3f34b434423e186db256673735ef`.
The driver writes that hash, the pinned Pi hash, synthetic event records and
private-file telemetry to its JSON result. Evidence from the run is retained
locally at `/tmp/659-logs/pi-tui-round3-final.json`; no temporary fixture state is
needed to reproduce it. In the true MCP-transport-failure scenario, the actual
extension sends SIGTERM and Pi exits. The other six Pi children remain alive
through their assertions and exit after controlled cleanup. All Rust observer
helpers exit 0, and all owned MCP children stop; reload creates two successive
fixture children, other scenarios one. All seven observer exits and all seven
Pi process exits are 0, including Pi's controlled exit after transport SIGTERM.

| TUI scenario | Measured result |
| --- | --- |
| Threshold and manual | Reported active context reaches 50001/64000; ordinary threshold attempts are cancelled by Build. Typing native `/compact` at idle succeeds, emits null active context and reaches deferred settlement. |
| Overflow | A synthetic `prompt is too long` response causes actual native `overflow` compaction with `willRetry: true`, then another agent run and final settlement. Build does not cancel it. |
| Reload | Typing native `/reload` tears down the original extension and loads the default export again. Readiness advances generation 1→2, sequence restarts at 1 in the new generation, and a new prompt settles normally. No old-generation records reappear after the replacement starts. |
| Slow MCP initialization | The fake child delays initialization 11.5 seconds. At 10.5 seconds Pi is alive, readiness has not been emitted and starting heartbeats are present. Initialization then completes; ordinary prompts and native manual compaction succeed. |
| MCP transport failure | An actual registered tool call makes the owned fake MCP child exit 17. The default extension emits unsupported and sends SIGTERM to Pi; the fixture records the signal and requires Pi to terminate. |
| Sink failure | Changing only the owned telemetry file to mode 0644 makes the private-file sink reject it. The real Rust observer reports Unsupported. Pi remains alive, native threshold compaction succeeds, a healthy MCP `done` call returns `one`, and a further prompt completes. |
| Reader loss with a writable sink | Changing only the owned sidecar to mode 0200 causes a confirmed `PermissionError` on reads while its size increases from actual JavaScript appends. The real Rust observer reports Unsupported and revokes its receipt. Pi allows native threshold compaction, preserves a healthy MCP call returning `one`, and completes another prompt. The driver restores file readability only after those assertions. |

The telemetry-failure scenarios require successful native threshold compaction
and a healthy MCP tool result after the injected failure. These assertions
prove that fallback withdrawal restores native behavior while preserving the
separate MCP transport. A real transport failure deliberately terminates Pi. Successful summaries still come
from the documented custom-compaction return value; no external LLM is used.
All telemetry assertions check the captured owner and contiguous sequence
numbers within strictly advancing generations. Every receipt must have mode
0600 and matching owner, thread, launch and generation, and acknowledge an
actually emitted sequence. Each failure scenario also requires exactly one
additional Rust context reset despite subsequent health polling.

The PTY probe executes Pi directly with the actual Rust telemetry observer.
It proves carrier-to-observer receipt exchange, starting heartbeats through slow
initialization, reload generations, and fallback withdrawal on reader loss.
The broader PtySession queue fencing and compaction policy engine remain
covered by repository tests. It never invokes the installed bridge or daemon.

## Supplemental RPC behavior

The fixture model has a 64000-token window and reports 50001 active tokens.
Temporary settings retain 10 recent tokens and reserve 16384. Two synthetic
prompts provide enough history for Pi to prepare a compaction. A separate
queued scenario adds an extension-generated follow-up. Successful summaries
are supplied through Pi's documented custom-compaction return value, so success
does not depend on a model or a network endpoint. The failure scenario instead
lets Pi call the synthetic provider for summarization and receives an error.

| Scenario | Actual Pi behavior and Build telemetry assertions |
| --- | --- |
| Threshold suppression | `session_before_compact` has `reason: "threshold"`; the actual Build handler cancels it. Pi emits `session_compact_failed` with `aborted: true`, retains 50001 context tokens, and then reaches `agent_settled`. |
| Extension manual success | `ctx.compact()` returns `undefined` immediately. Pi emits `session_before_compact` and `session_compact` with `reason: "manual"`, then invokes `onComplete`. Build emits completed compaction followed by null active tokens. |
| Extension manual failure | Pi emits `session_compact_failed` with `reason: "manual"`, `aborted: false`, then invokes `onError`. Build reports failure, never completion, and retains the 50001 active tokens. |
| Native manual success | The RPC `compact` command emits the same manual hooks. Build clears active tokens and emits a deferred settled record. |
| Native manual failure | RPC `compact` emits failed manual compaction. Build preserves context and emits a deferred settled record. |
| Overflow recovery | A synthetic `prompt is too long` error triggers `reason: "overflow"`, `willRetry: true`. Build allows it. Pi compacts, starts another agent run and receives fresh usage before settling. There is no `agent_settled` between the failed run and retry. |
| Queued follow-up | Two `turn_end` events occur before the first `agent_settled`; the first response does not authorize another Build delivery. |

Every RPC scenario asserts that Build emits `starting` then `ready` with the
explicit session owner, all records have that owner and a strictly increasing
sequence, no `unsupported` record appears, and the final non-heartbeat record
is `settled`.

## Contract and ordering details

The pinned package's `docs/extensions.md` documents:

- `session_before_compact`: `preparation`, `branchEntries`,
  `customInstructions`, `reason`, `willRetry`, `signal`; returning
  `{ cancel: true }` cancels that attempt.
- `session_compact`: `compactionEntry`, `fromExtension`, `reason`, `willRetry`.
- `session_compact_failed`: `reason`, `aborted`, `willRetry`, `fromExtension`,
  and `errorMessage` for non-abort failures.
- Reasons are `manual`, `threshold`, and `overflow`. Cancelling only threshold
  attempts preserves user-requested compaction and native overflow recovery.
- `ctx.compact()` is not awaitable; its options accept `onComplete` and
  `onError` callbacks.
- `agent_end` can precede retry, compaction or queued continuation.
  `agent_settled` is the full-run completion hook. `ctx.isIdle()` can become
  false again if another extension starts work.

The runtime reports context as
`{ tokens: 50001, contextWindow: 64000, percent: 78.12656249999999 }` before
compaction. On successful compaction, both `session_compact` and `onComplete`
see `{ tokens: null, contextWindow: 64000, percent: null }`. Pinned
`docs/rpc.md` documents that tokens and percent remain null until fresh
post-compaction assistant usage arrives. An unknown current count must replace
the stale pre-compaction count, not become zero or reuse cumulative usage.

Manual compaction has an additional ordering constraint. In the successful
`session_compact` hook, `ctx.isIdle()` is still **false**, because Pi has not
cleared its compaction state. At `onComplete`, and in a `setTimeout(0)` callback
scheduled from that hook, it is **true** with no pending messages. The failed
manual hook already reports idle. Neither manual path emitted a further
`agent_settled` during the 150 ms observation period. These observations were
repeated through both extension `ctx.compact()` and native RPC `compact`.

Build therefore checks manual completion in a deferred callback, guarded by
its work generation, `ctx.isIdle()` and `!ctx.hasPendingMessages()`. That
callback must not authorize delivery if another run has started. Native
overflow continues to wait for the actual `agent_settled` event.

## Regression evidence and limits

The cross-side regression probes were run against the saved pre-receipt
extension from commit `30fb34fd7`, with the real Rust observer helper. Each
failed with exit **1** at the intended behavior boundary:

- Mode 0200 denied reads while JavaScript continued appending. Rust reported
  Unsupported, but the old extension kept cancelling threshold attempts; the
  gate timed out waiting for native compaction.
- Mode 0644 withdrew telemetry and also closed the healthy MCP client. The
  gate failed because the subsequent `done` call could not return `one`.
- A real fake-MCP child exit left Pi alive. The gate failed because Pi did not
  terminate on transport failure.

The baseline source override is explicit (`--extension-source`) and isolated
in a temporary copied extension; no shared production file was swapped. The
final seven-scenario run uses the default current source. Local RED logs are
`/tmp/659-logs/pi-round3-red-read_loss.err`,
`/tmp/659-logs/pi-round3-red-sink_failure.err` and
`/tmp/659-logs/pi-round3-red-mcp_failure.err`.


The first final cross-side run exited 1 at the fixture's fixed 100 ms manual
settlement assertion while the repository was compiling. The assertion now
waits for the actual `settled` record after completed manual compaction, with a
bounded deadline and observer/event diagnostics. The manual-only rerun exited
0; the final full run uses that event condition, not a longer blind sleep.
Python AST parsing, pinned Node `--check` and own-file `git diff --check` also
exited 0 under nice 10.

An initial probe used only one prompt and Pi refused manual compaction as
`Nothing to compact (session too small)`; the driver exited 1. Seeding a second
turn provided the required history. A synthetic error phrased only as
`context window exceeded` did not trigger overflow recovery; the corresponding
assertion exited 1. The final probe uses the recognized `prompt is too long`
provider error. A queued-follow-up assertion initially expected two low-level
agent runs; Pi instead produced two turns in one run, so that assertion exited
1 and was corrected to require both turn completions before settlement.

The first full TUI attempt also exited 1 because its `--no-tools` flag disabled
the fake MCP tool: Pi correctly reported `Tool done not found`, so the intended
MCP failure had not been injected. The final fixture uses an explicit allowlist
of only the two fake MCP tools and proves the actual child failure. Probes
that copied a source file during intermediate edits failed at readiness; they
were rerun against the completed source and are not evidence of a pinned-Pi
timer incompatibility.

These were qualification-fixture assumptions, not passing results. The final
drivers assert actual events and judge success by exit code.

**Promotion limits:** live-provider summarization and native per-launch N-token
injection remain unqualified. Promoting native enforcement is blocked until the
effective model budgets and threshold mapping are demonstrated. Any Pi version
other than 0.86.1 remains blocked pending equivalent real TUI/default-export
hook, reload and failure qualification. The synthetic carrier probes do not
establish behavior of arbitrary external providers or future versions.

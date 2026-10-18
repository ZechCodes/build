# Pi compaction qualification — #659

Observed 2026-10-18 against Pi **0.86.1**. The extension hook contract and
between-turn fallback telemetry were exercised against the real pinned Pi
runtime with a synthetic, in-process provider. This qualifies the hooks and
ordering below; it does not qualify native enforcement of Build's N-token
policy, summary quality, arbitrary Pi versions, or a production provider.

## Isolation and reproducibility

Direct executable:
`/home/zech/.local/share/mise/installs/pi/0.86.1/pi/pi`.
SHA256: `9b939197c9ca98182981fc448ac6d441076d3c37360cf8dc649193e664fcf6f7`.
The direct isolated `--version` probe reported `0.86.1`, exit **0**.

The committed [qualification driver](../../scripts/qualify-pi-compaction.mjs)
creates a temporary HOME, cwd, explicit Pi agent directory, extension and
settings for each scenario. Every Pi child receives a constructed environment
and `PI_OFFLINE=1`. It enables only the synthetic extension, disables built-in
tools, skills, prompt templates and themes, and does not persist sessions.
The registered provider returns deterministic assistant events in process;
its placeholder API key is synthetic and no provider request is made.
The driver imports only Build's `registerCompactionTelemetry` named export;
it never calls the MCP-launching default extension function.

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

The final six-scenario rerun on pinned Node **22.23.3** exited **0**, using the
current Build extension including its manual-settlement generation guard and
unsafe-number policy validation. Initial development runs used Node 26.8.1;
the final command above is the recorded supported-Node qualification. Its
JSON output contains only synthetic
event metadata, context counts and generated session identifiers. The driver
is opt-in and is not executed by ordinary repository tests. It requires the
explicit pinned executable; it does not discover, download or install Pi.

## Verified runtime behavior

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

Every scenario also asserts that Build's first record is `ready` with the
explicit session owner, all records have that owner and a strictly increasing
sequence, no `unsupported` record appears, and the final record is `settled`.

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

## Limits and failed probes

An initial probe used only one prompt and Pi refused manual compaction as
`Nothing to compact (session too small)`; the driver exited 1. Seeding a second
turn provided the required history. A synthetic error phrased only as
`context window exceeded` did not trigger overflow recovery; the corresponding
assertion exited 1. The final probe uses the recognized `prompt is too long`
provider error. A queued-follow-up assertion initially expected two low-level
agent runs; Pi instead produced two turns in one run, so that assertion exited
1 and was corrected to require both turn completions before settlement.

These were qualification-fixture assumptions, not passing results. The final
driver asserts actual events and does not infer success from printed summaries.
It does not run the TUI, perform live-provider summarization, or prove arbitrary
future versions expose the same hooks. Unknown versions still require an
explicit unsupported/best-effort path rather than a claim of native N-token
enforcement.

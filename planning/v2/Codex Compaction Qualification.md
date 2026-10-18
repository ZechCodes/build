# Codex compaction qualification — #657

Observed 2026-10-18. Native enforcement remains **unqualified for production**
for both Codex carriers. Successful isolated runs establish the mechanism on
0.160.0 with `gpt-6.1-sol`; they do not establish arbitrary launch configuration,
model catalog contents, or a safe production threshold range.

## Isolation and executable

Direct executable:
`/home/zech/.local/share/mise/installs/codex/0.160.0/bin/codex`.
SHA256: `12eb3e81114588aca3b7998f4f19e8997b056aca08e57a7ca7c8a3ec8c652aad`.
The isolated `--version` command reported `codex-cli 0.160.0`, exit 0. The
install-capable `~/.local/bin/codex` wrapper was inspected but never executed.

The initial live executions used constructed environment variables, HOME,
CODEX_HOME and cwd beneath `/tmp/build-657-qualification`, under `nice -n 10`. For each live
process, only the authorized `~/.codex/auth.json` was copied, chmod 0400,
and bind-mounted read-only over the temporary copy. The original was never
modified, no other protected configuration was inspected, and each copy was
deleted after its process exited. No bridge executable, installer, login,
browser, pairing, or network/firewall configuration was used. Codex's normal
provider and plugin discovery wrote only into the temporary Codex home.

App-server used stdio JSONL with initialize/initialized, thread/start or exact
thread/resume, and turn/start. Both process flags and thread config carried:

```text
model_auto_compact_token_limit = 12000
model_auto_compact_token_limit_scope = "total"
```

Model selection was `gpt-6.1-sol`, effort `low`, approval `never`, sandbox
`read-only`. Initial prompts requested only `OK`; one added 1100 numbered
synthetic filler lines. No manual compaction request was sent. The TUI resumed
the same thread in an owned PTY with the same root flags and `--no-alt-screen`.
Only terminal capability responses and Ctrl-C after task completion were sent
by the PTY driver.

## Live observations

| Path | Observation | Probe / Codex exit |
| --- | --- | --- |
| App-server fresh, first turn | `OK`; active context 14229; no compaction yet | 0 / 0 |
| Same process, next turn | `contextCompaction` started then completed; context estimate 6018; new filler input then completed at 25648 | 0 / 0 |
| Exact app-server resume | Compaction completed at estimate 22592, then `OK` at 24416; native persisted task completion | 1 / 0, reader issue below |
| Repeated exact app-server resume | Compaction completed at estimate 22552, then `OK` at 24416; `turn/completed` observed | 0 / 0 |
| Exact TUI resume | Persisted `compacted` record and completed `ContextCompaction`; estimate 22541, then `OK` and `task_complete` at 25280 | 0 / 0 |
| Resumed app-server, one tool continuation | Pre-turn and mid-turn native compactions completed, then `OK` and normal `turn/completed` | 0 / 0 |
| Fresh app-server, window probe | N=25000 with temporary `model_context_window=20000`; `OK` and reported usable window 19000 | 0 / 0 |

The tool-continuation prompt requested only `printf qualification-probe`. The
owned read-only isolated environment refused unified-exec creation with
permission denied (tool exit -1). Codex consumed that failed tool result,
completed a second native compaction within the same still-open turn, and
returned `OK`. This verifies continuation after a tool result, not successful
shell execution. No shell command ran.

The first resume driver incorrectly combined buffered text reads with file
readiness polling. It timed out after receiving compaction and token updates;
the persisted rollout already recorded successful completion and the Codex
process exited 0. The driver was corrected to buffer raw reads and the exact
resume repeated successfully. This was a qualification-driver failure, not a
native compaction failure.

Before resume, the temporary user config deliberately contained N=900000 and
scope `body_after_prefix`. `config/read` resolved N=12000/`total` from session
flags, and both resumed carriers actually compacted. This establishes that
these root overrides beat that ordinary stale file. The matching thread config
was accepted on fresh and resumed app-server paths; acceptance alone is not
proof of native timing or every precedence relationship.

Sanitized machine-readable evidence is retained locally at
`/tmp/build-657-qualification/sanitized-evidence.json`; it contains event kinds,
completion statuses and token counters, without credentials or model text.
The credential copy was confirmed absent after the final run.

The provider-fetched temporary model catalog advertised raw context 500000,
maximum context 872000, and effective percentage 95. Normal token reports
showed 475000 usable context. These values were observed for these runs only.
The low requested threshold did not make a hard ceiling: retained user content
could leave post-compaction context above 12000, and a completed answer could
end above threshold until a later safe execution boundary.

## Selected-profile probe (scope-review follow-up)

On 2026-10-18, a separate credential-free probe used the same executable and
SHA256 above, with fresh HOME, CODEX_HOME and cwd beneath
`/tmp/build-657-profile-probe`. Its constructed environment contained only
PATH, HOME, CODEX_HOME, LANG and TERM. No credentials were read or copied, and
no thread, turn or model call was made. Each process ran at nice 10.

The requested temporary `config.toml` contained:

```toml
profile = "x"
[profiles.x]
model_auto_compact_token_limit = 900000
model_auto_compact_token_limit_scope = "body_after_prefix"
model_post_turn_compact_threshold_percent = 70
model_context_window = 500000
model_token_budget_fallback_buffer = 12345
model_effective_context_window_percent = 80
```

The process argv after `/usr/bin/nice -n 10` was:

```text
/home/zech/.local/share/mise/installs/codex/0.160.0/bin/codex
--config model_auto_compact_token_limit=150000
--config model_auto_compact_token_limit_scope="total"
--config check_for_update_on_startup=false
app-server --stdio
```

Each `--config` value above is one argument, retaining the quotes around
`total`. After `initialize`/`initialized`, the only request was:

```json
{"id":2,"method":"config/read","params":{"includeLayers":true}}
```

| Case | Observed result | Probe / Codex exit |
| --- | --- | --- |
| Selected legacy profile above | `config/read` error -32603: legacy `profile = "x"` is unsupported; use `--profile x` with `x.config.toml` | 0 / 0 |
| Suggested file profile: move the same settings to `x.config.toml`, add root `--profile x` | CLI rejects `--profile` for `app-server` before initialization; permitted command list includes runtime commands and `codex mcp` | 1 / 1 |
| Ordinary-file control: same stale settings at the root of `config.toml`, no profile selector | Effective limit 150000 and scope `total`, both attributed to `sessionFlags`; post-turn threshold 70 remains user-sourced | 0 / 0 |

The first probe's exit 0 means the driver collected the RPC error successfully;
it does **not** mean Codex accepted the selected profile. The second probe's
exit 1 records the missing initialization response after the CLI refusal.
The ordinary-file control retains the stale 900000/`body_after_prefix` values
in its lower user layer. It explicitly warns that the two speculative keys
`model_token_budget_fallback_buffer` and
`model_effective_context_window_percent` are ignored; their appearance in raw
layers/origins is not evidence of an effective setting.

These results do not establish a selected-profile override relationship or
per-thread precedence. The legacy selector is rejected, and the supported
runtime profile cannot be measured through this app-server `config/read`
path. **Selected-profile precedence remains a named promotion blocker.**
No child normalization or production native promotion follows from this probe.
Fixtures cover both the legacy selector/table and `--profile x` with
`x.config.toml`, exercise positive fallback and preserve the stale files;
the fake config evaluator refuses legacy selectors instead of inventing a
successful native merge. Ordinary-file native fixtures retain their narrower
precedence claim.

Reproducible drivers and full layer/origin evidence are retained under
`/tmp/build-657-profile-probe`: `probe.py` / `legacy-evidence.json`,
`supported-probe.py` / `supported-profile-evidence.json`, and
`root-file-probe.py` / `root-file-evidence.json`. The task carries these records
and their exit codes. The version probe also exited 0 (`codex-cli 0.160.0`).

## Pinned upstream semantics

The [0.160.0 ModelInfo implementation](https://github.com/openai/codex/blob/rust-v0.160.0/codex-rs/protocol/src/openai_models.rs#L484)
resolves raw context from `context_window`, falling back to
`max_context_window`. Total-scope automatic compaction clamps its configured
threshold to floor(90% of raw context). The usable inference window separately
uses `effective_context_window_percent` (default 95). The two bounds must not
be confused: raw 500000 gives native ceiling 450000 and usual usable window
475000. Translation must refuse an unrepresentable requested threshold instead
of reporting it unchanged after native clamping.

The [0.160.0 context status implementation](https://github.com/openai/codex/blob/rust-v0.160.0/codex-rs/core/src/session/context_window.rs#L53)
counts the full active context for `total`. `body_after_prefix` subtracts the
window's initial prefix and has different limit handling. A configured token
budget fallback buffer can delay the threshold, while the full usable context
bound independently forces compaction. Therefore N and `total` alone do not
prove an effective trigger of N in arbitrary settings.

The [0.160.0 turn loop](https://github.com/openai/codex/blob/rust-v0.160.0/codex-rs/core/src/session/turn.rs#L533)
checks after sampling and tools. Threshold compaction proceeds when another
sampling step or pending input requires continuation. It also has a separate
post-turn percentage path. Its pre-turn check precedes recording new user
input, explaining why one large message can pass the threshold before a later
check. These are native scheduling semantics, not a strict token ceiling.

## Promotion boundary

No production model/version/range is promoted by these observations. A future
qualification must establish the effective model window and token-budget buffer
for the actual launch, including the configurable effective-context percentage
that can make the full-window cap occur earlier than the 90% limit; bound
model/catalog/config changes; and verify any
post-turn threshold setting that can trigger earlier compaction. It must not
infer support from accepting CLI keys or an app-server config response.
Selected-profile precedence is also unqualified: the legacy profile table is
rejected by 0.160.0, while its supported runtime profile cannot be inspected
through app-server `config/read`. A future promotion must establish that
selected-profile compaction controls cannot defeat the captured policy, or
normalize them child-only with separate evidence. Current fallback does not
repair or claim to override a rejected legacy configuration.
The shared known-version adapter reports
`codex_model_window_buffer_and_profile_unqualified`. Its captured inputs do
not establish profile absence, so this conservative reason applies to all
0.160.0 production openings, including either selected-profile form, without
reading user configuration. Unknown versions retain `codex_version_unqualified`.

Unknown versions, unknown model bounds, altered native trigger semantics, and
thresholds above the exact native ceiling retain between-turn enforcement with
an explicit reason. Native automatic compaction is not disabled by this adapter;
manual compaction remains available. The between-turn fallback therefore does
not assert exclusive control over Codex's own context management.

## Exact launch and protocol templates

These templates reproduce the argv and constructed environment used by the
Python drivers; the drivers supplied cwd directly to `Popen`. They assume the
authorized read-only credential copy has already been prepared as described
above and remove it after the owned process exits. `<thread-id>` denotes the
exact ID returned by the successful fresh thread response. Neither template
invokes the installed wrapper.

Common isolated launcher (the braces are shell variable expansion, not a
request to read the original credential file):

```bash
QUAL_ROOT=/tmp/build-657-qualification
QUAL_CODEX=/home/zech/.local/share/mise/installs/codex/0.160.0/bin/codex
QUAL_PREFIX=(
  /usr/bin/env -i PATH=/usr/bin:/bin
  HOME="$QUAL_ROOT/home" CODEX_HOME="$QUAL_ROOT/codex"
  TERM=xterm-256color LANG=C.UTF-8
  /usr/bin/bwrap --ro-bind / / --bind "$QUAL_ROOT" "$QUAL_ROOT"
  --ro-bind "$QUAL_ROOT/codex/auth.json" "$QUAL_ROOT/codex/auth.json"
  -- /usr/bin/nice -n 10
)
cd "$QUAL_ROOT/cwd"
```

App-server process argv:

```bash
"${QUAL_PREFIX[@]}" "$QUAL_CODEX" \
  -c model_auto_compact_token_limit=12000 \
  -c 'model_auto_compact_token_limit_scope="total"' \
  -c 'model_reasoning_effort="low"' \
  -c check_for_update_on_startup=false \
  app-server --stdio
```

The stdio handshake and fresh thread request were these JSONL shapes; each
request awaited its correlated response before the next dependent request:

```json
{"id":1,"method":"initialize","params":{"clientInfo":{"name":"build_657_qualification","version":"0.1"},"capabilities":{}}}
{"method":"initialized","params":{}}
{"id":2,"method":"thread/start","params":{"cwd":"/tmp/build-657-qualification/cwd","model":"gpt-6.1-sol","approvalPolicy":"never","sandbox":"read-only","config":{"model_auto_compact_token_limit":12000,"model_auto_compact_token_limit_scope":"total"}}}
{"id":3,"method":"turn/start","params":{"threadId":"<thread-id>","input":[{"type":"text","text":"Reply only OK. Do not use tools."}],"model":"gpt-6.1-sol","effort":"low"}}
```

An exact resume replaced the `thread/start` request with this request, retaining
the same two compaction keys and selected model:

```json
{"id":2,"method":"thread/resume","params":{"threadId":"<thread-id>","cwd":"/tmp/build-657-qualification/cwd","model":"gpt-6.1-sol","approvalPolicy":"never","sandbox":"read-only","excludeTurns":true,"config":{"model_auto_compact_token_limit":12000,"model_auto_compact_token_limit_scope":"total"}}}
```

Resume probes also issued `config/read` with `{"includeLayers":true}` before
starting the turn, advancing the subsequent request ID accordingly. The fresh
window probe changed both N occurrences to 25000 and added root
`-c model_context_window=20000`. Diagnostic resume runs additionally supplied
`RUST_LOG=codex_core::session::turn=debug` or `trace` in their constructed
environment; the model and compaction parameters were unchanged.

TUI process argv, attached to the owned 150-column by 40-row PTY:

```bash
"${QUAL_PREFIX[@]}" "$QUAL_CODEX" --no-alt-screen \
  -c model_auto_compact_token_limit=12000 \
  -c 'model_auto_compact_token_limit_scope="total"' \
  -c check_for_update_on_startup=false \
  -c 'model_reasoning_effort="low"' \
  -a never -s read-only -m gpt-6.1-sol \
  resume '<thread-id>' 'Reply only OK. Do not use tools.'
```

TUI evidence covers this exact-resume invocation. It does not claim a separately
executed fresh TUI launch. No additional model runs were made to write these
command templates.

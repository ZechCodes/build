# Codex app-server 0.153.0 fixtures

## Capture binary

All retained captures report `codex-cli 0.153.0`. The package files were
installed before the captures. The new MCP capture and both schema generations
invoked the package entrypoint directly, bypassing the mutable local agent
wrapper. Paths are represented by non-machine-specific labels:

- package launcher `<codex-package>/bin/codex.js` SHA-256:
  `61b0194f3bb6534439c8d26a3ed57d0805f84b884588b761795323eeb92fcf70`
- resolved native executable
  `<codex-package>/vendor/aarch64-apple-darwin/bin/codex` SHA-256:
  `a29d9e86eef88cbbd69f97ce8c590b1d0a287c8f77424f5eef226b883d7eaa22`
- `@openai/codex` package manifest SHA-256:
  `4999d8fd8fda0c4b3656b51bca4ec3ef6a0e42754d1e45b8517721445631237f`
- `@openai/codex-darwin-arm64` package manifest SHA-256:
  `3564c66d95982802cc453c569db7114ba913267b3b55e380ed47f0a06010d2fa`

The two earlier process captures invoked the local `codex` command, whose
wrapper dispatched to this package installation. The native package files'
installation timestamps predate all three captures. The wrapper itself is not
part of the Codex build fingerprint.

## Process captures

Each observed JSONL file contains exactly one app-server process. Every client
request id is unique within its file and has one response. EOF is represented
outside JSONL here because it is transport state, not a protocol message.

`observed-session-start.jsonl`:

- initialize, initialized, thread start, and thread started
- completed reasoning and agent messages
- successful `printf` command with `<process-1>` on start and completion
- in-process exact thread resume
- `sleep 10` command with distinct `<process-2>`, accepted steer, interrupt,
  and interrupted turn completion
- stdin closed to send EOF; app-server exited 0
- two post-interrupt stderr diagnostics concerned cancellation and rollout
  recording; stderr is not checked in

`observed-session-resume.jsonl`:

- initialize, initialized, and exact cross-process thread resume
- completed reasoning and agent messages
- added `fixture-output.txt`
- failed `false` command with `<process-3>` on start and completion
- completed turn
- stdin closed to send EOF; app-server exited 0 with empty stderr

`observed-session-mcp.jsonl`:

- initialize, initialized, thread start, and one completed text turn
- startup and ready notifications for two local stdio MCP fixture doubles
- real `mcpToolCall` start/completion events for `build/done`
- real `mcpToolCall` start/completion events for `fixture_tools/ping`
- stdin closed to send EOF; app-server exited 0 with empty stderr

The build-named MCP server is a fixture double, not Build's daemon. Its `done`
result says `no Build lifecycle transition`, and no lifecycle transition is
claimed. The non-Build `ping` server also returns only a fixed local response.
Both servers ran from harmless test code in the temporary git repository; no
project code, user data, network access, or real Build token was used.

An initial MCP configuration attempt failed before initialize because an
override gave the built-in `codex_apps` entry an invalid transport. It made no
model call and is not represented as an observed protocol session. The
successful capture left unrelated globally configured MCP servers uncalled and
omitted their startup notifications.

## Model Calls

The retained captures contain exactly four authenticated `turn/start` model
turns: two in `observed-session-start.jsonl`, one in
`observed-session-resume.jsonl`, and one in `observed-session-mcp.jsonl`. They
also contain one accepted `turn/steer` input within the interrupted turn. Thus
the exact app-server count is four model turns and five model-bearing client
inputs. App-server does not expose its internal count of upstream HTTP requests
across tool continuations, so no HTTP-request count is claimed.

## Sanitization

Home and temporary paths, host and installation values, operating-system patch
version, git SHA, process ids, timestamps, durations, and thread/turn/item ids
were replaced with fixed values. Command process placeholders are distinct and
stable for each command while preserving start/completion pairing. Embedded ids
in cursor strings were replaced consistently. Account usage, token usage,
unrelated local MCP names, and token deltas were removed. No auth material,
environment dump, account/workspace id, token, URL, real home path, or machine
name is retained.

## Compatibility Fixture

`synthetic-model-events.jsonl` keeps its legacy name because the Rust fixture
test loads that exact path. Lines 1-10 are sanitized observed notifications:
reasoning, agent message, file change, failed command, build-named fixture MCP,
and non-Build fixture MCP. The build-named lines remain fixture-double events.

Lines 11-12 are the only synthetic records. They are retrying and terminal
`error` notifications.

The error pair is synthetic because safely inducing an upstream retry and
terminal failure would require manipulating network or account behavior. Both
lines use one synthetic thread/turn identity and validate against
`v2/ErrorNotification.json`.

## Schema Reproduction And Validation

The exact package entrypoint generated the complete schema tree twice with:

```text
codex app-server generate-json-schema --out <independent-directory>
```

The two directories compared byte-for-byte with no differences. The hashes
below were identical in both generations:

- `codex_app_server_protocol.v2.schemas.json`: `d3eace08be5dca386bfd1f1e8df650058b4113f1e10870a284d775d75517576a`
- `ClientRequest.json`: `25bc001b5dfe3b35785597b8f9ad9e5aaf7e437331fa9921f041c9e0e03fc9f3`
- `ClientNotification.json`: `706cf248d75027c84a3c63348d0ed507182e8eba40069dd17541793de029145a`
- `ServerNotification.json`: `b3e76cf11842f3e8b3270c05e000212b56eabafb0152fc38e8f920e2ef902991`
- `JSONRPCResponse.json`: `4796738c04c74288213a08fb8d820c7b4df19e0977cdcd35b65ffcb43cfc93ab`
- `v1/InitializeResponse.json`: `62ad689c2cb6379913c1d72749cfd8de5089d35760214123518eb92eef11acc9`
- `v2/ThreadStartParams.json`: `792e2f32e37cece971bd616664ea2053741acbed4e9c92e9d1766427718f2ecd`
- `v2/ThreadResumeParams.json`: `8ac68582a81d60940b10b330be8546123f56bfe246b56f8a4f121da00f347cf2`
- `v2/TurnStartParams.json`: `a3835e8c1e942e4b358e1a670939b89918b16c4d13105a579899892b7ade6dea`
- `v2/TurnSteerParams.json`: `4a52eb76e7a717bb388484ccd7538737fca0df35481fc30a21e259f1bfe96e37`
- `v2/TurnInterruptParams.json`: `6dff382dae73d1dbc58406ed045605f647e7a49660e2540fbd2c6c24d60c5f2b`
- `v2/ItemStartedNotification.json`: `c4c34f47db6326cd4841bae428f23d08eb285077ffad35be9772b928c65bb912`
- `v2/ItemCompletedNotification.json`: `69aba3fe5f72f38bf5c541e7e2c09de40778abe65ff969d9fc73372037812091`
- `v2/ErrorNotification.json`: `d77732536ef8864799e4fbb5bf6965a9f1970a6195e523f144252a82287c08e7`

Every retained message was validated with Draft 7 `jsonschema` from transient
`uv` tooling, not a project dependency. Direction wrappers were removed before
validation. Each process capture was also checked for one initialize sequence,
unique pending request ids, matched responses, and no request left pending at
EOF.

## Remaining Gaps

No real Build daemon `done` call or Build lifecycle transition was exercised.
Natural subagent activity, context compaction, retry/error notifications,
collaboration tool calls, and dynamic tool calls remain genuinely unobserved.

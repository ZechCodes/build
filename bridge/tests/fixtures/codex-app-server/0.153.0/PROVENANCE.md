# Codex app-server 0.153.0 fixtures

## Real captures

`observed-handshake.jsonl` is the original initialize-only local capture from
the installed `codex-cli 0.153.0` binary on 2026-09-03. It opened no thread and
made no model request.

`observed-session.jsonl` is a sanitized capture from the same installed binary
on 2026-09-03. The client ran in a temporary git repository containing only a
harmless README and the generated `fixture-output.txt`. It used
`approvalPolicy: "never"`, `sandbox: "workspace-write"`, and an effective
sandbox response with `networkAccess: false`.

The capture spans two app-server processes and retains these observed paths:

- `initialize`, `initialized`, `thread/start`, `thread/started`
- three text `turn/start` requests and completed reasoning/agent-message items
- successful `printf`, interrupted `sleep 10`, and failed `false` command items
- one added-file item for `fixture-output.txt`
- exact `thread/resume` both in-process and across a new app-server process
- accepted `turn/steer`, successful `turn/interrupt`, and interrupted completion
- completed turns and EOF shutdown with process exit status 0 for both processes

Three authenticated model turns were started. One additional `turn/steer`
request was accepted inside the second turn; app-server does not expose the
number of upstream model HTTP requests made within tool-using turns.

The first process wrote two post-interrupt diagnostics to stderr concerning the
cancelled command and rollout recording, then exited 0 after stdin EOF. The
second process emitted no stderr and exited 0 after stdin EOF. Stderr and raw
captures are not checked in.

An empty `mcp_servers` command-line override did not suppress MCP servers from
the local global Codex configuration. Their startup notifications, along with
account limits, token usage, deprecation notices, and token deltas, were omitted
from the retained minimal capture. No MCP tool was called.

## Sanitization

Home and temporary paths, host and installation values, operating-system patch
version, git SHA, process ids, timestamps, durations, and thread/turn/item ids
were replaced with fixed values. Embedded ids in cursor strings were replaced
consistently. Account usage and unrelated local MCP names were removed. No auth
material, environment dump, account/workspace id, token, or URL is retained.

## Compatibility fixture

`synthetic-model-events.jsonl` keeps its legacy name because the existing Rust
fixture test loads that exact path. Lines 1-6 are sanitized observed
notifications copied from `observed-session.jsonl`: reasoning, agent message,
file change, and failed command. Lines 7-11 remain synthetic and cover only
events not safely or deterministically available in this isolated capture:
Build's authenticated `done` MCP call, natural subagent activity, and context
compaction.

No non-Build MCP call, retry/error notification, collaboration tool call, or
subagent activity was naturally observed. Delegation was not forced.

## Schema validation

Every retained protocol message was validated with Draft 7 `jsonschema`, run as
a transient `uv` tool rather than a project dependency. Direction wrappers were
removed for validation. Requests, responses, and notifications were checked
against the generated envelope schema and their method-specific params/result
schema where one exists. The compatibility fixture was checked as server
notifications plus `ItemStartedNotification` or `ItemCompletedNotification`.

Generated schema SHA-256 values from the supplied schema directory:

- `codex_app_server_protocol.v2.schemas.json`: `e5f798fd1343c539f01fedea0e8a84a43c080fcca4615c80eb04a5edab4f7d0a`
- `ClientRequest.json`: `05c82ead1a820c765c23d3a1d262e4ae54889785276ee1acf7c494141fd94d70`
- `ServerNotification.json`: `1a59e2cecb8e7930c4358f7a92245d9d52c96adb0e02b210fc39772ed10610fc`
- `v1/InitializeResponse.json`: `62ad689c2cb6379913c1d72749cfd8de5089d35760214123518eb92eef11acc9`
- `v2/ThreadStartParams.json`: `25f490368ec6df52a2a3b82a5469d2413307eb93439121b309f415b5648eee7a`
- `v2/ThreadResumeParams.json`: `324e96004c49de35935cade3386958431c93a4fd3997a839f9796772ea4c8072`
- `v2/TurnStartParams.json`: `b36fb37326b1cf69f75c8b306f1f886d53a57c4b1b985e08e298e2407ea2ad02`
- `v2/TurnSteerParams.json`: `2e0cdcea6a90d6c8bc584fdc2ff838e824754b1eef0d2d16aa71bec4276fef44`
- `v2/TurnInterruptParams.json`: `6dff382dae73d1dbc58406ed045605f647e7a49660e2540fbd2c6c24d60c5f2b`

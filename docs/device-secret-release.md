# Session secret release

This is a private device-local protocol for trusted launched Build agents. It
has no MCP tool, HTTP route or browser value-read verb. Agent scope authorizes
the exact agent, canonical conversation and generation. Project scope authorizes
current agents of that canonical project, including its coordinator. A secret ID,
workspace path, CWD, task assignment or caller-supplied project selector confers
no authority.

Each agent launch receives its own purpose-limited capability in dedicated
`BRIDGE_SECRET_*` environment variables. It is separate from the MCP token and
bound to device, channel, owner, canonical project, conversation, generation and
session incarnation. Reset/removal invalidates it; a same-generation restart
receives a fresh capability and retains the grant. Descendants using that launch
context act as the parent agent. Terminal tabs and executable probes receive no
agent secret capability. Build agents are trusted with values they retrieve;
raw output can enter tool transcripts or terminal history.

## Private socket protocol

The socket path is the configured MCP control path plus `.secrets`, mode 0600.
The runtime CLI slice consumes this protocol; it must use the injected socket,
never discover an agent or device from a directory or scan service state.

One connection carries one newline-terminated JSON request (at most 2,048 bytes,
including newline). Only these fields are accepted:

```json
{"agent_id":"agent-example","capability":"<launch capability>","secret_id":"sec_00000000-0000-4000-8000-000000000001","request_id":"00000000-0000-4000-8000-000000000002"}
```

Request IDs are canonical UUIDs. The capability is authentication material; it
must never enter argv, diagnostics, message text, history or audit records.

Success starts with `{"ok":true,"length":N}\n`, followed by exactly N raw bytes
(up to 16 KiB), with no extra newline. Failure is one fixed JSON line:
`{"ok":false,"error":"<code>"}\n`. Codes are `unauthorized`,
`secret_unavailable`, `secret_expired`, `secret_revoked` and `secret_limit`.
Malformed or oversized requests fail as `unauthorized`. Unknown IDs, wrong
scope/project/generation and unavailable/corrupt vaults share
`secret_unavailable`. Only an authenticated, currently authorized recipient may
learn an expired/revoked status. No request or value is echoed.

There are at most four active connections/jobs, a five-second input/output
budget, 64 admitted requests per second in total, and 16 per capability per
second. Excess connections close immediately. Rate counters retain only a
bounded set of capability digests for the current window, not capability text.
A disconnected consumer can have a committed release audit without receiving
all bytes; an audit records release authorization, not downstream use.

## Ordering and audit

Release checks current issued-session authority and committed membership, then
checks scope before status. Its metadata transaction serializes against removal,
reset, project retirement, revoke and expiry. Every authorized release is
FULL-committed to SQLite before any value bytes are sent. A failed audit write
refuses the release. A release already committed may finish after revocation;
a later release cannot pass its tombstone.

Audit contains IDs, resolved caller/generation/session incarnation, request ID,
time and outcome. It contains no values, value hashes, command arguments or
child output. Authorized releases remain individually attributable; invalid
capability failures are coalesced. Maintenance drops detailed events older than
90 days while retaining summaries (last release, count and status). It runs
outside the global app lock.

This slice does not advertise public secret capabilities or enable uploads,
message attachment admission or the installed `buildctl secrets get` facade.
Those are separate slices.

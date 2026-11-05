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

The injected variables are `BRIDGE_SECRET_SOCKET` (exact local socket),
`BRIDGE_SECRET_CHANNEL` (the CLI `--env` binding), `BRIDGE_SECRET_AGENT`
(request `agent_id`), `BRIDGE_SECRET_OWNER` (informational only), and
`BRIDGE_SECRET_TOKEN` (request `capability`), plus `BRIDGE_SECRET_BRIDGE`
(the absolute executable used by the repository facade). The server derives owner, project,
device and generation from its issued record; those are never request selectors.

## Private socket protocol

The socket path is the configured MCP control path plus `.secrets`, mode 0600.
The runtime CLI slice consumes this protocol; it must use the injected socket,
never discover an agent or device from a directory or scan service state.

One connection carries one newline-terminated JSON request (at most 2,048 bytes,
including newline). Only these fields are accepted:

```json
{"agent_id":"agent-example","capability":"<launch capability>","secret_id":"sec_00000000-0000-4000-8000-000000000001","request_id":"00000000-0000-4000-8000-000000000002"}
```

Client request IDs are canonical UUIDs used only for bounded request admission;
they are never copied into durable audit. Each new audit group receives a fresh
bridge-generated UUID, independent of the client request ID.
The capability is authentication material; it
must never enter argv, diagnostics, message text, history or audit records.

Success starts with `{"ok":true,"length":N}\n`, followed by exactly N raw bytes
(up to 16 KiB), with no extra newline. Failure is one fixed JSON line:
`{"ok":false,"error":"<code>"}\n`. Codes are `unauthorized`,
`secret_unavailable`, `secret_expired`, `secret_revoked` and `secret_limit`.
Malformed or oversized requests fail as `unauthorized`. Unknown IDs, wrong
scope/project/generation and unavailable/corrupt vaults share
`secret_unavailable`. Only an authenticated, currently authorized recipient may
learn an expired/revoked status. No request or value is echoed.

There are at most four active connections/jobs, a 250-ms pre-auth frame
deadline, a five-second output budget, 64 admitted requests per second in total, and 16 per capability per
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

Audit contains resolved metadata IDs, caller/generation/session incarnation,
a bridge-generated audit request UUID, time and outcome. Unknown and wrong-scope
targets use the fixed `unavailable` sentinel; neither their supplied secret IDs
nor client request IDs enter audit. Known targets and summary keys come only
from the authorized stored record. It contains no values, value hashes, command arguments or
child output. Authorized releases remain individually attributable. Authenticated denials
coalesce per resolved caller/session, target and outcome in one-minute windows,
with first/last timestamps and a saturating attempt count. Unknown and wrong-scope
targets share the same fixed sentinel group. At most 4,096 denial groups are
retained; insertion evicts the oldest denial group, preserving release rows and
per-secret totals. Internal bounded keyset readers support per-secret and
indexed per-caller audit pages, including `unavailable` targets. These readers
carry metadata only and grant no retrieval authority. Invalid capability
failures are coalesced. Maintenance drops detailed events older than
90 days while retaining summaries (last release, count and status). It runs
outside the global app lock.

## Runtime command

`buildctl --env dev secrets get --id sec_…` (or `--env prod` for that
channel) writes the exact value bytes to stdout, with no added newline. Both
the installed launcher and repository management facade exec the bridge's
`secrets get` command. The installed launcher needs only a POSIX shell and its
sibling bridge; it does not need Python, Cargo, Git or a checkout. Missing or
stale capability context and an environment mismatch fail as `unauthorized`.
Errors are fixed bounded stderr messages and never include values or envelopes.
Production deployment restrictions in the management CLI remain unchanged.

The CLI reads no device keys or daemon state and does not select an agent from
CWD. It buffers a complete size-bounded response before writing stdout, uses an
absolute socket deadline, and writes no temporary credential file. Broken pipe,
spawn failure and process termination leave no credential file to clean up.
Retrieved bytes are deliberately printed; downstream tools/transcripts can
capture them. The installer, self-update, deployment and rollback install the
launcher beside the selected bridge, refusing to overwrite an unrelated CLI.

This slice does not advertise public secret capabilities or enable uploads or
message attachment admission. Those are separate slices.


## Codex shell capability and on-disk exposure

Codex filters credential-shaped environment names before shell commands run.
Build retains those default exclusions and excludes other `BRIDGE_*` variables,
then restores only the six dedicated secret-launch variables and the launcher
PATH. The MCP token is not part of that restored set. Capability bytes never
enter launch argv.

For the Codex TUI, a unique `build-secret-<uuid>.config.toml` profile in the
existing `CODEX_HOME` carries this narrow shell configuration. This is an
explicit on-disk exposure of the session capability, not a retrieved value.
The profile is created with exclusive atomic 0600 permissions, used by plain
profile name in argv, and removed on session end, process exit or failed spawn.
Startup cleanup removes only unlocked regular files owned by the current user
whose names match Build's UUID profile pattern. Live profiles are protected by
an advisory lock, including across bridge channels. User config, auth, history
and unrelated profiles stay in place and are not rewritten. Crash leftovers
can remain until the next sweep; revocation and session rotation still fence
the old capability independently of file deletion.

The Codex app server receives the same narrow configuration over private stdio
at thread start and resume. It does not use a profile file. A documented offline
provider persistence check covers rollout/session files and default logs; the
ready report records the tested version and result. Custom provider logging,
OS-user access and an agent deliberately printing or saving its own capability
are outside this storage boundary.

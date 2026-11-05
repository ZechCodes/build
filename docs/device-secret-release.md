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
parent-directory bridge (`../build-bridge`); it does not need Python, Cargo, Git
or a checkout. Missing or
stale capability context and an environment mismatch fail as `unauthorized`.
Errors are fixed bounded stderr messages and never include values or envelopes.
Production deployment restrictions in the management CLI remain unchanged.

The CLI reads no device keys or daemon state and does not select an agent from
CWD. It buffers a complete size-bounded response before writing stdout, uses an
absolute socket deadline, and writes no temporary credential file. Broken pipe,
spawn failure and process termination leave no credential file to clean up.
Retrieved bytes are deliberately printed; downstream tools/transcripts can
capture them. The installer, self-update, deployment and rollback install the
launcher in the selected bridge's `.build-runtime/` subdirectory. It never
writes a global `buildctl`. A foreign runtime destination is preserved and
skipped with a warning; its directory is not added to agent PATH. Rollback,
older-ref deployment and pinned installs tolerate older bridges that lack the
launcher subcommand.

This slice does not advertise public secret capabilities or enable uploads or
message attachment admission. Those are separate slices.


## Codex shell capability and on-disk exposure

Codex applies its configured shell environment policy before commands run.
Build extends the user's effective policy: inheritance and custom set values
remain, `BRIDGE_*` joins exclusions, and the six dedicated names join any
existing include allowlist. Only those six launch values are restored. The MCP
token is not part of that restored set. PATH is prepended only in the harness
process environment when the private Build launcher is present; user shell
PATH policy remains authoritative. Capability bytes never
enter launch argv.

For the Codex TUI, a unique `build-secret-<uuid>.config.toml` profile in the
existing `CODEX_HOME` carries this narrow shell configuration. This is an
explicit on-disk exposure of the session capability, not a retrieved value.
Optional secret setup never blocks an ordinary Codex launch. A symlinked home
or ancestor, unavailable/legacy/unsupported config probe, timeout, registry or
profile failure logs one fixed reason without values. Build strips all dedicated
secret variables, the overlay and its injected runtime PATH, preserving the
user's ordinary launch and MCP settings. Existing user directories use normal
filesystem resolution; Build does not walk or change their ancestors. A missing
final home leaf may be created with 0700. No-follow/private permissions apply to
Build-created registry storage and profile entries.

The profile is created with exclusive 0600 permissions and selected by plain
name in argv. Build first creates an empty pending entry, writes a durable
private registry record, then writes capability bytes and publishes without
replacing an existing entry. Records contain configured home, exact final and
pending names, file/parent device and inode IDs and owner UID; never a capability
or retrieved value. The registry lives at `<harness-state>/secret-profiles`,
with at most 4,096 records of at most 16 KiB each. Serialized admission prunes
stale entries at the limit; live locks are retained. Concurrent admissions wait
behind an in-process mutex, then retry the close-on-exec file lock for up to
one second; admission release explicitly unlocks before close, including across
sibling forks. Registry exhaustion or a lock held beyond the deadline degrades
secret setup rather than agent startup.

Session end removes exact matching entries through the held directory handle
and retires the record. Its metadata lease is explicitly unlocked, so an
inherited close-on-exec duplicate cannot outlive the guard logically. A profile
descriptor can also survive briefly between a concurrent fork and exec; quota
admission boundedly resweeps all full-capacity records, while ordinary cleanup
retries only records whose metadata guard has ended. Cleanup's own metadata
leases also unlock explicitly across every return. A genuinely live profile
keeps both its record and quota. Startup enumerates only the Build-private
registry, opens each recorded configured home normally (following the user's
own links),
checks parent dev/ino, and uses no-follow fstatat plus exact file dev/ino before
unlinkat. A missing, replaced or moved target drops only the record. No user
Codex home is enumerated or pattern-deleted; unregistered files remain untouched.
The identity check occurs immediately before unlinkat, relative to the held
parent fd. There is a remaining check-to-unlink window: Linux provides no atomic
unlink conditioned on an inode. A same-user process replacing that exact name
inside this window may have its replacement removed. This is accepted within
the same-user trust boundary: “Securing the machine is up to the user.” Build
never follows an entry symlink, and a replacement observed before the check is
left alone. Live profiles retain advisory locks. User config, auth and history
stay in place.
Registered crash leftovers are removed on a later sweep if identities still
match. Retired session capabilities remain dead even if a user moved the file
or permissions prevent deletion.

The Codex app server receives the same narrow configuration over private stdio
at thread start and resume. It does not use a profile file. Offline qualification on Codex 0.166.0 completed real shell-tool turns against
a loopback fake Responses provider, then restarted and resumed with a rotated
capability. Rollout/session and default-log scans found no copies of either
capability. A profile-selected Codex exec turn likewise found the capability
only in the expected TUI profile, and zero copies after removing that file.
Shell commands verified that MCP auth, unrelated Bridge variables and an AWS
credential were absent. The task's ready report retains the reproducible
check drivers and scan evidence. Custom provider logging,
OS-user access and an agent deliberately printing or saving its own capability
are outside this storage boundary.

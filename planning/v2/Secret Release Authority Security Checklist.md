# Secret release authority and audit security checklist

Scope: #1002, the private local session authority and audit layer, on top of
#1001's encrypted vault. Public uploads, message admission, runtime CLI
installation and SPA interaction remain separately reviewed slices.

**Scoped score: 100/100.** Verification has been refreshed after the
caller-controlled audit identifier review finding. The new regressions and
revised validation below cover bridge-owned audit identities and resolved
record targets. The final task report
records the frozen commit, commands, actual exit codes and attached evidence.
No public capability is advertised by this change.

| # | Control | Evidence |
|---|---|---|
| 1 | Purpose-limited capability is independent of MCP and binds device/channel, canonical project/owner/conversation/generation and process incarnation. | `runtime/sessions/secret_authority.rs`, `secret_release_authority.rs`; token independence, selector spoof, device-rebind and same-generation restart tests. |
| 2 | Authority derives from the issued record and current committed membership; no caller selector, CWD, path or task grants access. | `read_context/mcp_addresses.rs`, `store/secrets/release.rs`; saved/candidate membership, removed member, canonical and alias project rebind, durable generation regressions. |
| 3 | Agent grants require exact recipient/generation; Project grants allow current members including coordinator; cross-project and wrong-scope reads disclose no existence/status. | Scope and caller matrices in `secrets/release_tests.rs`; scope precedes expiry/revocation lookup outcomes. |
| 4 | Capability material enters only dedicated launch environment. Every provider strips inherited capability prefixes; probes and ordinary terminal tabs acquire no agent authority. | `harness/secret_launch.rs`, PTY/ADK/app-server spawn integration, installed probe test; HarnessSpec Debug redacts argument and environment values. |
| 5 | Private mode-0600 Unix socket is separate from MCP/HTTP/browser RPC. Live or full-backlog sockets are not replaced; arbitrary entries/symlinks are preserved. | `app/secret_release.rs`; protected binding and full-backlog bounded-probe tests. |
| 6 | Frames, admitted connections, jobs, rate state and I/O time are bounded. Only strict request fields and canonical request UUIDs are accepted. | `app/secret_release/transport.rs`; oversized/malformed/selectors, pipelining, rate and actual idle-client admission tests; shared four-job vault budget. |
| 7 | Scope/membership/generation/revision and final TTL are checked at the durable release linearization point shared with revoke/reset/removal. | `secrets/release.rs`, `store/secrets/release.rs`; concurrent revoke, final expiry sampling, persistent-generation and metadata-change tests. |
| 8 | Every authorized release is FULL-audited before bytes; failed or ignored audit/summary writes refuse release. Audit is metadata only, without value, value hash, argv or output. Audit request IDs are bridge-generated; target and summary keys come from authorized records, with a fixed unavailable target for denied selectors. | Trigger-failure/ignored-write tests, audit restart and serialization checks; actual launch-capability/UUID-value injection and denied-selector regressions; transport fixed-error test confirms no value bytes on audit failure. |
| 9 | Release, decryption, membership and audit work run outside the app lock. Vault filesystem work and all socket I/O run outside the SQLite writer lock; stalled clients cannot hold it. | Predecrypt/final-audit split, blocking-worker endpoint; real service+socket stalled-client test performs writer maintenance while output is blocked. |
| 10 | Unauthenticated failures are coalesced even on database failure. Detailed audit retention is 90 days with bounded cleanup throughput above admission, while summaries persist. | Coalescing/retry-storm, bounded-pagination, restart, pruning/cleanup-throughput tests; off-lock periodic sweep. |

Raw values are intentionally released to trusted launched agents and descendants
using their launch context. Such output may enter transcripts or terminal
history. The at-rest and normal-delivery guarantees do not promise secrecy from
that agent, the same OS user/root, debuggers or user-created disk snapshots.

## Validation — 2026-11-05

All commands ran under `nice -n 10`, with scratch, `TMPDIR` and Cargo target
inside `/home/zech/Projects/.agent-scratch/secrets-b/`.

- `cargo test --lib secret -j 2`: 146 passed, 0 failed or ignored, exit 0.
- Cargo-built library binary: 23 focused probe, schema migration, session
  registry and existing MCP authority checks passed; every process exited 0.
- `cargo clippy --all-targets -j 2 -- -D warnings`, `cargo fmt --check`,
  locked offline nested HTTP harness metadata, Semgrep and diff checks exited 0.
- Gitleaks runs before the task-branch push; its exit and remote verification
  are attached to the final task report.

The initial integrated run recorded the coalesced-failure regression red;
a production-source probe against the pre-fix library recorded cleanup of
1,000 instead of 8,000 rows. Corrected tests verify both fixes. The stalled
client regression confirms committed audit remains readable and maintenance
can write while a real secret reply is blocked by socket backpressure.
Independent security review closed the backlog, canonical-owner map and
retention-throughput findings. No new complexity ratchet annotations were added.
The full suite remains the coordinator's serial merge gate.

## Review revision — caller-controlled audit identifiers

The audit sink no longer accepts raw request IDs or target selectors. It
assigns each attempt a fresh server UUID and derives known targets and summary
keys only from an authorized resolved record; all other targets use the fixed
`unavailable` sentinel. The socket shape remains compatible; client request IDs
are validated for admission and never persisted.

Two new normal Cargo regressions failed before implementation (exit 101).
One uses an actual issued launch capability and an actual UUID-shaped stored
value as client request IDs, including a retry. The other sends unknown and
wrong-scope selectors. The revised run passes **148 tests, 0 failed/ignored,
exit 0**, including all existing durability, scope, retention and stalled-client
checks. Clippy all-targets `-D warnings`, fmt, the eight-path Semgrep scan,
locked offline harness metadata and diff checks passed. Gitleaks runs before
the revised task-branch push; the frozen-head report records its actual exit.
Read-only follow-up review found no remaining audit identifier injection path.

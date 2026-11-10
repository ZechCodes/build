# Secret release authority and audit security checklist

Scope: #1002, the private local session authority and audit layer, on top of
#1001's encrypted vault. Public uploads, message admission, runtime CLI
installation and SPA interaction remain separately reviewed slices.

**Scoped score: 100/100.** Verification has been refreshed after the
caller-controlled audit identifier review finding. The new regressions and
revised validation below cover bridge-owned audit identities and resolved
record targets. The final task report records the frozen commit, commands, actual exit codes and attached evidence.
No public capability is advertised by this change.

| # | Control | Evidence |
|---|---|---|
| 1 | Purpose-limited capability is independent of MCP and binds device/channel, canonical project/owner/conversation/generation and process incarnation. | `runtime/sessions/secret_authority.rs`, `secret_release_authority.rs`; token independence, selector spoof, device-rebind and same-generation restart tests. |
| 2 | Authority derives from the issued record and current committed membership, including the config-owned environment/archive projection; only the short config-membership fence denies environment-dependent reads; registry jobs never block secrets. No caller selector, CWD, path or task grants access. | `read_context/mcp_addresses.rs`, `store/secrets/release.rs`; saved/candidate membership, removed member, canonical and alias project rebind, durable generation regressions; real AppState workspace-member and project-coordinator probes pass the SQL boundary. |
| 3 | Agent grants require exact recipient/generation; Project grants allow current members including coordinator; Environment grants allow current unarchived member projects, with project-name precedence; cross-project and wrong-scope reads disclose no existence/status. | Scope and caller matrices in `secrets/release_tests.rs`; scope precedes expiry/revocation lookup outcomes. |
| 4 | Authority originates in the dedicated launch environment. Runtime CLI follow-up #1004 restores the Codex shell capability through a private per-launch TUI profile or app-server stdio config, as documented in `docs/device-secret-release.md`. Every provider strips inherited capability prefixes; probes and ordinary terminal tabs acquire no agent authority. | `harness/secret_launch.rs`, PTY/ADK/app-server spawn integration, installed probe test; HarnessSpec Debug redacts argument and environment values. |
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

## Gating review follow-up — real membership probes

Committed probes mint a real AppState workspace agent and the project
coordinator, authenticate each issued capability, and call the durable writer
release boundary with an unknown ID. Each returns `Unavailable`, proving it
passed membership rather than returning `Unauthorized`. They do not seed SQL
membership rows by hand. The launch context now uses named fields and explicit
optional device identity; startup validates private Unix socket paths and
reports an overlong/invalid path specifically.

The final expanded targeted run passes **150 tests, 0 failed/ignored, exit 0**.
Clippy all-targets `-D warnings`, fmt, the ten-path security scan, locked offline
HTTP harness metadata and diff checks passed. Earlier 23 focused compatibility
checks remain recorded on #1002. The pre-auth slot hold, authenticated-denial
coalescing and runtime vault-capture follow-ups are unchanged, as directed for
#1004. The final task report records the revised frozen head and Gitleaks before
its push. The full suite remains the serial merge gate.

## S2.1 — environment membership and lifecycle (#1095)

The scoped score remains **100/100**. `environment_scoped_secret_readable_across_projects`,
`environment_membership_revalidated_per_read` and
`archived_project_agent_not_current_for_secrets` exercise the writer release
boundary with current config-derived membership. `project_secret_overrides_environment_name`
checks precedence, including direct-ID denial of the hidden inherited grant.
The four `sibling_project_cannot_read_project_scoped_secret` cases cover release,
list, attach and audit; wrong-scope release keeps the fixed unavailable audit
target. `environment_delete_revokes_environment_scope` proves idempotent audited
retirement through the store hook; S2.2 integrates environment deletion.
`schema_26_to_27_preserves_secret_rows_and_reopen_is_noop` and
`schema_26_to_27_populates_membership_from_current_config_only` preserve old state
and rebuild only the disposable config projection. Raw release, sealing and
vault AAD retain their existing contracts. Task #1095 records final evidence
and gate exit codes; capability advertisement remains with S2.4.

Review #1104 adds explicit legacy-list opt-in and compatible project invalidation
fanout, without changing release authority. The original project value and grant
are checked after fresh sealed uploads, and both the retirement hook and actual
`project.delete` entry point preserve the sibling's environment grant. Schema 27's
DDL, indexes, config projection and version fence now share one transaction;
`schema_27_failure_rolls_back_schema_indexes_version_and_projection` verifies exact
old database bytes after an injected failure through full Store open, then a successful retry.
`schema_26_startup_populates_membership_from_config_before_publication` verifies
startup population rather than a manual reconciliation after opening.

The re-review requires loaded authoritative config for every existing pre-27
store. Missing/unreadable-config and fresh-install tests cover the daemon entry
path. `schema_26_open_without_authoritative_config_refuses_without_writes` checks
the bare Store refusal; the CLI backup regression copies populated schema 26
through a read-only source without migration. Membership compensation covers
leavers, joiners before/after delayed grant notification, and zero-member All
subscribers. Release authority and the S2.2 retirement seam are unchanged.

Membership fan-out now captures plain committed data under app and queues only
through the existing off-lock flusher after unlock/SQL fence clear. Actual
list/audit fence refusals retain bounded project/All retry scopes; each dirty
interval and late old-snapshot refusal re-arms a post-clear hint. The four
`device_lifecycle_tests::secrets_*` regressions cover these orderings without
changing release authority or the S2.2 retirement seam.


Gate #1106 qualification: legacy Project/Agent/stage operations ignore registry
and membership fences. Environment operations alone consult the dedicated
config-membership fence; `include_environment:true` refuses the whole list and
records a post-clear retry. The real `settings.set` regression and retained
consecutive/late-snapshot tests exercise this fence. Real workspace-creation and
review-opening jobs held in flight allow project create/release/list/audit/revoke
and environment release/list (and create/audit/revoke).
`legacy_secret_scopes_ignore_registry_and_membership_fences` also covers legacy
agent release and staged create/list while the membership fence is dirty.

Archived projects retain project list/audit/revoke; create and release report
`project_archived`, and an archived member cannot release an environment grant.
Evidence: `archived_project_secret_metadata_and_revoke_remain_available_create_reports_archive`,
`archived_project_keeps_metadata_and_safety_revoke_but_not_release`, and
`archived_project_agent_not_current_for_secrets`.
`project_revoke_and_audit_cannot_reach_an_environment_grant` proves project
selectors refuse shared grant IDs just like unknown IDs, preserve metadata/value,
and explicit environment selectors retain audit/revoke authority.

Historical migration fixtures supply their synthetic authoritative config; the
partial-migration regression asserts the injected malformed-JSON SQLite error.
Task #1095 records the widened complete `store::` and `app::tests` results.

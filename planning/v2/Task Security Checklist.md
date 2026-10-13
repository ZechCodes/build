# Task implementation security checklist

**Status:** 100/100 (10/10 controls verified)
**Verified:** 2026-07-30

Task body checklist edits re-verified 2026-10-03 (#347): the SPA uses the existing
authenticated `tasks.update` body write with optional `expected_body_hash`,
announced by `tasks.bodyPrecondition` in wire 3.10.0. Every checklist tick sends
the SHA-256 digest of the exact original body bytes. The bridge checks it under
the app lock before any task field or timeline write; a mismatch refuses the
whole update with nonretryable `stale_body`. This server check protects edits
made by another device or MCP agent, independently of the SPA's local cache
guards. A refusal conditionally rolls back only the optimistic cached body and
refreshes the task; it never retries the old body. The cached capability controls
availability, and the current greeting gates dispatch of the new parameter.
The hash requires `body`: status/priority/label-only updates carrying it refuse
with `invalid_params` before writing anything.
Bridge tests verify server divergence, no partial writes and exact UTF-8/CRLF
hashing. SPA tests separately cover server/cache divergence and refresh, local
stale reads, newer cache writes, comment immutability and keyboard saving. The
markdown edit replaces only the parsed marker's checked character. Existing
body size and task ownership checks continue to apply.

This checklist covers the security boundary changed by the Task final-repair work. Product/lifecycle completeness is tracked separately from this security score.

| # | Control | Score | Verification |
|---|---|---:|---|
| 1 | MCP daemon socket is explicitly owner-only (`0600`) | 10/10 | `done_socket_is_explicitly_owner_only` |
| 2 | MCP lifecycle and conversation frames require the current per-session capability | 10/10 | `mcp_control_frames_require_the_current_session_token` |
| 3 | Session capabilities rotate when an agent process is replaced and are never logged | 10/10 | agent-tab spawn path and harness-spec tests |
| 4 | A reused checkout is a registered worktree at the exact canonical managed path | 10/10 | `restore_rejects_an_existing_unregistered_directory` |
| 5 | Reused checkout common-dir, registered name, exact branch, HEAD, and ancestry are verified | 10/10 | `WorktreeManager::verify_existing_worktree` and restore suite |
| 6 | Branch recovery never falls back to the moving base | 10/10 | local/remote original-branch restore tests |
| 7 | Recovery fetch is noninteractive and bounded | 10/10 | `GIT_TERMINAL_PROMPT=0`, `GCM_INTERACTIVE=Never`, null stdin, 30-second deadline |
| 8 | Agent-supplied typed references are shape checked and Task-owner scoped; `expected_body_hash` requires `body`, otherwise `invalid_params` | 10/10 | MCP typed-link ownership and canonical-link round-trip tests; `a_task_body_precondition_requires_a_body_and_a_lowercase_sha256_hash` |
| 9 | File references remain traversal fenced and commit references are recorded immutable boundaries | 10/10 | thread-link validation tests and boundary ownership checks |
| 10 | Repository scanners are clean | 10/10 | `semgrep --config auto .` (0 blocking findings); Gitleaks full-history scan (0 leaks) |

**Total: 100/100.**


## #522 planned extension — not yet verified

These rows are proposed by [Project workspace](Project%20Workspace%20Spec.md).
Existing scores above cover their original implemented scope only. Each owning
child must supply passing regression/gate evidence before its new rows can
count toward 100/100; planning does not satisfy these controls.

| ID | Pending control | Required evidence |
| --- | --- | --- |
| TS522-1 | Existing/new project worker assignment and MCP tools are same-project scoped, idempotent and purpose-authorized; clients cannot mint or demote the pin. | C4/C5/C8: assign_project_worker_is_idempotent; cross-project, forged-purpose, removal and task-assignee tests. |
| TS522-2 | Task links preserve exact worker identity without a fake workspace; worker removal and project deletion retire pending delivery/tracking consistently without rewriting history. | C5/C10: assignment/link/cleanup and stale-generation lifecycle tests. |

C5 evidence (#533; the rows stay pending until C8/C10 add theirs):

- TS522-1: `assign_project_worker_is_idempotent` (existing and new worker,
  `operation_id` retry delivers once and makes no second worker, a reused id
  on another assignment is `conflict`), `assigning_refuses_a_foreign_or_removed_project_worker`,
  `a_worker_assigns_a_task_to_a_new_project_worker`,
  `project_workers_list_and_add_project_agents_with_coding_tools`,
  `a_worker_cannot_administer_sources_or_retire_the_pin_through_mcp` (a forged
  `project_role` on `add_project_agent` makes a worker), contract test
  `project_worker_assignment_is_announced_with_its_kind_and_operation_id`.
- TS522-2: dispatches to a worker link its owner and no workspace
  (`assign_project_worker_is_idempotent`),
  `removing_a_worker_keeps_its_tasks_record_and_drops_its_queued_turn`,
  `a_stale_worker_session_cannot_assign`, and
  `project_deletion_retires_workers_and_preserves_sources` (tokens revoked,
  queued turns dropped).
## #558 planned extension — not yet verified

These rows follow Zech's 2026-10-12 decisions in
[Tasks](Tasks%20Spec.md#waiting-on-decisions-and-delivery-558).
Done or closed releases a wait; delivery must distinguish closed-without-Done
prerequisites and ask the receiving agent to assess them and escalate with
`mention_user` when they do not clearly permit the work. Existing scores above
remain scoped to implemented controls. W1–W5 are the children in that plan;
each supplies regression and gate evidence before its rows count as verified.
Planning does not earn a security score.

W1 / #560 evidence (2026-10-12): the focused `cargo test -j2 --lib waiting_on
-- --test-threads=2` run passes. TS558-1's store boundary is covered by
`waiting_on_same_project_dag_is_atomic` and
`waiting_on_mixed_link_write_and_reconciliation_page_roll_back`: foreign and
missing endpoints refuse without task, link or timeline partial writes;
reverse reads refuse foreign projects and mismatched cursors. TS558-5's store
boundary is covered by the same DAG test and
`waiting_on_concurrent_opposite_edges_cannot_cycle`, with distinct SQLite
connections, resolved-edge cycles, deduplication and the 20-edge bound.

TS558-3's W1 provenance evidence is `waiting_on_close_resolution_reasons`,
`waiting_on_pending_fan_in_preserves_the_final_cause`,
`waiting_on_pending_fan_in_retains_close_cause_after_rename`,
`waiting_on_pending_fan_in_retains_close_cause_after_closed_move`,
`waiting_on_pending_fan_in_retains_done_cause_after_close`,
`waiting_on_a_readded_edge_is_retained_at_unblock`,
`waiting_on_fan_out_resumes_after_restart_and_invalidates_titles`, and
`waiting_on_reconciles_at_startup_without_a_client`. Ordinary moves, explicit
review completion and workspace finish share the writer in
`waiting_on_manual_review_completion_and_workspace_finish_share_transitions`.
`waiting_on_migration_defaults` opens a schema-15 database with old assignments
and no dependency tables; migration adds no waits, transitions or delivery
authority. These are W1 proofs only: TS558-1/3/5 still require their W2/W3
authority and RPC/MCP evidence before the extension is fully verified. W1
announces no waiting-on capability and provisions no assignment resources.

The TS558-3 delayed fan-in regression queues B's Done, C's close and C's
rename before draining. It proves that A's single durable unblock event
retains both lifecycle cause IDs and C in `closed_without_done_task_ids`,
and a repeated drain emits no duplicate.
`waiting_on_pending_fan_in_retains_close_cause_after_closed_move` queues B's
Done, C's close in Backlog, and C's Backlog-to-Ready move while still closed.
`waiting_on_pending_fan_in_retains_done_cause_after_close` queues B's Done,
C's Done and then C's close. Both reopen the store before the first bounded
page, verify that A's only unblock contains the actual releasing event IDs
and excludes the later non-releasing event, and verify close/Done precedence
and repeated-drain idempotence. The writer records whether resolution changed
before replacing the task record; a partial index seeks those recorded
transitions rather than the newest lifecycle activity.
`waiting_on_dependent_edits_invalidate_reverse_blocks` verifies that renaming,
moving and closing A invalidate B's reverse Blocks digest after A waits on B;
the affected prerequisite writes share the task transaction and the 20-edge
bound. W2/W3 delivery authorization and forged-notice evidence remain pending.

| ID | Pending control | Required evidence |
| --- | --- | --- |
| TS558-1 | Dependency create/add/remove and reverse reads stay within the caller's project; mixed valid/foreign writes fail atomically without exposing foreign titles or identities. | W1/W3: `waiting_on_same_project_dag_is_atomic`, RPC/MCP cross-project create, add, remove, read and partial-write refusal tests. |
| TS558-2 | Deferred delivery revalidates the saved target's project and current authority before provisioning and after off-lock work; missing/retired targets never fall back to another agent. | W2 verified: `waiting_assignment_defers_every_target`, `deferred_delivery_revalidates_foreign_target_before_admission`, `deferred_delivery_removed_target_never_substitutes_another_agent`, `deferred_delivery_retired_sender_preserves_authority_failure`, `deferred_workspace_creation_retry_refuses_retired_saved_owner` and `deferred_receipt_recovery_fences_missing_retired_and_foreign_targets` cover every target, off-lock recovery, retirement and foreign bindings. `deferred_provisioning_excludes_workspace_lock_and_rename_until_settlement` proves background provisioning holds filesystem mutation exclusion through settlement, refuses reclaim while waiting on source SyncLock, and accepted edits persist afterwards. `deferred_workspace_pins_saved_git_inputs_while_source_is_held` proves current and changed-base reservations wait on the saved repository lock, pin its commit, keep the app mutex available and preserve queued worker source-location holds. W3 caller-facing scope/compatibility evidence remains #562. |
| TS558-3 | Only a committed same-project readiness change (Done, close or edge removal) with a current authorized assignment, or an explicit ready assignment, can enqueue delivery; closed edges retain resolved-by-close provenance. Caller-authored notices, pushes or forged close/unblock/generation/operation fields grant no authority. | W1 provenance evidence above is retained. W2 verified: `durable_delivery_generation_and_live_graph_fence_saved_work`, `delivery_admission_fences_graph_writes_and_failed_retry`, `delivery_admission_binds_frozen_payload_address_and_origin` and `deferred_delivery_operation_id_cannot_admit_a_forged_payload_or_foreign_owner` refuse stale graph/generation and forged operation, address, payload and origin facts. `deferred_delivery_startup_runs_without_rpc_or_connected_client` proves startup handoff without a client. `pending_provisioning_does_not_self_wake_idle_reconciliation` holds provisioning in flight, bounds empty reconciliation polls to zero immediate repolls, rejects unrelated-write wakeups and retains new-assignment wakeups. W3 RPC/MCP contract evidence remains #562. |
| TS558-4 | Removing the last edge can release only a still-current explicit assignment; unassigned, cancelled, superseded and consumed assignments cannot start work. UI states the consequence of removing an armed final blocker. | W2 verified: `removing_last_edge_delivers_armed_and_notifies_unassigned_without_rearming_cancelled`, `deferred_dispatch_generation_fences` and `unblock_fan_in_and_fan_out` prove armed release, cancellation, reassignment and no redispatch after a consumed assignment is reblocked/unblocked. The two `admitted_delivery_reblocked_before_settlement` cases prove one informational holder notice after Posted/reblock/restart, with and without prior tracking, through receipt settlement and another restart. W3 adapter evidence remains #562; W5 task-page removal/browser evidence remains #564. |
| TS558-5 | Self-edges, cross-project edges and cycles (including resolved edges) are refused in the transaction that writes; bounds and deduplication apply to batch creation and edits. | W1/W3: concurrent opposite-edge attempts, multi-edge atomic refusal, duplicate and size-limit tests. |
| TS558-6 | Recovery creates at most one resource set and delivery per assignment, preserving the admitted closed-without-Done list and assessment/escalation instruction; cancellation and late Complete cannot resurrect a retired generation. | W2 verified: `deferred_dispatch_restart_boundaries` covers Reserved/Materialized/Agent/Frozen/Posted/Settled with one workspace, message and receipt; frozen retry tests preserve original context after edits/restart. `closed_blockers_delivery_context` and `closed_blockers_coordinator_notice_context` verify the retained closure list and assessment/escalation instruction. `delivery_receipt_recovery_keeps_reblocked_task_column_and_timeline` proves consistent settlement. `tracker_completion_fences` covers retired owners, consumed pending preservation, reset reconstruction/captured turns and stale Complete. `completion_fence_two_queued_assignments_keep_unrelated_turn_fenced` pins competing B/C markers during an unrelated Working interval and a later freshly waiting assignment. `a_v16_database_migrates_to_v17_without_arming_existing_assignments` uses frozen schema-16 DDL, preserves task record bytes and dependencies through upgrade/reopen, checks defaults/indexes/foreign keys and empty intent/notice tables, and grants no authority from existing assignments. W3 wire/retry contract evidence remains #562. |
| TS558-7 | Dependency UI renders escaped cached facts and capability-gates writes; an older bridge never receives a waiting form as an immediate assignment. | W4/W5: hostile title rendering, cache-first/push grouping including Backlog Blocked/Ready without label or column writes, resolved-by-close presentation, current-greeting refusal and older-bridge browser fixtures. |

W2 verification (#561 snapshot 2, 2026-10-12): the named regressions and full bridge test
gate exit 0 (4,592 library tests passed, 9 ignored; integration tests include
`api_contract` and `complexity_ratchet`). Clippy with warnings denied, formatting,
locked vendored-library tests, diff checking, Semgrep over changed tracked files
and Gitleaks over `main..HEAD` also exit 0. Exact commands, exits, behavioral
red/green evidence and gate logs are attached to #561. No complexity annotation
or ratchet count was added. The W2-owned portions of TS558-2/3/4/6 are verified;
the extension remains pending its W3/W5 contract and browser evidence.
Snapshot-1 review findings have test-first red/green evidence for competing completion
markers, provisioning exclusion, admitted reblock notices and idle polling, plus the
dedicated schema-16 migration fixture. The initial full gate caught
foreground/background accounting being released twice; the corrected background
settlement path passes both dispatch forms and the full rerun. Failed/setup attempts
remain attached alongside the passing evidence. Migration assertions were split by
concern after Clippy caught the initial helper over the cap; no exemption was added.
Provider handoff uses QA fixtures; no live bridge, client pairing or browser was driven.
The waiting-on capability remains absent until #562.

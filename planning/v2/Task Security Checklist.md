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


## #522 extension — verified by C10 / #538

These controls follow [Project workspace](Project%20Workspace%20Spec.md).
The checked rows have combined C10 / #538 evidence at commit
`6b6a236e2b4e51ba4f6d0b8ff3c924100912a687`. The earlier scoped checkpoints below
record the evidence available at those checkpoints; the final C10 entry closes
their pending combined acceptance evidence.

| ID | Control | Verified | Passing evidence |
| --- | --- | --- | --- |
| TS522-1 | Existing/new project worker assignment and MCP tools are same-project scoped, idempotent and purpose-authorized; clients cannot mint or demote the pin. | [x] | `assign_project_worker_is_idempotent`, `assigning_refuses_a_foreign_or_removed_project_worker`, `a_worker_cannot_administer_sources_or_retire_the_pin_through_mcp`; `offers workers only in their exact project's separate group`, `keeps a creation operation ID across a failed dispatch retry and preserves the worker name`; `accepts the combined original-source / worker / console / child journey (390px)` and `accepts the combined original-source / worker / console / child journey (1280px)`. Full bridge/SPA gates exit 0 at `6b6a236e2b4e51ba4f6d0b8ff3c924100912a687`. |
| TS522-2 | Task links preserve exact worker identity without a fake workspace; worker removal and project deletion retire pending delivery/tracking consistently without rewriting history. | [x] | `project_workspace_combined_journey_and_source_matrix`, `removing_a_worker_keeps_its_tasks_record_and_drops_its_queued_turn`, `a_stale_worker_session_cannot_assign`, `project_deletion_retires_workers_and_preserves_sources`, `deferred_delivery_project_deletion_cancels_frozen_work_and_notices`; `resolves exact workers without a workspace and rejects foreign IDs in a complete roster`, `retains an unavailable worker's durable name but removes its link`. Full bridge/SPA gates exit 0 at `6b6a236e2b4e51ba4f6d0b8ff3c924100912a687`. |

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
### C8 / #536 scoped verification — 2026-10-13

Fix-round checkpoint: findings 1–6 and copy have focused passing evidence.
The full SPA gate pass and final scans remain pending at the mandatory
context-limit handoff; this entry makes no final verification claim.

Final fix-round revalidation remains pending.

TS522-1's SPA contribution scores **25/25**: cached assignee groups include only
the exact project's workers; creation retains its operation identity on retry;
a removed selection refuses instead of selecting another agent. The shared
device RPC boundary waits for the current greeting and refuses
`new_project_agent` creation when `tasks.assignProjectAgent` is absent, including
a retained picker caller after downgrade. Ordinary existing-agent assignment
keeps its existing contract. Inherited C4/C5 tests above supply bridge purpose,
same-project authorization and server idempotence evidence.

Focused device-context, worker-assignee and review-feedback suites ran under
`nice -n 10` and passed 51 tests (exit 0); both new current-greeting regressions
failed first (exit 1). Scoped source lint passed (exit 0). Rail/shell coverage
passed 385 tests (exit 0), and the project-worker Chromium suite passed four
tests (exit 0). Final broad gates and scans are recorded on #536 when complete.

Fix-round identity evidence supplements truthful task presentation: a removed
unnamed worker keeps its ordinal and harness instead of becoming the pin; an
older captured identity without a role remains generic unless exact cached
membership establishes the pin. The identity regression failed first, then
37 task identity/notice tests passed; the legacy fallback brought the focused
set to 38 passing tests. The additive 3.21.0 `project_role` API contract
regression failed first, then all 46 API contract tests passed; client/contract
coverage passed 71 tests. All green focused runs exited 0 under `nice -n 10`.
The legacy project-purpose metadata regression failed first (exit 101), then
11 bridge identity tests passed (exit 0). Durable live/stored project identity
backfill clears synthetic workspace ids/names even when an older snapshot
retained them. Finding 5's same-pin mounted-continuity regressions failed first
at 390/1280 widths (exit 1), then all seven worker Chromium tests passed
(exit 0); nine focused rail/shell/browser suites passed 452 tests (exit 0)
and scoped lint passed (exit 0). Finding 6's notice/task-line regressions
failed first (seven semantic failures, exit 1), as did the cached rail notice
regression. Three focused suites then passed 403 tests (exit 0), and scoped
lint passed (exit 0). Shared purpose-aware identity keeps a named Codex
worker's own harness and cached name in inline task lines and rail notices;
project-worker fixture cases also cover removed durable workers, cached bare
actors/assignees and the coordinator. Copy/cache notice regressions failed
first (exit 1), then the focused project/pin suites passed 102 tests (exit 0)
and scoped lint passed (exit 0). Final broad gates remain pending; earlier
evidence does not certify the final revision.

TS522-1 plus the three Chat Isolation C8 controls (25 points each) score
**100/100 within C8's implemented scope**. The combined #522 extension and
TS522-2's C10 lifecycle evidence remain pending; this entry does not mark them
complete.

### C10 / #538 combined verification — 2026-10-13

All 2 controls above are verified at
`6b6a236e2b4e51ba4f6d0b8ff3c924100912a687`. Full bridge `cargo test` exited 0,
including `project_workspace_combined_journey_and_source_matrix`, coordinator,
persisted-purpose, exact-lineage, stale-generation and lifecycle regressions.
The run used the real uv 0.12.15 executable through `BST_UV` and `PATH`, with
`$HOME/.cargo/bin` first. Full SPA `npm test` under Node 22.23.3 / npm 12.0.2
and `CHROMIUM_PATH=/usr/bin/chromium` exited 0: 669 files / 10,473 tests passed,
including all seven `browser/projectWorkspaceFlow.test.js` cases. The two full
390/1280 journeys retain exact worker/task addresses, sibling and source drafts,
and the canonical pin across source, console and child-workspace navigation.

Semgrep `--config auto --error` on every changed file, Gitleaks with
`--log-opts='origin/main..HEAD'`, and `git diff --check origin/main..HEAD` each
exited 0. All gates ran under `nice -n 10` with output redirected to logs.
The four-pair compatibility matrix uses frozen legacy requests, the production
client adapter and isolated old/new protocol fixtures; it does not claim
historical executable runs. #538 records the gate exits and final publication.

## #558 extension — bridge verified; browser evidence pending

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
bound. W2 delivery authorization and W3 caller-facing evidence are verified below.

| ID | Control | Evidence and pending UI scope |
| --- | --- | --- |
| TS558-1 | Dependency create/add/remove and reverse reads stay within the caller's project; mixed valid/foreign writes fail atomically without exposing foreign titles or identities. | W1/W3: `waiting_on_same_project_dag_is_atomic`, RPC/MCP cross-project create, add, remove, read and partial-write refusal tests. |
| TS558-2 | Deferred delivery revalidates the saved target's project and current authority before provisioning and after off-lock work; missing/retired targets never fall back to another agent. | W2 verified: `waiting_assignment_defers_every_target`, `deferred_delivery_revalidates_foreign_target_before_admission`, `deferred_delivery_removed_target_never_substitutes_another_agent`, `deferred_delivery_retired_sender_preserves_authority_failure`, `deferred_workspace_creation_retry_refuses_retired_saved_owner` and `deferred_receipt_recovery_fences_missing_retired_and_foreign_targets` cover every target, off-lock recovery, retirement and foreign bindings. `deferred_provisioning_excludes_workspace_lock_and_rename_until_settlement` proves background provisioning holds filesystem mutation exclusion through settlement, refuses reclaim while waiting on source SyncLock, and accepted edits persist afterwards. `deferred_workspace_pins_saved_git_inputs_while_source_is_held` proves current and changed-base reservations wait on the saved repository lock, pin its commit, keep the app mutex available and preserve queued worker source-location holds. W3 caller-facing scope/compatibility is verified in #562 by the scoped RPC/MCP and exact-intent retry tests below. |
| TS558-3 | Only a committed same-project readiness change (Done, close or edge removal) with a current authorized assignment, or an explicit ready assignment, can enqueue delivery; closed edges retain resolved-by-close provenance. Caller-authored notices, pushes or forged close/unblock/generation/operation fields grant no authority. | W1 provenance evidence above is retained. W2 verified: `durable_delivery_generation_and_live_graph_fence_saved_work`, `delivery_admission_fences_graph_writes_and_failed_retry`, `delivery_admission_binds_frozen_payload_address_and_origin` and `deferred_delivery_operation_id_cannot_admit_a_forged_payload_or_foreign_owner` refuse stale graph/generation and forged operation, address, payload and origin facts. `deferred_delivery_startup_runs_without_rpc_or_connected_client` proves startup handoff without a client. `pending_provisioning_does_not_self_wake_idle_reconciliation` holds provisioning in flight, bounds empty reconciliation polls to zero immediate repolls, rejects unrelated-write wakeups and retains new-assignment wakeups. W3 RPC/MCP contract evidence is verified in #562, including forged fields and canonical typed refusals. |
| TS558-4 | Removing the last edge can release only a still-current explicit assignment; unassigned, cancelled, superseded and consumed assignments cannot start work. UI states the consequence of removing an armed final blocker. | W2 verified: `removing_last_edge_delivers_armed_and_notifies_unassigned_without_rearming_cancelled`, `deferred_dispatch_generation_fences` and `unblock_fan_in_and_fan_out` prove armed release, cancellation, reassignment and no redispatch after a consumed assignment is reblocked/unblocked. The two `admitted_delivery_reblocked_before_settlement` cases prove one informational holder notice after Posted/reblock/restart, with and without prior tracking, through receipt settlement and another restart. W3 adapter release/no-redispatch evidence is verified in #562; W5 / #564 verifies the cached armed/unassigned removal consequence in `taskWaitingOnModel.test.js`, `taskWaitingOnPage.test.js` and keyboard browser cases in `taskWaitingOnEdges.test.js`; removal sends only the edge command and does not invent an assignment or worker. |
| TS558-5 | Self-edges, cross-project edges and cycles (including resolved edges) are refused in the transaction that writes; bounds and deduplication apply to batch creation and edits. | W1/W3: concurrent opposite-edge attempts, multi-edge atomic refusal, duplicate and size-limit tests. |
| TS558-6 | Recovery creates at most one resource set and delivery per assignment, preserving the admitted closed-without-Done list and assessment/escalation instruction; cancellation and late Complete cannot resurrect a retired generation. | W2 verified: `deferred_dispatch_restart_boundaries` covers Reserved/Materialized/Agent/Frozen/Posted/Settled with one workspace, message and receipt; frozen retry tests preserve original context after edits/restart. `closed_blockers_delivery_context` and `closed_blockers_coordinator_notice_context` verify the retained closure list and assessment/escalation instruction. `delivery_receipt_recovery_keeps_reblocked_task_column_and_timeline` proves consistent settlement. `tracker_completion_fences` covers retired owners, consumed pending preservation, reset reconstruction/captured turns and stale Complete. `completion_fence_two_queued_assignments_keep_unrelated_turn_fenced` pins competing B/C markers during an unrelated Working interval and a later freshly waiting assignment. `a_v16_database_migrates_to_v17_without_arming_existing_assignments` uses frozen schema-16 DDL, preserves task record bytes and dependencies through upgrade/reopen, checks defaults/indexes/foreign keys and empty intent/notice tables, and grants no authority from existing assignments. W3 wire/retry contract evidence is verified in #562 by exact saved-intent retries and pinned delivery/notice fixtures. |
| TS558-7 | Dependency UI renders escaped cached facts and capability-gates writes; a downgraded device cannot receive ordinary new-workspace assignment. | W5 / #564 round 11: `taskWaitingOnSharing.test.js` verifies shared evidence under refused/unavailable storage, no echoes, device isolation and preserved rendering support; the Chromium sharing suite verifies native-channel evidence and refusal through the other tab’s real picker. `taskWaitingOnSupport.test.js` verifies a later capable greeting retries a refused first put and persists across reload. `taskWaitingAssignmentHistory.test.js` exercises real greeting authority, device RPC and the picker for superseded capable hellos, a refused IndexedDB history put and refused reads, plus capable/confirmed-legacy and retired-account controls. `taskWaitingOnSupport.test.js` verifies monotonic evidence before persistence, failed writes, unknown/unavailable/malformed reads, committed retries/no-ops, account reset, existing cached support and reload. `taskWaitingOnActions.test.js` admits capable/never-upgraded assignment, refuses previously capable devices and unknown history with retry/refresh copy, preserves greeting authority, and gates graph verbs/parameters. `taskWaitingAssignmentDowngrade.test.js` contains the six real-picker legacy-erasure reproductions (detail/catalogue/query × absent/first-ready knowledge); `trackerTasksPaneDom.test.js` and `taskWaitingOnEdges.test.js` verify inline board/list/task-page refusal with retained drafts, including ready tasks. Per-task arbitration and dependency-knowledge records are removed. `trackerAssignmentDetailRefresh.test.js` verifies post-assignment trailing detail refresh restores concurrent unread mentions while rejecting the pre-response read. `taskWaitingOnCache.test.js`, `taskWaitingOnPush.test.js`, `taskWaitingOnPage.test.js` and `trackerTaskChecklistDom.test.js` retain cache/reverse-page fences and edge-mutation timeline refresh. Browser fixtures retain escaped titles, same-project keyboard editing, Done/close presentation, navigation, exact saved-id retries and legacy shapes at 390/1280 light/dark. W4 / #563 cache/group evidence and the combined journey remain applicable. |

TS558-7 accepted residual risk: capability history is keyed by the paired device
UUID. Re-pairing a machine mints a fresh UUID, so the previous device's history
does not follow it and a downgraded bridge can appear never upgraded. Downgrading
the bridge and then re-pairing it are two deliberate actions on an unsupported
downgrade path; the old bridge cannot enforce waiting-on prerequisites whatever
the client does. This limitation is accepted, with no identity linking across
pairings.

TS558-7 accepted residual risk: evidence that could never be written to storage
lasts only for that page's lifetime.
Failed writes cannot erase monotonic evidence in the observing page, and open
tabs listening on the existing `build-cache` BroadcastChannel retain shared
evidence for their own lifetimes. Each authenticated capable greeting retries
persistence. New or uninformed tabs cannot recover unpersisted evidence, nor can
a reload after all informed page lifetimes end. This residual requires refused
storage, a reload or uninformed tab, and a bridge downgrade: the client cannot
retain across reload what the browser will not persist. The limitation is
accepted with these retry and open-tab mitigations, with no new channel or
durable fallback.

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
The waiting-on capability stayed absent until #562 verified the corrected complete graph/dispatch/read contract.


W3 / #562 bridge integration evidence (2026-10-12): **100/100 for the
bridge-owned portions of TS558-1–6 (all six applicable controls verified).**
This W3 score covers bridge controls. W5 UI evidence for TS558-4/7 is recorded
below; W4 grouping and the combined journey belong to #563/the second landing.

- TS558-1: `waiting_on_rpc_mcp_contract_refusals_are_atomic_and_scoped`,
  `waiting_on_rpc_mcp_contract_foreign_endpoints_and_cursors_are_fenced` and
  `waiting_on_rpc_mcp_contract_create_refusals_name_proposed_blockers` cover
  create/add/remove/reverse reads, mixed-write rollback, cursor scope and
  same-project-only refusal facts. Malformed MCP cursors refuse explicitly.
- TS558-2: `waiting_on_rpc_mcp_contract_retry_preserves_saved_intent_and_refuses_replacement`
  preserves the failed intent's exact target and authority; null or replacement
  target/note fields and stale operations refuse. Public assignment parameter
  types remain compatible, with strict presence-aware wire parsing.
- TS558-3: the scoped refusal suite rejects caller-authored actor, readiness,
  unblock, generation and operation facts. The canonical-refusal regression
  covers all three new verbs' sorted unknown fields and `details.params`.
  `unblock_preserves_user_attention` and
  `waiting_on_invalidation_fanout_contract` verify persisted attention and
  endpoint/dependent invalidations with bounded fan-out.
- TS558-4: `waiting_notices_and_reminders` and its undelivered-state regression
  exclude blocked/queued/failed work from Complete reminders and automatic
  moves; partial resolution stays quiet, final release sends one brief,
  unassigned releases notify the coordinator, and reblocking is informational.
  `waiting_review_finished_unresolved_task_refuses_user_assignment` prevents
  an unresolved finished task from manufacturing actionable user work.
- TS558-5: `waiting_on_same_project_dag_is_atomic` and
  `waiting_on_concurrent_opposite_edges_cannot_cycle` retain transactional
  self/foreign/cycle refusal, resolved-edge cycle checks, deduplication and
  bounds. The RPC/MCP mixed-edge regression verifies the adapter cannot bypass
  those store controls or leave partial tasks/links.
- TS558-6: the exact-intent retry regression, inherited deferred-generation and
  completion fences, and `closed_blocker_notice_contract` retain at-most-once
  dispatch and distinguish every retained close-without-Done prerequisite from
  completed work. Briefs ask for assessment and explicit task escalation when
  unclear, without manufacturing a user mention. Contract fixtures pin
  waiting/queued/failed/delivered, resolution provenance and thread-page notices.

The corrected complete library gate exited 0: 4,648 passed, 9 ignored.
`waiting_reads_bound_reverse_preview_and_share_summaries` now compares complete
get/list/watched dependency summaries, preserving 21 reverse dependents, a
20-item preview and its continuation cursor. Existing list/detail parity and
typed-handler guards pass. The new canonical-error regression first failed
behaviorally (exit 101) and then passed; initial setup compile errors are not
counted as behavioral evidence. Fresh Astra review and final reassessment found
no remaining issues after the canonical fix, reachable notice-fixture correction
and test-helper extractions. `api_contract` and
`complexity_ratchet` remain unchanged from inherited e82c24ea.

Repository release evidence is bridge 0.2.13 **Unreleased**, wire/API/fixtures
3.21.0 and latest bridge tag bridge-v0.2.12. New verbs retain `since: 3.21.0`;
`tasks.link` remains retired. `tasks.waitingOn` was enabled only after the
corrected complete graph/dispatch/read library gate passed.

The unchanged API contract suite also exited 0 (45 tests), including reachable
newest-activity notice pages and typed result round trips. Clippy with warnings
denied and formatting exited 0 after splitting two inherited test functions
into assertion helpers; no complexity exemption or ratchet count changed.


W3 final gates (2026-10-12), all under `nice -n 10`, judged by their actual
exit codes: full `cargo test` **0** (4,648 library tests passed, 9 ignored;
`api_contract` 45 passed, `complexity_ratchet` 2 passed, all remaining
integrations and documentation tests passed); Clippy `--all-targets -- -D
warnings` **0**; `cargo fmt --check` **0**; locked vendored-library test **0**
(169 passed, 4 ignored); Semgrep auto/error **0** (203 rules, 29 changed tracked
Rust/JS targets, no findings); Gitleaks `--log-opts='origin/main..HEAD'` **0**;
`git diff --check origin/main..HEAD` **0**. Transport's locked install and SPA's
locked install, lint, full tests and build each exited **0** using pinned Node
22.23.3/npm 12.0.2. Full SPA: 652 files, 10,229 tests passed, including the
shared cached unread regression and isolated Chromium tests.

Rust interop used the existing uv 0.12.15 executable directly on the gate's
PATH after the unset mise shim caused one failed attempt; no global tool
configuration changed. Earlier broad gates exposed the stale reverse-preview
assertion, unreachable notice fixture and two test functions over the complexity
cap. Their corrections passed the unchanged contract and ratchet tests and the
complete rerun. A SPA attempt that encountered the initial regression-test
compile error was stopped (exit 143), then the complete stable-source suite
passed. Failed/setup attempts are retained with the passing evidence on #562.
Origin/main remained b1e4643a through the final fetch, so no merge was needed.


W5 / #564 task-page evidence (2026-10-13): **100/100 for the W5-owned
portions of TS558-4/7 (both applicable controls verified).**

- TS558-4: model and task-page tests distinguish a saved waiting/queued assignment
  from an unassigned, cancelled or consumed one. The browser's armed/unassigned
  final-edge confirmations send only `tasks.waiting_on.remove`; they never issue
  an assignment, workspace creation or agent start. Assignment copy explains Done
  or close, delayed creation, closure assessment and user-held waits; failed
  delivery retries only its exact saved id.
- TS558-7: browser fixtures exercise 390/1280 light/dark keyboard add/remove and
  both navigation directions, paged reverse reads, escaped hostile titles,
  same-project choices, distinct Done/close resolution and retained cycle/stale
  drafts. Reads held in flight leave cached controls usable. Legacy records send
  the exact old assignment shape; cached waiting records under an older greeting
  retain the prerequisite explanation and refuse sends. A ready form reblocked
  in the cache before submit also refuses the downgraded greeting without losing
  its note or target.
- Reverse-cache regressions remove/add edges at endpoints absent from previews,
  invalidate an unmounted production task push, fence stale first/continuation
  replies (including previously unseen endpoints), discard mounted stale tails
  and restore the authoritative preview cursor. Other projects remain usable.

Test-first evidence on #564: cached reblock and legacy fixture runs exited 1
before correction, then assignment/browser coverage exited 0 (22 tests, including
15 Chromium cases). Cache mutation and production-push/mounted-page regressions
also exited 1 before correction, then all four focused cache/page/push/change
suites exited 0 (53 tests). Earlier assignment copy, model, action, capability and
cache evidence is retained in the checkpoint. No complexity exemption or ratchet
count changed. The four inspected phone/desktop screenshots still match the
markup; the takeover changed cache/dispatch guards and fixtures.

W4 Active/Backlog grouping is #563's scope. As directed for the parallel landing,
the second of #563/#564 to land verifies assigned wait → Done/close release →
one delivery with closure assessment → updated Active/Backlog groups. This W5
score does not claim that combined journey before W4 lands.


W5 final gates, all under `nice -n 10`, with SPA/transport using Node
22.23.3/npm 12.0.2 and judged by actual exit codes: locked transport install
with `--ignore-scripts` **0**; locked SPA install with `--legacy-peer-deps` **0**;
all-source lint **0**; full `npm test`, including Chromium, **0** (668 files,
10,380 tests passed); `test:captures` **0**; build **0**; unchanged
`complexityRatchet` **0** (2 tests); Semgrep auto/error **0** (200 rules, 30
changed tracked targets, no findings). Gitleaks and `origin/main..HEAD`
diff-check evidence is attached on #564 at the final published head.

The first complete suite exited 1 with exactly two strict source-inventory
assertions missing the new waiting renderer (10,378 tests passed). The select
and input manifests now include its production search/select surfaces; the
original assertions are unchanged. Both inventories failed first and then
passed; their complete style suites exited 0 (13 tests, both themes), followed
by the passing complete rerun. #614 did not fail, so no isolated flake reruns
were needed. Gate logs, red/green evidence and four representative screenshots
are attached on #564. Origin/main stayed e42778df through the ready fetch;
no merge was needed. No live bridge or user browser was used.


W5 revision 2 / #564 closes the two P2 review gaps in snapshot 1. The board/list
picker now forwards device and project scope (`facfb5bb`). Four regressions run
through the real picker, fake IDB and greeting authority in both board/list
views: capable blocked assignment succeeds; cached reblock plus greeting
downgrade refuses without losing the note or target. All four failed before the
fix and pass afterward; the related pane suites exit 0 (94 tests).

Accepted edge/assignment replies now advance an atomic task-detail mutation
fence (`b0ca5ccc`). Both the mounted task-page reader and secondary detail feed
capture its basis before `tasks.get` and reject stale responses during atomic
cache admission, alongside the existing checklist guard. Production reader
regressions failed first for graph edits and saved-assignment state, including a
same-timestamp feed response. Fresh reads begun after the action still admit.
Expanded DOM/churn/read-order/cache/ratchet coverage exits 0 (117 tests); source
lint and diff checks exit 0. No complexity cap, annotation or ratchet changed.
The W5-owned TS558-4/7 score remains 100/100 with these gaps closed; revision-2
broad gates and security scan logs are attached to the new #564 review head.

W4 / #563 cached-rendering evidence (2026-10-13): the rendering portion of
TS558-7 is verified by `taskBlockedSections.test.js` (six Chromium tests,
exit **0**). The 390px and 1280px fixtures mount from cached tasks while
reads are pending. Normal task pushes update Dashboard, watched inbox and
agent rail through Done, close, unblock and reopen; project and board Backlog
move between Blocked and Ready without any label or column write. Hostile
cached blocker titles remain escaped text, legacy capability fixtures retain
their existing groups, explicit questions retain attention, and Done/closed
history takes precedence over readiness. Blocking links preserve their own
pointer and keyboard navigation instead of also opening the dependent row.
That regression was observed failing before the inbox click guard (exit **1**)
and passing afterwards (isolated exit **0**, full six-test suite exit **0**).

The Dashboard-model pending creating-target name regressions likewise failed
before the fix (exit **1**, two tests), then passed with all 48 model tests
(exit **0**). The grouping, rail, inbox and board unit evidence is recorded
on #563 alongside the browser red/green logs and 390px/1280px light/dark
fixture screenshots. The real-wire watched-inbox fixture additionally verifies
the unblocked dependency digest and `Assigned · Assigned to you` group-plus-ask
facts (four tests, exit **0**); its stale pre-grouping expectation was first
observed failing (isolated exit **1**).
These renderers introduce no write authority or
connection-state branch. This evidence verifies the complete W4 cached-rendering
scope; TS558-7's W5 write-capability and edge-control scope remains owned by
#564 and is not claimed here.


W5 revision 3 / #564 (2026-10-13) closes snapshot 2's list-first P2 and
integrates W4 from origin/main `0ee0188f` (merge `54a0eb57`). The two new
board/list regressions hold task detail ready with `tasks.get` pending, reblock
only the newer list cache, and downgrade the current greeting. Both failed
before the fix (exit **1**, unwanted `tasks.assign`), then passed in the
109-test assignment/pane/unchanged-ratchet run (exit **0**). The shared guard
reads detail and catalogue/query/page replicas together, prefers newer task
facts and uses list read order for timestamp ties, falling back to cache write
time for detail/legacy records and write order for remaining ties; it retains
the captured wait and the note/target on refusal. Older blockers do not defeat newer ready
facts, and missing legacy fields do not erase known prerequisites. No ratchet
annotation or cap changed.

The second-landing browser/style gate now exits **0** (73 tests). It includes
all 15 task-page edge cases, all six W4 Blocked cases, input/select style suites,
and six `taskWaitingOnJourney.test.js` Done/close journeys reached from the
task page, list and board. A scripted fixture bridge supplies durable release
and delivery replies to production actions, cache observers and renderers:
assigned waits move from Active Blocked to Working; unassigned work moves
Backlog Blocked → Ready without label/column writes or automatic user mentions;
resolved edges retain Done/close provenance; reopening reblocks both surfaces;
a second release does not redispatch consumed/cancelled assignments. The
fixture records no early provisioning and exactly one delivery with retained
closure-assessment context. Actual durable at-most-once enforcement remains
covered by W2/W3's inherited bridge tests above, not by the scripted service.
The combined UI portion of TS558-4/7 is now verified, preserving the W5-owned
**100/100** score. Final merged-head gate and scan exit codes are attached on
#564. No bridge files changed from origin/main; no live bridge or user browser
was used.

W5 revision 4 / #564 closes the delayed-page freshness gap in
`taskAssignmentFacts.js`. Three regressions failed before the fix (exit **1**,
commit `5bdd675d`): an older page arriving after newer catalogue/query facts
must not clear a blocker with the same task timestamp, nor revive an older
blocker over newer ready facts. Commit `772f6e1b` reuses `listAskedAt` so list
`read_order` takes precedence over arrival time when task timestamps tie.
Newer task timestamps still win; detail and legacy lists retain the cache
write-time fallback. The assignment facts/picker/waiting/unchanged-ratchet
gate exits **0** (116 tests). No cap or ratchet entry changed. Fresh final
gate results and security scan evidence are recorded on #564.

Revision-4 fresh gates use Node 22.23.3/npm 12.0.2 under `nice -n 10`:
transport ci (`--ignore-scripts`), SPA ci (`--legacy-peer-deps`, scripts disabled),
all-source lint, captures and build each exit **0**. The full Chromium-inclusive
suite exits **1**: 670 files / 10,441 tests pass; its sole failure is known #614,
`workspaceDirectories.test.js`'s kept-Changes-directory rebind timeout. The
exact failing case was rerun alone three times: exits **0 / 0 / 0**, each one
test passed and 12 skipped. The complete suite is recorded as failed, with
that isolated flake evidence attached on #564. Semgrep exits **0** (38 changed
tracked targets, 200 rules, no findings); Gitleaks `origin/main..HEAD` and
diff-check exit **0**. The W5-owned TS558-4/7 score remains **100/100**.
Fetched origin/main remains `0ee0188f`, already merged; no bridge files,
complexity caps or ratchet entries changed.

W5 revision 5 / #564 moves assignment admission to the send boundary after
round-3's pending-greeting finding. The real board/list regressions in
`trackerTasksPaneDom.test.js` both fail first (exit **1**, commit `95acb82a`):
a ready form submits while re-greeting waits, only the list cache reblocks,
then an older greeting settles. Action-level fresh-fact/captured-authority
regressions also fail first (exit **1**, commit `ea0da86e`).

Commit `2a753441` removes the picker's captured/submit-time readiness decision.
After greeting settlement, `taskWaitingOnActions.js` rereads the freshest
scoped `taskAssignmentFacts`, rechecks the same greeting authority after that
asynchronous cache read, then validates capability and sends without another
wait. The opened task is a fallback only if no cache replica supplies known
prerequisite facts; no cached-capability downgrade invents immediate work.
All prior assignment/cache freshness regressions, the new pending-greeting
cases, and unchanged ratchet pass (six files / 127 tests, exit **0**); lint
exits **0**. Final fresh broad gates and publication evidence follow on #564.

Revision-5 fresh final gates, pinned Node 22.23.3/npm 12.0.2 under
`nice -n 10`: transport ci (`--ignore-scripts`), SPA ci
(`--legacy-peer-deps`, install scripts disabled), all-source lint, captures,
build and full Chromium-inclusive `npm test` all exit **0**. All **671 files /
10,446 tests passed**; #614 did not fail, so isolated reruns were unnecessary.
Semgrep exits **0** (38 changed tracked js/mjs/css targets, 200 rules, no
findings). Gitleaks `origin/main..HEAD` and diff-check exit **0**, with exact-head
publication evidence attached on #564. The W5-owned TS558-4/7 score remains
**100/100** after the send-time race is closed. No bridge files, complexity
caps or ratchet entries changed. Source stayed fixed throughout these gates;
the final follow-up only records this evidence.

W5 revision 6 / #564 supersedes revision 3’s timestamp-tie read/write
ranking after round-4’s unrelated-mutation reproduction. Test-first commit
`ef9c0139` reproduces both list and board forbidden assignments through the
real picker and production cache writers: an accepted edit of unrelated B
raises the catalogue read order while retaining ready A, outranking blocked
A in a filtered query at the same task timestamp. Both fail first (exit **1**).
Unit-rule commit `97c24a2b` fails first too (exit **1**, four assertions).

Fix `84540e28` removes source read/write ranking from `taskAssignmentFacts.js`.
The task’s own timestamp selects the newest stamped facts; unstamped facts
remain ambiguous. Among those candidates, waiting wins disagreement.
Strictly newer ready versions still supersede older blockers. Unit coverage
checks tied writes, delayed tied pages, unknown/invalid timestamps in both
readiness directions, mixed stamped/unstamped sources and device/project/task
isolation. Two former assertions of ready-wins read ordering now assert the
requested waiting-wins tie policy. A ready task can temporarily be refused
on an older bridge until replicas converge; ambiguous waiting evidence cannot
authorize immediate assignment on that bridge.

The send-time structure from `2a753441` is unchanged: settle the greeting,
reread scoped facts, recheck the same greeting authority, validate capability
and send without another wait. The earlier scope, detail/list-first reblock,
delayed-page and pending-greeting regression coverage remains green. Focused
validation exits **0** (seven files / **146 tests**, including the unchanged
complexity ratchet). TS558-4’s edge-removal consequence evidence is unchanged;
TS558-7’s cache conflict and old-bridge refusal evidence now includes the
round-4 reproductions. Final broad-gate and publication evidence follows.

Revision-6 final gates, Node 22.23.3/npm 12.0.2 under `nice -n 10`:
transport ci (`--ignore-scripts`), SPA ci (`--legacy-peer-deps`, install
scripts disabled), all-source lint, full Chromium-inclusive `npm test`,
`test:captures` and build each exit **0**. All **671 files / 10,451 tests**
pass; #614 does not fail, so isolated reruns are unnecessary. Semgrep exits
**0** (38 tracked js/mjs/css targets, 200 rules, no findings); Gitleaks
`origin/main..HEAD` and diff-check exit **0**. The W5-owned TS558-4/7 score
remains **100/100** with the round-4 cache-conflict reproduction closed.
No bridge files, complexity caps or ratchet entries change. Source is fixed
through the gates; this final follow-up records evidence only. Push, saved
snapshot, head/base and attached exit/log evidence are recorded on #564.

W5 revision 7 / #564 closes round-5’s omitted-dependency refresh finding.
`3244f02c` adds real list/board picker regressions using production cache
writers: ready t1 remains in a filtered query; a blocked t2 task opens the
picker; a downgraded legacy refresh omits its dependency summary from the
current replicas. Both fail first (exit **1**, forbidden `tasks.assign`).
`ceeccd38` adds unit coverage for the opened source and unknown dependency
data, red exit **1** (three assertions).

Fix `6957095f` feeds the opened task into the same scoped timestamp
arbitration as every cached source and removes its fallback-only admission.
Omitted/null summaries and objects with no boolean readiness flag are unknown,
never ready. Unknown plus any known blocker keeps waiting. Known sources
retain task-timestamp arbitration and the conservative tie/unknown-timestamp
rule; source read/write order is never compared. The send-time greeting
authority design from `2a753441` remains intact. Seven focused regression
files pass (**154 tests**, exit **0**, including unchanged complexity ratchet).

Explicit empty waiting lists remain authoritative: the current wire represents
no blockers as `dependencies: { blocked: false, waiting_on: [] }`, not `{}`.
Commit `c1273844` adds three regressions showing this newer summary wins over
older catalogue/query/page blockers and an older opened blocker. They pass
with the production fix unchanged (**24 unit tests**, exit **0**). Thus the
“empty dependency data” wording referred to an object lacking readiness data,
never to the wire’s empty prerequisite list. TS558-4’s removal-consequence
evidence is unchanged; TS558-7 includes the opened-source, unknown-summary
and authoritative-empty-list cases. Final broad gates and publication follow.

Revision-7 final gates, pinned Node 22.23.3/npm 12.0.2 under `nice -n 10`:
transport ci (`--ignore-scripts`), SPA ci (`--legacy-peer-deps`, install
scripts disabled), all-source lint, full Chromium-inclusive `npm test`,
`test:captures` and build each exit **0**. All **671 files / 10,462 tests**
pass, including the explicit empty-list regressions; #614 does not fail, so
isolated reruns are unnecessary. Semgrep exits **0** (38 tracked js/mjs/css
targets / 200 rules / no findings); Gitleaks `origin/main..HEAD` and diff-check
exit **0**. W5-owned TS558-4/7 score remains **100/100** after closing the
opened-source and unknown-summary bypass. No bridge files, complexity caps
or ratchet entries change. Source stays fixed at `6957095f` throughout the
final gates; later commits add explicit empty-list tests and record evidence.
Publication head/base, snapshot and attached exit/log evidence are on #564.


W5 round-7 fix / #564 (2026-10-13), addressing the round-6 review:

- Dependency knowledge now has its own per-task `tracker-task-dependencies`
  cache record scoped by device/project/task. Only admitted writes carrying a
  boolean readiness summary can update it. The summary keeps the source task's
  timestamp. Legacy omitted/null/unknown summaries leave it untouched even if
  their ordinary task timestamp is newer. A blocked record persists until a
  strictly newer capable version reports ready; the current-wire
  `{blocked:false, waiting_on:[]}` clear remains authoritative. Tied or unknown
  timestamps retain waiting. Replica timestamps/read orders no longer decide
  the guard once knowledge exists. The previous conservative fallback remains
  for caches without a knowledge record, including one atomic snapshot of
  knowledge with the fallback replicas after an initial knowledge miss.
- The reviewer's real list/board reproduction opens ready t1, learns blocked
  t2 in detail/catalogue, verifies the reblock was known, then overwrites every
  blocking replica with legacy t2. Both tests failed on forbidden
  `tasks.assign` before the fix (exit **1**), then pass while retaining note and
  target. Twenty-five knowledge tests cover every production writer path,
  newer legacy writes, older/newer capable empty lists, timestamp ties, rejected
  writes, concurrent writers, reload, scoped eviction and the initial-miss race.
  The additional initial-miss test failed first (exit **1**) and passes after
  the knowledge/fallback snapshot change.
- Successful add/remove requests a detail refresh through the task page's
  existing trailing reader after the action cache write. The page/button/action
  reproduction failed first (exit **1**, only two detail reads). It now proves
  the push-triggered read is rejected by the unchanged detail fence, then the
  third read restores the cached and rendered link/readiness timeline. Controls
  retain refused drafts and removal still issues only the edge command.

Focused validation exits **0**: eight files / **207 tests** for the primary
record/cache/assignment/paging fix (including unchanged complexity ratchet);
three files / **40 tests** for expanded writer coverage, push invalidation and
page/action race; three files / **51 tests** for the final initial-miss change,
including all fallback fact regressions and unchanged ratchet. Scoped lint
exits **0**. ARCHITECTURE's Render from cache and capability sections describe
both fixes. No bridge files, complexity caps or ratchet entries change. Final
broad gates and publication evidence follow below and on #564.


W5 round-8 fix / #564 (2026-10-13), addressing round-7 review finding 1:

- Tests `3627dc60d` port the four attached real-picker reproductions and all
  eight detail/catalogue/query/page × list/board variants, plus an opened-only
  blocker. All thirteen fail first on forbidden `tasks.assign` sends (exit
  **1**; 110 existing picker/pane tests pass). Fourteen pure union cases fail
  against the record shortcut (exit **1**, nine failures); three partial-summary
  persistence cases expose two unsafe clears (exit **1**). Older readiness
  fixtures now use the explicit current-wire `{blocked:false, waiting_on:[]}`;
  partial summaries retain separate unknown-data assertions.
- Fix `e2c11afbe` removes both exclusive-record returns. The reader snapshots
  record and all scoped detail/catalogue/query/page replicas together and feeds
  them, the initially observed record and the opened task into pure
  `selectTaskAssignmentFacts`. Highest capable task version wins; blocked wins
  ties, and unversioned waiting remains conservative. A newer explicit ready
  fact clears an older blocker even across legacy refreshes. Legacy/partial
  facts never prove ready, and the absence of capable facts retains today's
  conservative rule. Knowledge writers share the same capability predicate,
  so partial false summaries cannot erase durable waiting.
- Focused units exit **0**: **238 tests / 14 files**, including every earlier
  cache, action, page, assignment, refusal and unchanged complexity-ratchet
  regression. All-source lint exits **0**. ARCHITECTURE now describes the union
  rather than treating the record as exclusive authority. The earlier timeline
  refresh and greeting-authority fences remain covered. No bridge files,
  complexity caps or ratchet entries change. Broad gate evidence follows.


Round-8 final gates, pinned Node 22.23.3/npm 12.0.2, all under `nice -n 10`:
transport ci (`--ignore-scripts`), SPA ci (`--legacy-peer-deps`, scripts
disabled), lint, focused units/browser (**17 files / 265 tests**), full
Chromium-inclusive `npm test` (**673 files / 10,528 tests**), all three
`test:captures` checks and build each exit **0**. The reviewer's 36 writer-pair
cases and lifecycle case also pass (**37 tests**, exit **0**). No #614 reruns
are needed. Semgrep on all **44 changed paths**, Gitleaks
`origin/main..HEAD`, and diff-check exit **0**, with no findings or leaks.
W5-owned TS558-4/7 is **100/100** with the round-7 first-record bypass closed.
The full suite validated head `48cd950fd` against merged main `01296e8e`;
source remained fixed at `e2c11afbe`. This final commit changes evidence only.
Final scans, pushed head and saved snapshot are recorded on #564. No bridge
files, complexity caps or ratchet entries change; no independent Astra review
was run in this fix round.

W5 round 9 / #564 (2026-10-13) supersedes the per-task assignment arbitration
and dependency-knowledge policies described in earlier evidence. Those policies
could lose a pre-existing capable replica to a legacy write before observation.
The device's persisted `everWaitingOn` flag now records its highest support seen;
a later incapable greeting cannot clear it. Current capable bridges admit
assignment normally. Never-upgraded devices retain legacy assignment. A device
that advertised support before and now lacks it refuses ordinary new-workspace
assignment from board/list and task-page pickers, including ready tasks, with
an inline downgrade/update message and the draft intact. Display still reads
cached task replicas; no per-task history is needed for dispatch.

Test-first commit `17a6ad4ee` records all six round-8 real-picker failures
(detail/catalogue/query × absent/first-ready knowledge), capability persistence
and dispatch tests. Red picker exit **1**, six forbidden `tasks.assign` sends;
capability history exit **1**, seven failures; dispatch exit **1**, three
failures. The device-rule focused run passes all **21 tests**, exit **0**.
Finding 2 separately fails first in the real board/list pickers (two failures,
exit **1**): a partial assignment response fences a held mention read and
leaves the old timeline at the new task version. Post-response detail refresh
now uses a bounded trailing reader while retaining the mutation fence; the
replacement read restores the unread mention and Needs you rendering even
when cache writes share a local timestamp.

Deleted arbitration-only suites: `taskAssignmentFacts.test.js`,
`taskAssignmentFactUnion.test.js`, `taskDependencyKnowledge.test.js`; removed
picker migration/record-ranking cases in `taskWaitingAssignment.test.js` and
`trackerTasksPaneDom.test.js`. The six writer-loss reproductions and ordinary
picker refusal/draft tests remain under the device rule. No complexity caps
or ratchet entries change. Final gate and publication evidence is on #564.

W5 round 10 / #564 (2026-10-13) closes the late authenticated hello and storage
refusal gaps from round 9. The real greeting-authority/device-RPC/picker
reproductions failed first on forbidden assignment sends (late capable hello,
one refused history put, refused history read); the superseded-history and
unavailable/malformed-history units failed too: **6 failures, exit 1**.
Historical capability updates synchronously in memory, survives refused puts,
and is merged monotonically even when hello is already superseded. Current
rendering and dispatch remain with the newest greeting. Successful absence is
distinct from read failure; uncertain history fails closed with the same update
message plus retry/refresh advice. Account reset retires pending and in-memory
history, and old-account greetings cannot introduce evidence.

Node **22.23.3** / npm **12.0.2**, every gate under `nice -n 10`: transport ci
`--ignore-scripts`, SPA ci `--legacy-peer-deps`, lint, focused units/browser
(**19 files / 285 tests**), all three `test:captures` scripts and build exit **0**.
Extra fixture captures reproduce both late-hello and history-put refusal at
390/1280 light/dark, retaining the note and selection with zero assignment sends
(**8 captures**, exit **0**). Complexity caps and ratchet entries are unchanged.
Semgrep on **47 changed tracked files**, **200 rules / zero findings**, exits
**0**. The full suite is intentionally deferred to the serial gate. Final
Gitleaks, diff-check, pushed SHA and snapshot evidence are recorded on #564.
TS558-7 includes the explicitly accepted re-pair UUID/history loss described
above; identity linking is outside this fix. No bridge execution, pairing,
private-directory access, install scripts or independent Astra review occurred.

W5 round 11 / #564 (2026-10-13) accepts the unpersisted-evidence lifetime
residual described beside TS558-7, with retry and existing-channel mitigations.
The later-capable-greeting retry test already passed on round 10: that path
always attempts the atomic merge again, and a successful retry survives module
reload. The new independent-tab refused-put test failed first because the
listening tab had no evidence (**1 failure, exit 1**). It now passes: capable
observations publish transient monotonic evidence through the existing
`build-cache` BroadcastChannel before storage, and open listening tabs retain
it without changing rendering support or granting greeting authority. Receipt
never echoes the message. No new channel or durable fallback is introduced.

`taskWaitingOnSharing.test.js` covers refused/unavailable storage, device
isolation, unchanged rendering support and a missing-channel local fallback.
`browser/taskWaitingOnSharing.test.js` uses two fixture Chromium tabs sharing
IndexedDB and the native channel, injects refused capability persistence, then
verifies the other tab's real picker sends no assignment and retains its note.
`taskWaitingOnSupport.test.js` proves a later capable greeting retries a refused
first put, persists history, and refuses downgrade after module reload.

Node **22.23.3** / npm **12.0.2**, all gates under `nice -n 10`: lint,
focused units/browser (**21 files / 290 tests**) and build exit **0**. Semgrep
on **50 changed tracked files**, **200 rules / zero findings**, exits **0**.
The prior late-hello, storage-refusal, detail-refresh and complexity-ratchet
checks remain in the focused run. Full suite and captures were not requested
for this narrow round; the serial gate owns full-suite verification. TS558-7
is verified with the two explicitly accepted residuals (re-pair UUID loss and
unpersisted evidence after page lifetimes end). Final Gitleaks, diff-check,
pushed SHA and snapshot evidence are recorded on #564. No new complexity
exceptions, ratchet entries, independent Astra review or security-constraint
exceptions.

## #696 shared evidence intake — 2026-10-20

Agent chat attachments reuse the existing task evidence intake policy without
changing task file limits, task reads or local storage trust. The new chat
entry points and readers are scoped separately in
[Agent Message Attachments Security Checklist](Agent%20Message%20Attachments%20Security%20Checklist.md).
The task controls and their historical evidence remain unchanged.

## #704 task read consistency — 2026-10-22

Task read ownership remains scoped by device, project and task. Inbox and the
project Tasks badge reconcile accepted detail reads with cached list state;
neither manufactures a read cursor or draws from connection state. Existing
timeline-key validation remains in force. The task-page New divider uses the
watch baseline already used by Inbox.

`task704UnreadCache.test.js` covers new watched comments, accepted reads,
passive reads from another client, cache-reader restart and stale detail replies.
The fixtures verify that an accepted read clears both badge and Inbox, while
reload and stale detail cannot resurrect unread. Task-line markup is unchanged;
`taskLineFixture.test.js` remains the fixture consistency gate. Final gate exit
codes and the scoped security score are recorded in the #704 handoff.

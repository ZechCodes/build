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
| TS558-4 | Removing the last edge can release only a still-current explicit assignment; unassigned, cancelled, superseded and consumed assignments cannot start work. UI states the consequence of removing an armed final blocker. | W2 verified: `removing_last_edge_delivers_armed_and_notifies_unassigned_without_rearming_cancelled`, `deferred_dispatch_generation_fences` and `unblock_fan_in_and_fan_out` prove armed release, cancellation, reassignment and no redispatch after a consumed assignment is reblocked/unblocked. The two `admitted_delivery_reblocked_before_settlement` cases prove one informational holder notice after Posted/reblock/restart, with and without prior tracking, through receipt settlement and another restart. W3 adapter release/no-redispatch evidence is verified in #562; W5 task-page removal/browser evidence remains #564. |
| TS558-5 | Self-edges, cross-project edges and cycles (including resolved edges) are refused in the transaction that writes; bounds and deduplication apply to batch creation and edits. | W1/W3: concurrent opposite-edge attempts, multi-edge atomic refusal, duplicate and size-limit tests. |
| TS558-6 | Recovery creates at most one resource set and delivery per assignment, preserving the admitted closed-without-Done list and assessment/escalation instruction; cancellation and late Complete cannot resurrect a retired generation. | W2 verified: `deferred_dispatch_restart_boundaries` covers Reserved/Materialized/Agent/Frozen/Posted/Settled with one workspace, message and receipt; frozen retry tests preserve original context after edits/restart. `closed_blockers_delivery_context` and `closed_blockers_coordinator_notice_context` verify the retained closure list and assessment/escalation instruction. `delivery_receipt_recovery_keeps_reblocked_task_column_and_timeline` proves consistent settlement. `tracker_completion_fences` covers retired owners, consumed pending preservation, reset reconstruction/captured turns and stale Complete. `completion_fence_two_queued_assignments_keep_unrelated_turn_fenced` pins competing B/C markers during an unrelated Working interval and a later freshly waiting assignment. `a_v16_database_migrates_to_v17_without_arming_existing_assignments` uses frozen schema-16 DDL, preserves task record bytes and dependencies through upgrade/reopen, checks defaults/indexes/foreign keys and empty intent/notice tables, and grants no authority from existing assignments. W3 wire/retry contract evidence is verified in #562 by exact saved-intent retries and pinned delivery/notice fixtures. |
| TS558-7 | Dependency UI renders escaped cached facts and capability-gates writes; an older bridge never receives a waiting form as an immediate assignment. | W4 / #563 cached-grouping evidence is verified below: hostile-title escaping, cache-first/push grouping, Backlog Blocked/Ready without label or column writes, history precedence and older-capability rendering. W5 / #564 owns resolved-by-close edge presentation, current-greeting write refusal and refusal of waiting forms on older bridges. |

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
W4/W5 UI and browser evidence, including TS558-4's removal warning and
TS558-7, remains pending; this score does not claim those controls.

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

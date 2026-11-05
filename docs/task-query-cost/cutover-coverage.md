# #894 producer and recovery coverage map

This is a source audit of the test cases present in the #894 working tree.
**A case listed here is not an execution claim.** This audit ran no tests;
the execution receipts below are imported from the parent's retained process
records. Final-head gate logs, exact source identity, exits and ignored-test
disposition remain separate from this source inventory. The startup
performance runs are separately recorded in [cutover-protocol.md](cutover-protocol.md)
and do not prove producer coverage or the whole recovery matrix.

The durable guarantee applies to the `device`, `project_tasks` and `task`
journal scopes. Git/filesystem contents and process liveness retain their
separate observation/read-on-open paths; their limitations are listed below.

## Producer inventory boundary

[`producer_guard.rs`](../../bridge/src/store/sync/producer_guard.rs)
`every_canonical_sql_writer_has_an_audited_publication_owner` parses Rust syntax
under `bridge/src/store`, including literal SQL, referenced statement constants
and formatted table writes. Its pinned inventory covers:

- `tracker_tasks`, `tracker_comments`, `tracker_events`,
  `tracker_task_dependencies`, `reviews`, `tracker_delivery_intents`;
- `agents`, `thread_items`, `attention`, `captures`, `implementations`, `tasks`,
  `archived_worktrees`, `operations`, `inbox_retained_messages` and
  `inbox_retained_run_messages`;
- named `meta:user_session` and `meta:workspace_lifecycle` writers.

The same test pins calls from source owners through their maintenance hooks,
and the common finalizer's `collect_tasks → collect_device → publish → commit`
boundary in `writer.rs::transaction_with_publications`. Its additional controls
are `device_followup_inventory_covers_operation_receipt_mutations`,
`device_followup_inventory_covers_named_meta_but_excludes_unrelated_metadata`
and `device_followup_inventory_rejects_a_missing_device_finalizer`.

This guard deliberately excludes `store/sync` internals and test-only code.
Startup migrations, dynamic rename/reset programs and private-only operation
maintenance are classified separately in the pinned inventory. A new table or
writer still needs classification and semantic coverage. Finding a hook's name
in a function does not prove that the hook executes, publishes the right value,
or shares the source transaction; the following semantic cases are necessary.
Arbitrary SQL writes are outside the supported producer guarantee; explicit
import/repair is the recovery path, as approved for the task projections.

## Task producers

Exact cases below are in
[`task_producer_tests.rs`](../../bridge/src/store/sync/task_producer_tests.rs).

| Producer/change | Exact semantic test |
| --- | --- |
| Create, title/body edits, unchanged save and task-stream routing | `canonical_create_edit_and_unchanged_save_publish_once_per_commit` |
| Appended comments/events, several activity appends in one transaction | `multiple_activity_appends_in_one_transaction_have_one_revision` |
| Explicit repair and history-only changes absent from a card | `summary_repair_is_quiet_when_identical_and_publishes_history_only_change` |
| State, status, assignment, labels, trackers, read/watch/unwatch and dismiss | `task_state_status_assignment_labels_and_read_watch_dismiss_writers_publish` |
| Review header changes without a card delta | `snapshot_review_header_change_invalidates_task_even_without_a_card_delta` |
| Queued/failed delivery facts versus private delivery checkpoints | `committed_delivery_summary_changes_publish_but_private_checkpoints_do_not` |
| Dependency edges, both endpoint summaries and readiness reconciliation | `dependencies_publish_both_endpoints_and_readiness_reconciliation` |
| Refused project reparent and project deletion/tombstone | `task_project_is_immutable_and_project_delete_keeps_its_tombstone` |
| Publication failure rolls back the canonical task and journal together | `canonical_task_and_journal_both_rollback_on_append_failure` |
| Runtime availability is excluded from durable task publication | `ephemeral_identity_availability_does_not_publish_a_task_change` |

Canonical summary equivalence and import/repair paths also have independent
producer cases in
[`producer_transactions.rs`](../../bridge/src/store/tests/task_summaries/producer_transactions.rs):
`store_task_metadata_history_and_read_markers_match_canonical_records`,
`store_roster_owner_changes_and_deletion_keep_identity_consistent`,
`failed_project_deletion_restores_canonical_tasks_and_all_projection_memberships`,
`explicit_import_repair_restores_consistency_after_an_unsupported_raw_write`,
`import_repair_of_a_missing_task_removes_its_card_facts_and_memberships`,
`native_delivery_changes_and_projection_failure_remain_atomic`,
`public_repair_batch_is_atomic_and_rebuilds_canonical_history_without_duplicate_counts`
and `native_blocker_lifecycle_refreshes_counterparts_and_rolls_back_on_projection_failure`.
[`producer_reviews.rs`](../../bridge/src/store/tests/task_summaries/producer_reviews.rs)
adds `native_pr_publication_opinion_and_projection_failure_are_atomic`.

## Device record families and source owners

Tests named in the next table are in
[`device_producer_tests.rs`](../../bridge/src/store/sync/device_producer_tests.rs)
unless another file is linked in the row. These are supported canonical-writer
cases, not just manually staged `PublicationBatch` cases.

| Published family | Canonical owner and exact regression |
| --- | --- |
| `project`, `project_source` | Registry/config mutators, durable marker, authoritative reconcile. [`device_lifecycle_tests.rs`](../../bridge/src/app/device_lifecycle_tests.rs): `lifecycle_project_sources_publish_registry_ids_and_only_durable_fields`. |
| `workspace`, `workspace_directory`, `badge` | Workspace registry/lifecycle/adoption/reopen/capability observations. [`device_lifecycle_tests.rs`](../../bridge/src/app/device_lifecycle_tests.rs): `lifecycle_reconcile_tombstones_deleted_registry_members_without_runtime_badges`, `device_followup_reopen_publishes_and_marker_failure_preserves_the_manifest`, `device_followup_background_capability_refresh_publishes_without_a_list_note`, `device_followup_adopted_owner_marks_before_staging_its_workspace_alias`. |
| `agent`, `identity` | Agent save/roster deletion and compact directory facts. `agent_and_conversation_writers_publish_current_rows_and_roster_deletions`; `device_334_pin_republishes_later_ordinal_roles_and_owner_pin_atomically`; `device_334_pin_keeps_missing_recorded_pin_and_null_roles_without_inference`. |
| `inbox_conversation` | Canonical committed item, read/watch/mute/reset, attention and receipt hooks. `agent_and_conversation_writers_publish_current_rows_and_roster_deletions`; `device_canonical_thread_changes_reach_every_alias_and_reset_transactionally`; `device_followup_watch_floor_and_muting_do_not_resurrect_old_attention`; `device_delivery_ack_and_terminal_attention_publish_durable_summary_changes`; `device_followup_pending_only_receipts_refresh_all_canonical_aliases`. |
| `inbox_task` | Existing task summary finalizer, including dependency/review/delivery updates. `task_capture_and_user_session_writers_publish_device_rows_and_deletions`, plus the task-producer cases above. |
| `inbox_capture` | Capture save/delete in the canonical transaction. `task_capture_and_user_session_writers_publish_device_rows_and_deletions` asserts membership and excludes private capture body bytes. |
| `user_session` | Both unconditional and conditional committed session writers. `task_capture_and_user_session_writers_publish_device_rows_and_deletions`; `device_sql_source_and_projection_share_rollback_and_identical_writes_are_quiet`; inventory pins both `save_user_session` and `save_user_session_if_newer`. |
| `archived_worktree`, `workspace_lifecycle` | Archive save and lifecycle-meta save. `device_followup_archived_worktree_and_workspace_lifecycle_publish_durable_rows`; durable fields are separated from live holds/measurements. |
| `owner_session`, `project_session` | Pooled live/retained conversation activity and explicit owner pin. [`device_session_tests.rs`](../../bridge/src/store/sync/device_session_tests.rs): `device_334_sessions_unwatched_activity_advances_pooled_owner_and_project`, `device_334_sessions_pool_interleaved_conversations_and_replay_exactly`, `device_334_sessions_mapping_joins_retained_and_live_pools_exactly`, `device_334_sessions_reset_recomputes_owner_and_project_from_remaining_history`. [`device_lifecycle_tests.rs`](../../bridge/src/app/device_lifecycle_tests.rs): `device_334_sessions_equal_workspace_and_project_list_snapshots_for_unwatched_activity`. |

Additional cross-family invariants in `device_producer_tests.rs`:

| Boundary | Exact regression |
| --- | --- |
| Streaming/progress/status/still-working output must not amplify durable publication | `committed_conversation_items_have_linear_or_better_journal_amplification`, `device_summary_ignores_status_progress_and_still_working` |
| Unchanged attention/owner records remain quiet | `device_fix_round_attention_unchanged_save_journals_nothing_and_only_changed_owners_are_touched` |
| All canonical aliases share committed thread/reset facts | `device_canonical_thread_changes_reach_every_alias_and_reset_transactionally`, `device_followup_empty_conversation_reset_publishes_the_durable_generation` |
| Incremental unread/summary work does not scan old transcript metadata | `device_unread_counters_are_incremental_and_last_summary_uses_indexed_seek`, `device_followup_hot_projection_does_not_decode_historical_agent_metadata` |
| Retain/delete/prune/reset bookkeeping remains bounded and keeps facts | `device_followup_counter_reset_has_constant_writer_work_and_bounded_cleanup`, `device_followup_retained_owner_deletion_has_no_history_dependent_producer_cost`, `device_followup_retained_summaries_use_saved_facts_after_owner_history_is_removed` |
| Committed visible-item selection excludes uncommitted/private items | `device_followup_summary_keyset_contains_only_committed_visible_items`, `device_followup_build_notices_do_not_advance_session_timing` |
| Missing compact facts fence instead of exposing partial rows | `device_missing_facts_fences_scope_without_aborting_pruning_and_repairs_atomically`, `device_missing_facts_startup_repairs_without_agent_save_and_rejects_old_generation`, `device_missing_facts_startup_repair_chunks_resume_after_publication_failure` |
| Resumable startup/backfill and first-reader admission | `device_upgrade_backfills_existing_sql_sources_before_leases`, `device_followup_backfill_resumes_after_a_committed_chunk_and_refuses_partial_admission`, `device_followup_backfill_links_retained_owners_with_an_empty_roster` |
| Older-writer rollback invalidates the completed 3.34 coverage, then rebuilds | `device_334_rollback_old_writer_invalidates_completed_pin_and_pool_coverage`, `device_334_pin_upgrade_reprojects_saved_roles_and_pins_without_agent_save` |

The #894 append budget is separately pinned in
[`conversations.rs`](../../bridge/src/store/tests/conversations.rs),
`appending_one_message_writes_a_constant_row_budget_however_long_the_conversation_is`.
It attributes every write by table and compares the exact map at histories 1,
60 and 4096. The statement-cache control belongs with its own before/control/
restore evidence; this coverage map does not assert that it has run.

## Non-SQL registry publication and failures

These cases are in
[`device_lifecycle_tests.rs`](../../bridge/src/app/device_lifecycle_tests.rs),
which is outside `store::sync::` and must be included explicitly in gate
selection if runtime coverage is claimed:

| Mutation/failure window | Exact regression |
| --- | --- |
| Persist marker before a manifest/memory effect; refusal must prevent effect | `lifecycle_marker_failure_refuses_workspace_change_before_manifest_or_memory`, `device_followup_workspace_get_cannot_refresh_after_marker_failure` |
| Restart on either side of the durable marker | `lifecycle_startup_recovers_crashes_on_both_sides_of_dirty_marker`, `device_followup_startup_marker_failure_cannot_expose_stale_device_rows` |
| Deferred/prepared work holds the fence until authoritative settlement | `lifecycle_deferred_workspace_keeps_fence_until_authoritative_settlement`, `device_followup_prepared_delivery_holds_the_fence_before_job_accounting` |
| Branch/capability changes publish without relying on a list read | `lifecycle_observation_branch_change_marks_before_manifest_and_publishes_then_settles`, `device_followup_background_capability_refresh_publishes_without_a_list_note` |
| One failed workspace must not starve another or leave the device fenced | `lifecycle_observation_failed_manifest_does_not_starve_later_workspace_or_fence_reads`, `lifecycle_observation_failed_reopen_does_not_starve_later_workspace_or_fence_reads` |
| Unchanged ticks/read jobs/unrelated Git work do not create or hold a marker | `lifecycle_observation_unchanged_ticks_do_not_advance_dirty_generation`, `lifecycle_observation_read_job_does_not_fence_or_write_an_unchanged_registry`, `lifecycle_observation_unrelated_git_job_does_not_hold_an_existing_marker`, `lifecycle_observation_registry_job_settles_while_unrelated_read_remains_taken` |
| Refused stage handoff releases its registry fence | `device_fix_round_refused_registry_handoff_does_not_strand_its_fence` |

Store-level generation races and rollback use
[`device_tests.rs`](../../bridge/src/store/sync/device_tests.rs)
`dirty_markers_survive_restart_and_generation_races_refuse_stale_reads` and
`dirty_reconcile_rolls_back_projection_journal_and_marker_clear_together`, plus
[`device_entry_tests.rs`](../../bridge/src/store/sync/device_entry_tests.rs)
`dirty_clear_remark_rejects_delayed_generation_across_restart` and
`device_followup_authoritative_reconcile_preserves_newer_per_entity_markers`.

## Missed-event and recovery matrix

The following file labels are used to keep long test names readable:

| Label | Exact file |
| --- | --- |
| bridge core | [`store/sync/tests.rs`](../../bridge/src/store/sync/tests.rs) |
| bridge device | [`store/sync/device_tests.rs`](../../bridge/src/store/sync/device_tests.rs) |
| bridge entry | [`store/sync/device_entry_tests.rs`](../../bridge/src/store/sync/device_entry_tests.rs) |
| bridge task RPC | [`app/tracker/sync_tests.rs`](../../bridge/src/app/tracker/sync_tests.rs) |
| SPA engine | [`scopedSyncEngine.test.js`](../../spa/test/scopedSyncEngine.test.js) |
| SPA recovery | [`scopedSyncRecovery.test.js`](../../spa/test/scopedSyncRecovery.test.js) |
| SPA reconnect | [`scopedSyncReconnect.test.js`](../../spa/test/scopedSyncReconnect.test.js) |
| SPA task cache | [`scopedTaskSyncCache.test.js`](../../spa/test/scopedTaskSyncCache.test.js) |
| SPA device cache | [`scopedDeviceSyncCache.test.js`](../../spa/test/scopedDeviceSyncCache.test.js) |

| Condition | Expected boundary | Exact tests present |
| --- | --- | --- |
| A commit lands after snapshot watermark or during task paging | Catch-up from the original watermark covers it | bridge task RPC `snapshot_watermark_catches_a_write_before_first_changes_and_during_paging`; SPA recovery `recovers entries and exits committed while a multi-page snapshot was being read` |
| Device records update/delete during leased paging | Frozen keyset/COW versions stay coherent, later changes remain recoverable | bridge device `leased_snapshot_keeps_copy_on_write_rows_through_update_and_delete`, `mutation_during_device_paging_keeps_the_leased_keyset_and_versions`; bridge entry `device_page_keyset_uses_first_sequence_and_frozen_upper_bound` |
| Snapshot is incomplete or lease expires mid-page | Publish no partial membership; restart only that snapshot | SPA engine `does not expose a partial snapshot while a later page is outstanding`, `restarts an expired multipage lease without publishing its abandoned rows`; SPA recovery `restarts an expired first snapshot lease with no previously stored cursor`; bridge device `lease_expiry_during_snapshot_is_rechecked_before_reply` |
| Catch-up takes multiple batches while more writes arrive | Drain a fixed head using one run origin; commit each applied position independently | bridge task RPC `changes_continuation_keeps_fixed_head_and_refuses_retention_gap`, `continuation_retention_uses_applied_position_and_accepts_floor_equality`; SPA engine `drains fixed-head continuations with the original run cursor and separate committed CAS` |
| A wake-up notification is lost while a scoped view remains mounted | The 30-second head check observes divergence and catches up | SPA engine `repairs a lost push at 30 seconds and skips catch-up when the applied head matches`; immediate wake/close behavior: `foreground resume and matching pushes wake immediately; closing stops head checks` |
| Ordinary bridge restart/new process epoch | Durable incarnation/cursor survives; old replies do not | bridge core `ordinary_restart_keeps_cursor_and_changes_only_reply_epoch`; bridge task RPC `live_snapshot_and_recovery_resume_after_an_intact_restart_with_new_reply_epoch`; SPA reconnect `continues the holder's committed cursor through a greeting gap and session reconnect` |
| Store restore/replacement or incompatible definition | Reject old positions and resnapshot affected scope | bridge core `fresh_store_explicit_restore_and_incompatible_schema_rotate_incarnation`, `closed_database_atomic_replacement_is_detected_even_with_copied_meta`; bridge task RPC `restore_refuses_old_snapshot_pages_and_applied_cursors`; SPA engine `resnapshots only the affected scope on %s` (`incarnation_changed`) |
| Long absence, retention compaction or event/byte over-budget | No replacement success cursor may skip a gap; reset that scope | bridge core `logical_retention_floor_is_stream_aware_before_physical_gc`, `retention_edges_for_age_count_and_bytes_are_indexed`; bridge task RPC `catchup_above_500_events_requires_resync_without_a_success_cursor`, `byte_budget_counts_envelopes_across_changes_batches`; SPA engine `resnapshots only the affected scope on %s` (`retention_expired`, `over_budget`, `lease_expired`) |
| Filter entry/exit, reassignment, unwatch or deletion | Remove stale membership in every cached spelling while retaining unrelated scopes | bridge task RPC `current_membership_covers_enter_leave_reassign_unwatch_and_project_delete`; SPA task cache `removes filter exits from every cached spelling of the same query, preserving other scopes`, `invalidates a filter exit without deleting shared task content or other cached memberships` |
| Delete followed by recreate, repeated delivery or out-of-order revisions | Current revision/membership and tombstones govern; historical operation cannot resurrect stale state | bridge task RPC `current_revision_can_be_newer_than_the_fixed_event_head`; SPA task cache `compares row revisions rather than delivery sequence and refuses an overtaken cursor`, `preserves a tombstone against same-revision intra-batch upserts, reversed=%s`; SPA device cache `historical deletes carrying the current recreated row preserve membership across split-page=%s` |
| Source/publication SQL failure | Canonical state, journal and projections roll back together | bridge core `same_transaction_append_commits_and_rolls_back_with_source`; task/device semantic rollback cases above |
| IndexedDB cursor/row write fails, even after another put was issued | Rows, membership and applied cursor all roll back; replay starts at last committed cursor | SPA recovery `replays a failed real IndexedDB batch from its committed snapshot cursor`; SPA task cache `rolls back rows and membership when cursor persistence fails; reread applies once`, `rolls back an already-issued cursor put when a later row put fails`; SPA device cache `cursor persistence failure rolls the whole device publication back` |
| Local mutation or generation retirement overtakes a capture | Reject the entire stale publication without advancing its cursor | SPA task cache `leaves optimistic mutations and their cursor untouched until a fresh post-mutation read`, `rejects an old cache-generation response even after an empty cache is recreated`; SPA device cache `a retired shared task generation rejects a device capture even without a local mutation`, `a retired conversation thread rejects stale device digests without advancing the cursor` |
| Session/re-greeting/reply epoch changes during read or commit | Old opening cannot publish or reset a newer position | SPA engine `discards an obsolete %s response`; SPA reconnect `fences an old publication inside its transaction while the new reply is still held`, `fences an old cursor reset inside its transaction while the new reply is held`, `keeps the newer opening's position after a late successful %s return` |
| Same engine reconnect versus real registration/lifetime boundary | Preserve own committed cursor across reconnect; resnapshot after reload/handover/key gap | SPA reconnect `retains a committed changes cursor when the opening ends before publication returns`, `snapshots when a reconnect's cursor was replaced outside this engine`, `still snapshots after a true %s lifetime boundary`; SPA recovery `snapshots reopened and handed-over views without reusing persisted cursor origins or registration` |
| Frozen holder and timeout recovery run concurrently | Applied-cursor CAS prevents late writer from losing rows | SPA recovery `keeps concurrent holder and timeout engines on committed cursor CAS without losing rows` |
| Deferred registry/missing-facts fence exists | Every device entry point refuses partial coverage; cached paint stays intact | bridge entry `device_dirty_fence_covers_existing_lease_head_events_and_snapshot`, `device_followup_dirty_fence_covers_device_continuation_helpers`, `device_334_pending_coverage_fences_reads_continuations_and_lease_admission`; SPA engine `silently retries a reconciling device scope with backoff outside the captured recovery` |
| A fence lasts longer than connection bootstrap | No loading/empty/error replacement of existing cache content | [`scopedDeviceSyncPaint.test.js`](../../spa/test/scopedDeviceSyncPaint.test.js) `keeps cached inbox and directory DOM and records identical through a long fence, then paints catch-up` |
| Scope invalidation or removal is received | Persist snapshot demand before cursor progress; remove just affected scope/entity | SPA task cache `persists invalidation before advancing the cursor`, `persists snapshot demand on invalidation through clean batches until snapshot publication`, `removes just the affected scope and retires a removed task through content authority`; SPA engine `drops membership and cursor without resnapshotting a removed scope` |
| Tombstones compact while cached old membership remains | Full resnapshot removes absent members instead of preserving stale ones | SPA device cache `a complete resnapshot retires previously tracked records omitted after retention expiry`, `a completed empty first snapshot clears legacy-only device membership but preserves independent records` |
| Capability/cache upgrade, downgrade and 3.34 → 3.33 → 3.34 rollback | Current greeting controls writes; cached content survives; returning definition rebuilds | [`scopedSyncRuntime.test.js`](../../spa/test/scopedSyncRuntime.test.js) `requires complete current ownership across 3.34 → 3.33 → 3.34 rollback`; [`cacheSyncLazyReads.test.js`](../../spa/test/cacheSyncLazyReads.test.js) `removes all-scope body subscriptions and unopened push repairs, then restores legacy behavior on downgrade`; bridge producer rollback tests above |
| Token is malformed, forged, wrong scope/purpose or future | Explicit refusal before disclosing/advancing authority | bridge core `scope_binding_and_authentication_are_required`, `malformed_future_and_large_decimal_cursors_are_explicit`; [`cursor_tests.rs`](../../bridge/src/store/sync/cursor_tests.rs) `f2_foreign_cursor_rejects_malformed_fields_before_incarnation_reset`, `f2_signature_tampering_including_all_spare_bit_variants_is_rejected`, `f3_bad_same_incarnation_tag_cannot_reveal_scope_mismatch`; bridge task RPC `recovery_rejects_bad_cross_scope_future_and_cross_purpose_tokens` |
| Recovery has an empty-page or large opaque-key prefix | Bound page/candidate/token budgets without skipping membership | bridge entry `device_candidate_budget_yields_through_a_tombstone_prefix`, `device_page_budget_is_one_mib_and_oversized_rows_refuse_explicitly`; SPA engine `bounds empty snapshot pages without publishing a partial membership`, `resnapshots device leases and preserves large opaque page and change continuations` |

## Read-pool and timing qualification

[`app/tests/runtime/sql_reads.rs`](../../bridge/src/app/tests/runtime/sql_reads.rs)
contains `task_sql_lane_answers_while_app_is_held_and_observes_next_commit` and
`scoped_sync_wire_reads_bypass_the_app_lock_and_device_head_is_available`.
These exercise actual wire dispatch while the app mutex is held. Their 500 ms
transport timeout proves held-lock independence; it is not the ≤100 ms read
performance bound.

The large-fixture ≤100 ms assertion is
`app::tracker::sync_tests::large_recovery_reads_finish_under_100ms_while_the_app_lock_is_held`.
It is `#[ignore = "timing: release-profile timing.yml"]`; a normal targeted
`cargo test --lib app::tracker::sync_tests::` does **not** execute it. A claim
that this timing gate passed requires its explicit release-profile ignored-test
run or separately attributed exact-source timing evidence. None was run by
this documentation audit.

## Filesystem/source-view boundary and unsupported claims

- There is no durable filesystem/Git event replay in the device/task journal.
  Git/files carry live observations, and view-open/reconnect reads refresh the
  cache. The journal's 30-second missed-push test cannot establish filesystem
  no-gap recovery or freshness after a lost filesystem notification while a
  view remains continuously open.
- A secondary **workspace source** has no independent live filesystem subject.
  [`gitPane.js`](../../spa/src/core/gitPane.js) documents that a durable workspace
  ID is not one of the run/project/external-worktree Git subjects. Its watcher
  retains the existing board/state fallback; source-qualified request/cache
  identity does not create a new filesystem subject.
  [`changeEvents.js`](../../spa/src/core/changeEvents.js) trims board scope to
  state, and original-project-source capability coverage to Git. This is an
  existing live-refresh limitation, not proof of a new source watcher.
- Mounted source reads are independently covered in
  [`syncVisibleReads.test.js`](../../spa/test/syncVisibleReads.test.js):
  `awaits the mounted source tree's forced cache writes for %j`,
  `refreshes only status, history and unpushed metadata from the source pane for %j`,
  `awaits a mounted source Changes plug's conditional read and cache write`,
  `fences a stopped pass's in-flight source %s answer` and
  `rechecks pass authority inside the source %s write transaction`.
  [`syncPhases.test.js`](../../spa/test/syncPhases.test.js) verifies mounted
  reads enter recovery and hidden callbacks do not. These establish bounded
  open/reconnect refresh and stale-write fencing; they do not manufacture live
  secondary-workspace filesystem notifications.
- Dirty-marker crash tests inject persistence/reconciliation failure windows
  and reopen stores/manifests. They are not destructive live-process kill or
  power-loss experiments. SQLite/source publication retains the existing WAL
  policy rather than strengthening it.
- Producer inventory/semantic tests cover supported writers and rendered
  families; they do not prove arbitrary future writers, unsupported raw SQL,
  every OS watcher behavior, encrypted-network loss, or every account's timing.
- Default targeted gates omit ignored release timing tests. No full SPA suite,
  installed bridge, live filesystem watcher integration or deployment was
  executed by this audit. The coordinator's serial gate remains separate.

## Retained gate attribution and remaining qualification

The original retained batch is copied without changing its exits in
[`gate-receipts/initial-rust.json`](gate-receipts/initial-rust.json) and
[`initial-spa.json`](gate-receipts/initial-spa.json). Its passing groups are
evidence for that batch, not a blanket pass at later code head `18749eab6`.
The later source contains the intervening fixes; fresh supplement/final-head
qualification remains pending and is tracked on #894.

Three Rust groups in that first batch exited 101: `changes-final` (stale
legacy `KindSet` fixture), `api-contract-final` (a fixture missing
`bridge_epoch`) and `rust-clippy-final` (two test-only complexity findings).
The source/fixture corrections have landed; this document does not infer a
successful rerun. Formatting and prior-wire/HTTP manifest checks in the
retained batch exited 0.

The first affected unit aggregate exited 1: 355/357 files passed, with 15
failed cases in `taskPageQueuedRepairOwnership.test.js` and
`taskPendingContentRecovery.test.js`. Every failed case name and the log hash
are retained in
[`unit-initial-failures.json`](gate-receipts/unit-initial-failures.json).
The parent subsequently reports all 15 corrected fixture cases passing in
focused runs; a new 359-file aggregate is still pending. The first layout
process exited 143. The lead explicitly directed a detached repeat; its
recorded exit is 0 with 76 files/597 tests, retained in
[`layout-repeat.json`](gate-receipts/layout-repeat.json). No third layout
run is claimed. The full state, including the earlier failures and remaining
work, is explicit in [`status.json`](gate-receipts/status.json).

| Required group | Why it is separate | Execution evidence in this audit |
| --- | --- | --- |
| `cargo test --lib store::sync::` | Journal/device/task producer, cursor, retention and startup tests | Retained `sync-final` process exit 0; later-head qualification pending |
| `cargo test --lib store::tests::` | Shared writer/projection/append budget regression coverage | Retained `store-tests-counted-final` process exit 0; later-head qualification pending |
| `cargo test --lib app::device_lifecycle_tests::` | Registry/non-SQL producers and marker ordering | Retained `device-lifecycle-final` process exit 0; later-head qualification pending |
| `cargo test --lib app::tracker::sync_tests::` | Task recovery RPC/snapshot/selection semantics | Retained `tracker-sync-final` process exit 0; ignored timing separate |
| `cargo test --lib app::tests::runtime::sql_reads::` | Wire read-pool dispatch while app/writer locks are held | Retained `sql-reads-final` process exit 0; fresh frozen-head supplement pending |
| Scoped SPA engine/recovery/reconnect/cache/presence/runtime/paint suites listed above | Real cache atomicity plus lifecycle/DOM boundaries | Earlier 357-file unit aggregate exit 1 for the two fixture files above; corrected final 359-file aggregate pending |
| New task timeline and directory paging contract suites | New 3.35 open-read shapes/continuations, legacy direction and cache behavior | Retained `tracker-api-final` and `filesystem-final` process exits 0; separate source-fix/final-head qualification pending |

### Final affected SPA selection: prepared, not executed

The final selection was generated from the five independently assembled
target/fixture lists, every new or changed SPA test, and all tests that directly
import, load, mock or inspect a changed JavaScript module. A refreshed fixed-
string search also covers the exact capability names, greeting versions and
fixture filenames, subscription identifiers, and added timeline/directory
record fields. Tests importing matching helper modules are included as well.

| Prepared list | Count | Status |
| --- | ---: | --- |
| `/home/zech/Projects/.agent-scratch/894/final-affected-spa-unit.txt` | 359 | Prepared, including `directoryLegacyRoot`, `checkoutWatcherLifecycle` and changed `cacheSyncChanges`; final aggregate pending |
| `/home/zech/Projects/.agent-scratch/894/final-affected-spa-browser.txt` | 76 | Detached repeat exited 0/597 tests; source/head attribution remains in the parent receipt |

The generator, selection reasons and literal-search evidence are preserved in
the same scratch directory as `collect-final-spa-union.py`,
`final-affected-spa-manifest.json`, `final-spa-exact-patterns.txt`,
`final-spa-exact-matches.txt`, `final-spa-module-patterns.txt` and
`final-spa-direct-module-matches.txt`. Selection generation exited 0; that is
not a test-suite result.

`syncTimingCutover.test.js`, `compat335.test.js` and `taskOpenCost.test.js` are deliberately excluded
from this ordinary affected-suite list because their measurements and exact-
source compatibility cohorts run separately and serially. The existing 18-row
performance cohort has its own recorded exits; the separate wire-3.35
old-source/new-source compatibility driver still needs its own evidence.
Capture tools, seed modules and diagnostic files that match literal searches
are listed explicitly as non-test matches in the manifest. They are not
Vitest suites; importing tests cover the helper fixtures. Standalone captures
such as `captureBoard.mjs` say they are tools, not tests, and were not silently
executed as screenshot-generation or live-diagnostic work.

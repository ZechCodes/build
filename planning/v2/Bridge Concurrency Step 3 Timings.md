# Bridge concurrency step 3 evidence

Task #686; 2026-10-20. This report records temporary-fixture
measurements and the Step 3 acceptance matrix. It is not a production workload
sample or a worker-count optimization study.

## Method and provenance

The existing Step 1 timing clock and file-backed carrier fixture remain the
measurement boundary. The [Step 2 evidence](<Bridge Concurrency Step 2 Timings.md>)
records the original Step 1 production revision
`502cfa9d3087ce72511ca0b26370c3436480b419` and its six-verb overlay; its final
paired capture is under `/tmp/build-685-paired.LnKyAj/`. Those older captures
cannot establish a Step 3 snapshot improvement: they measured six SQL verbs.

For this step, the complete pre-change tracked source at
`68494d2b23d68eb6c04257a8a83aefd4f78550a6` was preserved using `git archive`
in `/tmp/build-686-before-source/`. The timing fixture adds nine moved snapshot
verbs, eight computations each, through encrypted real carrier intake.
One request completes before the next isolated request; a held-work batch
finishes before the next batch to prevent board/project coalescing from changing
computation counts. Every held-work scenario keeps the gate for at least 75 ms.

Total means carrier intake to handler completion, including queueing. Queue is
the clock's `queued` phase, including admission and scheduling. Both use the
existing fixed histogram buckets; p95 is a bucket estimate, capped by the
observed maximum when appropriate. App wait/hold are independently measured;
percentile differences are not phase costs. Samples are small and sensitive to
machine scheduling and concurrent build work.

The first attempted shared-worktree six-SQL recapture failed compilation while
other agents were installing the new ReadView modules; it produced **no valid
measurements**. Its exit was 101 and log is
`/tmp/build-686-timing-pre/capture.log`. Clean archived source is used for the
pre-change snapshot capture.

## Pre-change capture

The behavioral red test ran directly from the archived test binary
`build_bridge-ac9b11f48af68cec` and failed as intended: the isolated
`board.list` accumulated 103.423 ms of app-lock hold rather than zero.
Exit **101** and original output are preserved in
`/tmp/build-686-snapshot-red/direct.log` and `direct.exit`. The subsequent
queued Cargo invocation also failed with nonzero app wait, independently.

For valid legacy measurements, the archived timing-test overlay omits the
new snapshot zero-lock assertion and releases each 75 ms gate **before**
collecting legacy replies. The shipped tests instead require every snapshot
reply before release. Requests, fixture seeding, counts and clock boundaries
match; gate release placement expresses the expected old/new dependency.
Other agents' behavioral red overlays add only tests to this archive, including
`cfg(test)` declarations. `source.json` records every changed tracked file;
production code is the original archived revision.

```sh
BUILD_CONCURRENCY_BASELINE_DIR=/tmp/build-686-snapshot-before CARGO_TARGET_DIR="$PWD/bridge/target" nice -n 10 cargo test --manifest-path /tmp/build-686-before-source/bridge/Cargo.toml --lib concurrency_timing -- --nocapture --test-threads=1
```

Actual exit **0**: 11 tests passed. Raw artifacts and provenance are in
`/tmp/build-686-snapshot-before/`: `capture.log`, `capture.exit`,
`bridge-concurrency-baseline.jsonl`, `bridge-concurrency-baseline.md`,
`source.json` and `test-overlay.sha256`. The timing overlay SHA-256 is
`ef9f6c6777dee95f2505357bf3304bc807b1200b3d39e7e886f7b94a57e478c1`.
The archive has no `.git`, so emitted revision is `unknown`; the raw records
are unedited. Rust is 1.98.1 and crate version 0.2.12. There are six scenario
records: three original SQL samples and three nine-verb snapshot samples.
The emitter-serialization test writes its constructed metric into its own
fixture, outside this persistent capture.

## Final after capture and comparison

Following the review revisions, the source was frozen at
`7c6995511cc53f23170e8ea9ce1d404d9eca0da5`. The default Rust 1.98.1 binary
was run directly after the full unfiltered Cargo suite, formatting, both default
1.98.1 and stable 1.99.1 Clippy gates, and the serial Semgrep scan had each exited
0. No builds or scans ran during the capture. Git HEAD and the empty worktree
status matched before and after; this report is a later documentation change.

The previous after sample at source `6403cbe119fc80acc0b6dc60c8904c2c05de1165`
is superseded by this capture. Its artifacts remain at
`/tmp/build-686-snapshot-after.bq2rn6xt/`; they are not the final after evidence.
The archived before capture at `68494d2b23d68eb6c04257a8a83aefd4f78550a6`
is unchanged.

```sh
python3 /tmp/build-686-recapture.py 7c6995511cc53f23170e8ea9ce1d404d9eca0da5
```

The guarded helper records provenance, invokes the following command without a
Cargo build, then validates the comparison:

```sh
BUILD_CONCURRENCY_BASELINE_DIR=/tmp/build-686-snapshot-final-after.7qugak56 nice -n 10 bridge/target/debug/deps/build_bridge-ac9b11f48af68cec concurrency_timing --nocapture --test-threads=1
```

Actual exit **0**: **12 passed, 0 failed, 0 ignored**, in 5.43 s. The directory
`/tmp/build-686-snapshot-final-after.7qugak56/` contains `capture.log`,
`capture.exit`, `command.json`, `bridge-concurrency-baseline.jsonl`,
`bridge-concurrency-baseline.md`, `bridge-snapshot-costs.json`,
`source-before.json`, `source-after.json`, `comparison.md` and
`p95-regressions.json`. The six scenario records retain the three original SQL
samples alongside the three snapshot samples; every snapshot method was
computed eight times per scenario. The toolchain was
`rustc 1.98.1 (48a229cea 2026-09-01)`, crate 0.2.12.

The executed binary SHA-256 is
`6bf3589b0e98cd642d7943be5c678c2e248acd8d6a766603d3162e739f5dba73`;
the unchanged timing-test source SHA-256 is
`63fa6eefd654203350deb95f4bf247dcb829cdb205b820c2828840acbe59f8cb`.

All entries below are **milliseconds**, using the existing clock histogram.
Every after-sample app-lock wait and hold sum, p50, p95 and maximum is exactly
zero, with eight observations per verb. The held-app and held-Git tests require
the snapshot replies while the gate remains closed.

| Scenario | Verb | Total p95 before | Total p95 after | Queue p95 before | Queue p95 after |
| --- | --- | ---: | ---: | ---: | ---: |
| isolated | board.list | 19.279 | 1.116 | 0.662 | 0.226 |
| isolated | project.list | 28.227 | 0.414 | 0.620 | 0.190 |
| isolated | workspace.list | 6.212 | 0.376 | 0.694 | 0.170 |
| isolated | workspace.get | 3.380 | 0.343 | 0.652 | 0.184 |
| isolated | agent.list | 1.603 | 0.313 | 0.649 | 0.175 |
| isolated | settings.get | 28.539 | 0.308 | 0.596 | 0.165 |
| isolated | models.list | 2.456 | 0.820 | 0.573 | 0.187 |
| isolated | harnesses.list | 1.099 | 0.446 | 0.577 | 0.169 |
| isolated | bridge.update_status | 0.797 | 0.222 | 0.662 | 0.156 |
| app_held_write | board.list | 100.364 | 2.549 | 0.742 | 0.485 |
| app_held_write | project.list | 147.314 | 0.978 | 0.542 | 0.492 |
| app_held_write | workspace.list | 111.280 | 0.858 | 105.555 | 0.419 |
| app_held_write | workspace.get | 115.771 | 0.849 | 112.609 | 0.436 |
| app_held_write | agent.list | 117.652 | 0.699 | 116.876 | 0.432 |
| app_held_write | settings.get | 145.295 | 0.708 | 117.861 | 0.377 |
| app_held_write | models.list | 148.447 | 1.924 | 146.365 | 0.377 |
| app_held_write | harnesses.list | 147.990 | 0.977 | 146.170 | 0.397 |
| app_held_write | bridge.update_status | 149.154 | 0.471 | 149.073 | 0.381 |
| off_lock_git_worker | board.list | 104.824 | 2.200 | 85.834 | 0.345 |
| off_lock_git_worker | project.list | 135.502 | 0.776 | 109.242 | 0.375 |
| off_lock_git_worker | workspace.list | 139.485 | 0.794 | 136.968 | 0.399 |
| off_lock_git_worker | workspace.get | 143.087 | 0.769 | 140.157 | 0.377 |
| off_lock_git_worker | agent.list | 144.838 | 0.631 | 143.735 | 0.376 |
| off_lock_git_worker | settings.get | 171.115 | 0.664 | 145.120 | 0.370 |
| off_lock_git_worker | models.list | 172.666 | 1.672 | 171.625 | 0.352 |
| off_lock_git_worker | harnesses.list | 176.618 | 0.938 | 176.273 | 0.354 |
| off_lock_git_worker | bridge.update_status | 177.829 | 0.389 | 177.749 | 0.311 |

All 27 total-p95 comparisons and all 27 queue-p95 comparisons improved. There
were **no isolated regressions**. Isolated total p95 ranges from 0.222–1.116 ms
after, versus 0.797–28.539 ms before. With the app writer held it ranges from
0.471–2.549 ms after, versus 100.364–149.154 ms before; with the ordinary Git
worker held it ranges from 0.389–2.200 ms after, versus 104.824–177.829 ms before.
The corresponding after queue-p95 ranges are 0.156–0.226, 0.377–0.492 and
0.311–0.399 ms. These are single small-fixture captures, not confidence intervals
or production latency claims.

## Producer, publication and clone costs

The direct intervals below have different scopes. The raw metric name
`global_workspace_and_fact_generation` is retained, but it measures an
**unguarded helper diagnostic**: direct calls to `prepare_global_snapshot` and
`prepare_workspace_snapshot_details`. It does not establish the ephemeral
`begin_snapshot_ownership` context used by the optimized production entry
points. Its p95 of 4.014080 ms therefore **cannot establish optimized global
producer cost**. The separately measured `owner_agent_digest_generation` is
also an isolated `agent_list` renderer without that shared context; it is not a
complete production publication.

`owner_generation_and_publication` calls the real top-level
`publish_owner_snapshot_projection`, including its ownership-context guard,
canonical owner projections, incremental history work and root publication.
Its p95 is **3.955530 ms** in this fixture. `root_publication` separately measures
publishing the diagnostic helper's prepared delta through the real root slot;
`root_arc_load` measures loading an `Arc`. Raw names and records are unchanged.

Each phase and clone has eight samples. Direct p50/p95 use nearest ranks on
measured `Duration`s, so p95 equals the maximum at this sample count. These are
not the carrier histogram estimates. All times below are milliseconds. The
separately repeated phases are **not additive**, and the inclusive owner phase
already includes generation and publication.

| Direct phase | Count | Sum ms | p50 ms | p95 ms | Maximum ms |
| --- | ---: | ---: | ---: | ---: | ---: |
| global_workspace_and_fact_generation | 8 | 31.810431 | 3.977892 | 4.014080 | 4.014080 |
| owner_agent_digest_generation | 8 | 1.644278 | 0.192995 | 0.270561 | 0.270561 |
| owner_generation_and_publication | 8 | 30.157382 | 3.740554 | 3.955530 | 3.955530 |
| root_arc_load | 8 | 0.004900 | 0.000541 | 0.001112 | 0.001112 |
| root_publication | 8 | 0.121218 | 0.014788 | 0.016611 | 0.016611 |

| Cached base value | Bytes | Count | Sum ms | p50 ms | p95 ms | Maximum ms |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| agent.list | 1158 | 8 | 0.042411 | 0.005089 | 0.006563 | 0.006563 |
| board.list | 9288 | 8 | 0.417508 | 0.051377 | 0.058501 | 0.058501 |
| models.list | 6419 | 8 | 0.382211 | 0.046327 | 0.054212 | 0.054212 |
| project.list | 1452 | 8 | 0.076524 | 0.009288 | 0.011101 | 0.011101 |
| settings.get | 1141 | 8 | 0.067135 | 0.008225 | 0.009808 | 0.009808 |
| workspace.get | 1338 | 8 | 0.065615 | 0.008056 | 0.009117 | 0.009117 |
| workspace.list | 1321 | 8 | 0.064592 | 0.007945 | 0.008967 | 0.008967 |

The clone table measures the cached base `serde_json::Value::clone()` and
records its serialized size; serialization is outside the clone interval.
`harnesses.list` and `bridge.update_status` use their own immutable service
snapshots, so they appear in the carrier comparison and are absent from this
root-backed clone table.

The fixture has one project, one managed workspace, eight tasks, eight retained
user messages, eight operation receipts and 64 activity events. Direct intervals
use warmed observations; they exclude startup filesystem/SQL/history warming
and the separately prepared transactional task-observation candidate. Base
clones precede service enrichment such as workspace history attachment, catalog
overlays, typed validation and serialization. Carrier totals cover intake
through handler completion, without a network round trip. This sample does not
measure large-project memory growth, refresh throughput, startup cost or optimal
worker counts.

## Actual publication ownership bounds

The #672 regression now counts **complete warmed publication plus facts reads**,
including cached ownership comparisons. A positive control,
`snapshot_publication_counts_cached_ownership_lookups`, proves the producer
counter is nonzero. `thread_push_facts_resolve_ownership_once_per_entity`
retains the original `8*N*N` bound at 16 and 64 owners, at most quadratic growth,
and identical producer counts when rosters grow from one to four agents. Its
separate immutable facts read performs zero comparisons.

A focused successful run in `/tmp/build-686-scoped-ownership-bounds.log` recorded:

| Owners | Shared root | One agent: publication comparisons | Four agents: publication comparisons |
| --- | --- | ---: | ---: |
| 16 | false | 1,121 | 1,121 |
| 64 | false | 16,769 | 16,769 |
| 16 | true | 1,101 | 1,101 |
| 64 | true | 16,314 | 16,314 |

The review reproduction had found 7,825/21,505 comparisons for the distinct-root
16-owner one/four-agent case before ownership contexts were shared. These are
separate operation-count regressions, not measurements from the historical
`68494d2` timing fixture or the final latency capture. The final full suite
reran their assertions. Additional passing controls cover cached comparisons
versus actual filesystem canonicalizations, nested guard cleanup, failed-candidate
context cleanup, direct/fallback/terminal/project/path-alias ownership parity,
and canonical alias/provider/parked-project compaction inheritance.

The final plaintext evidence export is
`/tmp/build-686-step3-final-evidence-7c6995511.txt`. It contains this report, both
raw timing captures, source provenance, comparison, direct costs, red outputs,
review mapping and successful focused ownership evidence. The files are local
temporary artifacts and must be exported to outlive this workspace. The older
`/tmp/build-686-step3-evidence.tar.gz` belongs to the superseded after sample.

## Step 3 acceptance matrix

| Contract | Passing test coverage | Scope and boundary |
| --- | --- | --- |
| Disjoint immutable deltas merge; previously loaded roots remain immutable | `read_view::tests::disjoint_deltas_merge_and_old_root_remains_immutable`; `concurrent_delta_publication_retains_every_disjoint_owner`; `current_builder_rebases_on_a_concurrent_publication` | Atomic chunk publication, concurrent owner merge and same-owner rebuilding against the latest root. |
| Stale refresh cannot resurrect deletion; multi-owner candidate is atomic | `read_view::tests::deletion_tombstone_rejects_stale_refresh_and_multi_owner_delta_is_atomic` | Tombstone/version primitive; the stale real-probe and project-deletion tests below exercise producer fences. |
| Plan failed save survives unrelated success and same-owner retry | `transactions::failed_plan_save_cannot_leak_through_an_unrelated_success_or_same_owner_retry` | Canonical state, durable SQL and the published agent chunk after unrelated success; the separate refreshed-push test pins retained thread content. |
| Run failed save survives unrelated success and same-owner retry | `transactions::failed_run_save_cannot_leak_through_an_unrelated_success_or_same_owner_retry` | Canonical state, durable SQL and the published agent chunk after unrelated success; the separate refreshed-push test pins retained thread content. |
| Failed save emits no change/news | `transactions::failed_finishers_emit_no_change_or_news` | No immediate failed event; later unrelated success explicitly refreshes both owners and rejects the failed words in pushed content. |
| Bounded rollback preserves history; multi-owner borrowing does not leak pending candidates | `transactions::retry_after_bounded_rollback_keeps_older_committed_history`; `borrowed_owner_defers_committed_events_until_failed_peer_is_restored` | Older SQL messages survive bounded in-memory rollback; borrowed peer failures delay publication/events until coherent completion. |
| Failed config saves cannot leak through later successful publication | `configuration::snapshot_publication::{failed_settings_persistence_cannot_leak_through_a_later_publication,failed_compaction_persistence_cannot_leak_through_a_later_publication}` | Injected write/rename failures retain the root and canonical settings; a later unrelated success must preserve the last committed fields. |
| Failed project config/source saves cannot leak through later publication | `configuration::project_snapshot_persistence::{failed_project_configuration_edits_never_reappear_after_later_success,failed_append_restores_source_ids_and_removes_only_owned_checkouts,failed_git_initialization_keeps_actual_git_but_not_unsaved_project_fields}` | Write and rename failure injection covers source removal/name/sync-base/isolation, later unrelated success, source-ID restoration and owned-checkout cleanup. Actual Git initialization remains observable while unsaved durable fields stay committed. |
| Project deletion removes its cached task rows | `configuration::project_snapshot_persistence::deleting_project_removes_its_cached_watched_tasks_from_snapshot` | Direct deletion publisher coverage alongside root tombstones. |
| Task observation rendering uses captured identities and paths | `tracker_observation_purity::{task_observation_rendering_uses_captured_paths_for_unnamed_live_actors,task_observation_rendering_preserves_retired_names_without_the_writer,dependency_edit_captures_a_new_actor_before_publishing_its_reply}` | Zero filesystem canonicalizations, while cached comparisons are allowed; a held Store writer cannot block retired-name rendering. Real MCP link/unlink edits capture each new actor before reply and retain that identity in SQL. |
| Older observations cannot overwrite completed Git/filesystem/dependency changes | `facts::observations::tests::{a_completed_same_branch_commit_rejects_an_older_probe,a_completed_file_mutation_rejects_an_older_root_listing,mutating_a_workspace_dependency_rejects_an_older_summary_probe}` | Paused real probes are rejected after newer canonical completions. |
| Task board rows publish from their committed transaction, retaining unrelated rows and dependency digests | `tracker_finished::{failed_watched_task_observation_keeps_the_published_rows,a_task_write_publishes_its_owned_row_without_rereading_other_tasks,a_task_write_publishes_dependency_digests_with_its_own_row,task_creation_rolls_back_when_its_watched_row_cannot_be_prepared}` | Failed observations preserve prior rows; successful writes prepare affected task/dependency projections transactionally, and a failed candidate cannot leave a committed task without its board row. |
| Git checkout publishes observed branch before reply, including detached fallback | `runtime::snapshot_publication::successful_checkout_reply_publishes_branch_to_board_and_workspace_snapshots`; `detached_checkout_publishes_the_recorded_branch_fallback` | Real checkout status and repository HEAD form the oracle; captured board/workspace snapshots must advance while detached HEAD retains the recorded branch fallback. |
| Failed goal/state/summary fields remain committed in detail, snapshots and pushes | `transactions::projections::{failed_plan_fields_stay_committed_in_detail_snapshot_and_push_after_refresh,failed_run_fields_stay_committed_in_board_detail_and_push_after_refresh,committed_plan_waits_for_failed_borrowed_run_without_losing_fields,committed_run_waits_for_failed_borrowed_plan_without_leaking_fields}` | Separate plan/run failures, unrelated success, retry and both mixed borrowed-owner orders. Summary is checked in detail because it is absent from board/push wire; legacy plans intentionally lack board rows. Six removed temporary leak injections each failed with exit 101. |
| Successful reset checkpoints survive later rollback/retry | `conversations::reset_checkpoint::{reset_plan_checkpoint_survives_failed_save_publication_and_retry,reset_run_checkpoint_survives_failed_save_publication_and_retry,reset_shared_conversation_checkpoints_its_canonical_plan_owner,reset_shared_conversation_checkpoints_its_alias_run_owner}` | A committed reset checkpoints canonical and alias owners; later failed save/unrelated publication/retry cannot restore deleted history, generation, resume metadata or summaries. |
| Release and failed borrowed-owner settlement keep publication live | `review_publication::{merge_release_keeps_later_reads_notices_and_news_live,merge_release_keeps_unrelated_routed_reads_live,git_action_orchestrator_error_keeps_owner_and_later_publication_live,adopted_implementation_qa_error_keeps_owner_and_later_publication_live,joined_dispatch_settlement_error_keeps_owner_and_later_publication_live}` | Actual release forgets its checkpoint; error exits restore their borrowed owner. Subsequent committed reads, changes and news continue. |
| Seen/mute/dismiss publish before acknowledgment | `board::snapshot_attention` (nine cases) | Named-agent and whole-owner seen publish cursors/unread; mute and alias handling preserve cursor independence. Dismiss does not imply reading; a new committed message invalidates dismissal, while a failed message cannot do so after unrelated publication. |
| Behavior suites exercise routed immutable reads | `runtime::snapshot_publication::qa_behavior_reads_use_the_published_snapshot`; ordinary `qa_state` initialization | A deliberately stale published value controls the routed answer. Existing behavior fixtures warm snapshots; only the explicit first-publication fixture starts cold. |
| Direct and claimed Git-stat refresh publish their installed values | `git::cache_publication::{changed_run_stat_is_published_after_install_before_snapshot_reads,completed_run_stat_refresh_is_published_before_snapshot_reads}` | Both cache-update paths publish before routed snapshot reads. |
| New adoption publishes its alias before notices; failed adoption retains isolation | `transactions::adoption::{adoption_finisher_first_publication_includes_its_workspace_alias,failed_adoption_cannot_publish_a_workspace_alias_or_remove_its_lock_marker}`; `review_publication::{adopted_implementation_publishes_its_workspace_before_acknowledgement,refused_implementation_handover_publishes_the_adoption_it_retains}`; `workflow::adoption` | First advertised root contains the saved workspace. Successful and refused handovers publish the adoption they retain; failed persistence cannot expose an alias through unrelated writes or remove its private checkout lock marker. |
| Shared ownership context preserves lookup and compaction semantics | Four `workspaces::conversation_git::projection_ownership` cases; `snapshot_projection::ownership::tests::nested_projection_reuses_ownership_until_the_outer_preparation_finishes`; `tracker_observation_purity::path_counters_distinguish_cached_comparisons_from_canonicalization` | Direct/fallback/terminal/project/path aliases, launch/provider bindings and parked-project inheritance match prior answers; nested and failed candidates cannot leave stale ownership context. |
| Best-effort activity keeps live observation after persistence failure | `transactions::failed_user_activity_persistence_keeps_its_best_effort_observation` | Existing intentional live-versus-durable semantics. |
| All migrated snapshot verbs complete with app held | `concurrency_timing::snapshot_reads_complete_while_app_held_write_is_gated` | Real encrypted intake; zero app wait/hold; nine verbs, eight computations each. |
| All migrated snapshot verbs complete with ordinary Git worker held | `concurrency_timing::snapshot_reads_complete_while_off_lock_git_worker_is_gated` | One ordinary worker, gated Git work; real intake bypass. |
| Isolated snapshot costs | `concurrency_timing::isolated_snapshot_reads_publish_a_file_backed_baseline` | Same fixture, serialized requests; report total and queue p95 together. |
| Catalog/update snapshots do not wait for service mutation locks | `harness::inventory::tests::inventory_snapshot_does_not_wait_for_observation_lock`; `update::service::tests::update_status_does_not_wait_for_subscription_lock` | Real service snapshots retain their values while the separate observation/subscription mutexes are held. |
| Facts finish while app/writer held | `facts::snapshot_tests::nonempty_push_facts_finish_with_application_and_store_writer_held` | File-backed committed thread/state/terminal/project facts complete while both app and writer transaction are held. |
| First publication retains canonical rows; no request maintenance | `runtime::snapshot_publication::first_snapshot_keeps_seeded_workspace_rows_and_performs_no_path_lookups`; projection/parity tests | First reads retain all seeded rows at 4/16 real workspace sizes, with zero request path comparisons; the producer regressions separately retain 16/64-owner bounds. |
| Producer field coverage and #672 bounds | `worktree_roots_resolves_workspace_ownership_without_cubic_scans`; `thread_push_facts_resolve_ownership_once_per_entity`; publisher tests | Complete warmed publication is counted at 16/64 owners with distinct/shared roots, equal one/four-agent counts and the original 8*N*N limit; the immutable facts reader performs zero comparisons. See the focused count table above. |
| Subscribe, write, post-ack read and push | `runtime::snapshot_publication::subscribed_workspace_write_is_visible_to_snapshot_and_next_push`; settings/compaction tests | Subscribe, acknowledge rename, read through a previously captured immutable service, then flush and compare the pushed name/project. |
| Snapshot/control progress under SQL saturation; no capacity refusal | `carrier::snapshot_tests::snapshots_bypass_saturated_write_and_sql_admission` | Eight SQL reader workers are occupied, sixteen requests queue and the next ordered intake parks. All nine snapshot methods and native controls answer while app and an actual Store writer transaction remain held. Closing the shared opening prunes backlog and wakes intake before release; normal missing/unavailable errors remain, with no capacity refusal. |
| Agent removal failure separates actual retirement from committed roster | `transactions::failed_agent_remove_keeps_committed_roster_and_cursors_but_retires_delivery`; `tracker_identity_removal::{removed_live_agent_invalidates_tasks_even_if_saving_roster_fails,failed_removal_retires_live_binding_without_reactivating_task_identity,configured_idle_agents_remain_available_in_routed_task_reads}` | Actual session retirement remains unavailable after failed roster persistence; configured idle agents remain available, and installing a new live session clears retirement. Routed SQL task reads and pure identity overlays agree. |

The final full Cargo test gate passed at the measured source, as did formatting
and both Clippy toolchains. This matrix names the specific Step 3 contracts
covered by that suite; the timing capture alone establishes neither failure
isolation nor the absence of every possible SQL/filesystem call. Held-writer,
filesystem-canonicalization and producer-comparison counters, immutable-service
and real-backlog saturation regressions exercise those boundaries independently.
The suite does not exhaustively cross every producer, failure mode and workload
size. The nine migrated verbs are the complete timing set;
`term.list` remains outside this Step 3 migration.

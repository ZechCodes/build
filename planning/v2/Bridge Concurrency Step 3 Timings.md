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

The source was frozen at
`6403cbe119fc80acc0b6dc60c8904c2c05de1165`. The default Rust 1.98.1 binary
was run directly after the full suite, formatting and both default 1.98.1 and
stable 1.99.1 Clippy gates had each exited 0. No builds or security scans ran
during this capture. Git HEAD and the empty worktree status matched before
and after the sample; subsequent changes to this report do not change the
measured source.

```sh
BUILD_CONCURRENCY_BASELINE_DIR=/tmp/build-686-snapshot-after.bq2rn6xt nice -n 10 bridge/target/debug/deps/build_bridge-ac9b11f48af68cec concurrency_timing --nocapture --test-threads=1
```

Actual exit **0**: **12 passed, 0 failed, 0 ignored**, in 5.47 s. The directory
`/tmp/build-686-snapshot-after.bq2rn6xt/` contains `capture.log`, `capture.exit`,
`bridge-concurrency-baseline.jsonl`, `bridge-concurrency-baseline.md`,
`bridge-snapshot-costs.json`, `source-before.json`, `source-after.json` and
`comparison.md`. The six scenario records retain the three original SQL samples
alongside the three snapshot samples; each snapshot method was
computed eight times per scenario. The toolchain was
`rustc 1.98.1 (48a229cea 2026-09-01)`, crate 0.2.12.

The executed binary SHA-256 is
`8fd6340b49bf202a8ddb553d7101f0b1d58891398fe5b21a393142e6f02c70c4`;
the measured timing-test source SHA-256 is
`63fa6eefd654203350deb95f4bf247dcb829cdb205b820c2828840acbe59f8cb`.

All entries below are **milliseconds**, using the existing clock histogram.
Every after-sample app-lock wait and hold sum, p50, p95 and maximum is exactly
zero, with eight observations per verb. The held-app and held-Git tests require
the snapshot replies while the gate remains closed.

| Scenario | Verb | Total p95 before | Total p95 after | Queue p95 before | Queue p95 after |
| --- | --- | ---: | ---: | ---: | ---: |
| isolated | board.list | 19.279 | 1.297 | 0.662 | 0.207 |
| isolated | project.list | 28.227 | 0.450 | 0.620 | 0.241 |
| isolated | workspace.list | 6.212 | 0.385 | 0.694 | 0.173 |
| isolated | workspace.get | 3.380 | 0.365 | 0.652 | 0.178 |
| isolated | agent.list | 1.603 | 0.354 | 0.649 | 0.191 |
| isolated | settings.get | 28.539 | 0.306 | 0.596 | 0.159 |
| isolated | models.list | 2.456 | 0.855 | 0.573 | 0.167 |
| isolated | harnesses.list | 1.099 | 0.465 | 0.577 | 0.195 |
| isolated | bridge.update_status | 0.797 | 0.326 | 0.662 | 0.233 |
| app_held_write | board.list | 100.364 | 2.264 | 0.742 | 0.427 |
| app_held_write | project.list | 147.314 | 0.825 | 0.542 | 0.394 |
| app_held_write | workspace.list | 111.280 | 0.749 | 105.555 | 0.335 |
| app_held_write | workspace.get | 115.771 | 0.689 | 112.609 | 0.331 |
| app_held_write | agent.list | 117.652 | 0.677 | 116.876 | 0.403 |
| app_held_write | settings.get | 145.295 | 0.702 | 117.861 | 0.365 |
| app_held_write | models.list | 148.447 | 1.752 | 146.365 | 0.349 |
| app_held_write | harnesses.list | 147.990 | 1.009 | 146.170 | 0.420 |
| app_held_write | bridge.update_status | 149.154 | 0.449 | 149.073 | 0.350 |
| off_lock_git_worker | board.list | 104.824 | 2.270 | 85.834 | 0.357 |
| off_lock_git_worker | project.list | 135.502 | 0.943 | 109.242 | 0.461 |
| off_lock_git_worker | workspace.list | 139.485 | 0.838 | 136.968 | 0.407 |
| off_lock_git_worker | workspace.get | 143.087 | 0.819 | 140.157 | 0.405 |
| off_lock_git_worker | agent.list | 144.838 | 0.690 | 143.735 | 0.407 |
| off_lock_git_worker | settings.get | 171.115 | 0.638 | 145.120 | 0.369 |
| off_lock_git_worker | models.list | 172.666 | 1.779 | 171.625 | 0.317 |
| off_lock_git_worker | harnesses.list | 176.618 | 0.902 | 176.273 | 0.322 |
| off_lock_git_worker | bridge.update_status | 177.829 | 0.392 | 177.749 | 0.308 |

There were **no isolated total-p95 or queue-p95 regressions** in this sample.
Isolated total p95 ranges from 0.306–1.297 ms after, versus 0.797–28.539 ms
before. With the app writer held it ranges from 0.449–2.264 ms after, versus
100.364–149.154 ms before; with the ordinary Git worker held it ranges from
0.392–2.270 ms after, versus 104.824–177.829 ms before. These are single
small-fixture captures, not confidence intervals or production latency claims.

## Producer, publication and clone costs

The actual warmed producer is measured independently from carrier latency.
`global_workspace_and_fact_generation` calls `prepare_global_snapshot` and
`prepare_workspace_snapshot_details`; `root_publication` then publishes that
delta through the real root slot. `root_arc_load` measures loading its `Arc`.
`owner_agent_digest_generation` renders `agent_list` with project workers;
`owner_generation_and_publication` invokes the full owner publisher, including
its incremental history work and root publication. This exposes the cost moved
onto publication: global/workspace/fact generation has p95 6.945391 ms, and the
inclusive owner completion has p95 4.572989 ms in this fixture.

Each direct phase and clone has eight samples. Direct p50/p95 use nearest ranks
on measured `Duration`s, so p95 equals the maximum at this sample count. These
are not the carrier histogram estimates. All times below are milliseconds.
The separately repeated phases are **not additive**, and the inclusive owner
phase already includes generation and publication.

| Direct phase | Count | Sum ms | p50 ms | p95 ms | Maximum ms |
| --- | ---: | ---: | ---: | ---: | ---: |
| global_workspace_and_fact_generation | 8 | 36.195581 | 4.164127 | 6.945391 | 6.945391 |
| owner_agent_digest_generation | 8 | 1.593240 | 0.197653 | 0.207311 | 0.207311 |
| owner_generation_and_publication | 8 | 35.223968 | 4.348045 | 4.572989 | 4.572989 |
| root_arc_load | 8 | 0.005560 | 0.000591 | 0.001222 | 0.001222 |
| root_publication | 8 | 0.160224 | 0.020459 | 0.028274 | 0.028274 |

| Cached base value | Bytes | Count | Sum ms | p50 ms | p95 ms | Maximum ms |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| agent.list | 1158 | 8 | 0.045377 | 0.005510 | 0.006773 | 0.006773 |
| board.list | 9297 | 8 | 0.436416 | 0.052720 | 0.060655 | 0.060655 |
| models.list | 6419 | 8 | 0.391197 | 0.047690 | 0.057618 | 0.057618 |
| project.list | 1452 | 8 | 0.080624 | 0.009728 | 0.012233 | 0.012233 |
| settings.get | 1141 | 8 | 0.078317 | 0.008356 | 0.016451 | 0.016451 |
| workspace.get | 1338 | 8 | 0.073139 | 0.008456 | 0.011883 | 0.011883 |
| workspace.list | 1321 | 8 | 0.078348 | 0.008516 | 0.015930 | 0.015930 |

The clone table measures the cached base `serde_json::Value::clone()` and
records its serialized size; serialization is outside the clone interval.
`harnesses.list` and `bridge.update_status` use their own immutable service
snapshots, so they are present in the carrier comparison and absent from this
root-backed clone table.

The fixture has one project, one managed workspace, eight tasks, eight retained
user messages, eight operation receipts and 64 activity events. Producer intervals use warmed observations;
they exclude startup filesystem/SQL/history warming and the separately prepared
transactional task-observation candidate. Base clones precede service enrichment
such as workspace history attachment, catalog overlays, typed validation and
serialization. Thus the carrier table gives complete handler latency for these
requests, while the direct table isolates selected producer/root/clone costs.
Carrier totals do not include a network round trip. This sample does not measure
large-project memory growth, refresh throughput, startup cost or optimal worker
counts. The raw artifacts are temporary local files and need exporting if they
must outlive this workspace. A portable bundle is available at
`/tmp/build-686-step3-evidence.tar.gz`; it includes both raw captures, the red
outputs, source provenance, archived test overlays and the comparison helper.

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
| Task observation rendering uses captured identities and paths | `tracker_observation_purity::{task_observation_rendering_uses_captured_paths_for_unnamed_live_actors,task_observation_rendering_preserves_retired_names_without_the_writer,dependency_edit_captures_a_new_actor_before_publishing_its_reply}` | Zero path comparisons; a held Store writer cannot block retired-name rendering. Real MCP link/unlink edits capture each new actor before reply and retain that identity in SQL. |
| Older observations cannot overwrite completed Git/filesystem/dependency changes | `facts::observations::tests::{a_completed_same_branch_commit_rejects_an_older_probe,a_completed_file_mutation_rejects_an_older_root_listing,mutating_a_workspace_dependency_rejects_an_older_summary_probe}` | Paused real probes are rejected after newer canonical completions. |
| Task board rows publish from their committed transaction, retaining unrelated rows and dependency digests | `tracker_finished::{failed_watched_task_observation_keeps_the_published_rows,a_task_write_publishes_its_owned_row_without_rereading_other_tasks,a_task_write_publishes_dependency_digests_with_its_own_row,task_creation_rolls_back_when_its_watched_row_cannot_be_prepared}` | Failed observations preserve prior rows; successful writes prepare affected task/dependency projections transactionally, and a failed candidate cannot leave a committed task without its board row. |
| Git checkout publishes observed branch before reply, including detached fallback | `runtime::snapshot_publication::successful_checkout_reply_publishes_branch_to_board_and_workspace_snapshots`; `detached_checkout_publishes_the_recorded_branch_fallback` | Real checkout status and repository HEAD form the oracle; captured board/workspace snapshots must advance while detached HEAD retains the recorded branch fallback. |
| Best-effort activity keeps live observation after persistence failure | `transactions::failed_user_activity_persistence_keeps_its_best_effort_observation` | Existing intentional live-versus-durable semantics. |
| All migrated snapshot verbs complete with app held | `concurrency_timing::snapshot_reads_complete_while_app_held_write_is_gated` | Real encrypted intake; zero app wait/hold; nine verbs, eight computations each. |
| All migrated snapshot verbs complete with ordinary Git worker held | `concurrency_timing::snapshot_reads_complete_while_off_lock_git_worker_is_gated` | One ordinary worker, gated Git work; real intake bypass. |
| Isolated snapshot costs | `concurrency_timing::isolated_snapshot_reads_publish_a_file_backed_baseline` | Same fixture, serialized requests; report total and queue p95 together. |
| Catalog/update snapshots do not wait for service mutation locks | `harness::inventory::tests::inventory_snapshot_does_not_wait_for_observation_lock`; `update::service::tests::update_status_does_not_wait_for_subscription_lock` | Real service snapshots retain their values while the separate observation/subscription mutexes are held. |
| Facts finish while app/writer held | `facts::snapshot_tests::nonempty_push_facts_finish_with_application_and_store_writer_held` | File-backed committed thread/state/terminal/project facts complete while both app and writer transaction are held. |
| First publication retains canonical rows; no request maintenance | `runtime::snapshot_publication::first_snapshot_keeps_seeded_workspace_rows_and_performs_no_path_lookups`; projection/parity tests | First reads retain all seeded rows at 4/16 real workspace sizes, with zero path comparisons; existing #672 tests retain 16/64-owner bounds. |
| Producer field coverage and #672 bounds | `worktree_roots_resolves_workspace_ownership_without_cubic_scans`; `thread_push_facts_resolve_ownership_once_per_entity`; publisher tests | Existing 16/64-owner roots bounds and 32-entity one/four-agent push-facts bounds remain; integration coverage exercises lifecycle/pump/review publication, without exhaustively crossing every producer and failure type. |
| Subscribe, write, post-ack read and push | `runtime::snapshot_publication::subscribed_workspace_write_is_visible_to_snapshot_and_next_push`; settings/compaction tests | Subscribe, acknowledge rename, read through a previously captured immutable service, then flush and compare the pushed name/project. |
| Snapshot/control progress under SQL saturation; no capacity refusal | `carrier::snapshot_tests::snapshots_bypass_saturated_write_and_sql_admission` | All nine real snapshot handlers answer with app held and both worker lanes saturated. Missing owner/update fixtures retain normal not-found/unavailable errors, without capacity refusals. |
| Agent removal failure separates actual retirement from committed roster | `transactions::failed_agent_remove_keeps_committed_roster_and_cursors_but_retires_delivery`; `tracker_identity_removal::removed_live_agent_invalidates_tasks_even_if_saving_roster_fails` | Required independently of generic finisher rollback. |

The final full Cargo test gate passed at the measured source, as did formatting
and both Clippy toolchains. This matrix names the specific Step 3 contracts
covered by that suite; the timing capture alone establishes neither failure
isolation nor the absence of every possible SQL/filesystem call. Held-writer,
path-comparison, immutable-service and saturation regressions exercise those
boundaries independently. The nine migrated verbs are the complete timing set;
`term.list` remains outside this Step 3 migration.

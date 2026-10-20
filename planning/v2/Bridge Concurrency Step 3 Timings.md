# Bridge concurrency step 3 evidence

Task #686; 2026-10-19. This report records deterministic temporary-fixture
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

## Step 3 acceptance matrix

| Contract | Evidence to run | Scope and remaining gap |
| --- | --- | --- |
| Disjoint immutable deltas merge; previously loaded roots remain immutable | `read_view::tests::disjoint_deltas_merge_and_old_root_remains_immutable`; `concurrent_delta_publication_retains_every_disjoint_owner` | Atomic chunk publication and concurrent owner merge. |
| Stale refresh cannot resurrect deletion; multi-owner candidate is atomic | `read_view::tests::deletion_tombstone_rejects_stale_refresh_and_multi_owner_delta_is_atomic` | Tombstone/version primitive. Lifecycle publisher coverage requires separate tests. |
| Plan failed save survives unrelated success and same-owner retry | `transactions::failed_plan_save_cannot_leak_through_an_unrelated_success_or_same_owner_retry` | Canonical state, durable SQL and the published agent chunk after unrelated success; the separate refreshed-push test pins retained thread content. |
| Run failed save survives unrelated success and same-owner retry | `transactions::failed_run_save_cannot_leak_through_an_unrelated_success_or_same_owner_retry` | Canonical state, durable SQL and the published agent chunk after unrelated success; the separate refreshed-push test pins retained thread content. |
| Failed save emits no change/news | `transactions::failed_finishers_emit_no_change_or_news` | No immediate failed event; later unrelated success explicitly refreshes both owners and rejects the failed words in pushed content. |
| Best-effort activity keeps live observation after persistence failure | `transactions::failed_user_activity_persistence_keeps_its_best_effort_observation` | Existing intentional live-versus-durable semantics. |
| All migrated snapshot verbs complete with app held | `concurrency_timing::snapshot_reads_complete_while_app_held_write_is_gated` | Real encrypted intake; zero app wait/hold; nine verbs, eight computations each. |
| All migrated snapshot verbs complete with ordinary Git worker held | `concurrency_timing::snapshot_reads_complete_while_off_lock_git_worker_is_gated` | One ordinary worker, gated Git work; real intake bypass. |
| Isolated snapshot costs | `concurrency_timing::isolated_snapshot_reads_publish_a_file_backed_baseline` | Same fixture, serialized requests; report total and queue p95 together. |
| Facts finish while app/writer held | `facts::snapshot_tests::push_facts_finish_while_application_state_is_locked` | Held app is explicit; a held Store writer case remains a separate integration concern. |
| First publication retains canonical rows; no request maintenance | `snapshot_projection::tests::first_projection_contains_authoritative_lists_and_settings`; SnapshotService parity tests | Must include parked projects, workspace adoption, unread/root/compaction and settings observations. |
| Producer field coverage and #672 bounds | `worktree_roots_resolves_workspace_ownership_without_cubic_scans`; `thread_push_facts_resolve_ownership_once_per_entity`; publisher tests | Audit pump/idle/review/lifecycle completions before enabling routes. |
| Subscribe, write, post-ack read and push | `runtime::snapshot_publication` settings/compaction tests plus existing push integrations | Settings and compaction pin the published root after success; subscription/push round trips cover coalesced facts. |
| Snapshot/control progress under SQL saturation; no capacity refusal | `carrier::snapshot_tests::snapshots_bypass_saturated_write_and_sql_admission` plus actual-service held-app/worker timing tests | Saturation injects the snapshot callback for all nine names; actual-service timing separately proves snapshot reads. It is not one combined actual-service SQL-saturation test. |
| Agent removal failure separates actual retirement from committed roster | `transactions::failed_agent_remove_keeps_committed_roster_and_cursors_but_retires_delivery`; `tracker_identity_removal::removed_live_agent_invalidates_tasks_even_if_saving_roster_fails` | Required independently of generic finisher rollback. |

The owning agents validate the wider suite; this matrix names concrete required
boundaries rather than treating a green timing sample as complete correctness
coverage.

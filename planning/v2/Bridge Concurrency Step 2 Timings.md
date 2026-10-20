# Step 2 concurrency timings

Task #685, 2026-10-20. These are temporary file-backed Store fixtures through real encrypted carrier intake, not live-daemon or production-workload measurements. The six migrating verbs each complete eight distinct requests per scenario; the history fixture has eight messages, 64 activity items and eight stored operation receipts. Requests differ so queued folding cannot reduce the computation counts.

## Before provenance

Production source: `502cfa9d3087ce72511ca0b26370c3436480b419`, extracted by `git archive` into `/tmp/build-685-before-source.i4UwKX`. The only source overlay is `bridge/src/app/tests/runtime/concurrency_timing.rs` from `4e2ee700a1afa4ca6571f9c825eb88ac203b7274`. That test-only overlay adds six-verb coverage and metadata fallback; production routing remains the original shared worker/app-lock path. No checkout or live daemon was changed for this capture.

The source archive has no Git repository, so the emitter correctly records `git_revision: "unknown"`. The archive revision above records the source provenance; emitted JSON and timing values were not edited. The emitter records `rustc 1.98.1 (48a229cea 2026-09-01)` and crate version `0.2.12`.

Isolated calls run sequentially with two dispatch workers. The app-held scenario queues all 48 reads behind a synthetic write holding the app mutex for at least 75 ms, with two dispatch workers. The Git-held scenario queues all 48 reads behind one off-lock worker gated for at least 75 ms, with one dispatch worker. The queue budget is 128 so the original dispatcher admits the entire workload. Histogram percentiles are bucket estimates; sums and maxima retain microsecond resolution. Queue time includes intake and scheduling; lock wait/hold measure only instrumented app-lock use, not separate SQLite costs.

## Commands and exit codes

Run from the task repository root, with `CARGO_TARGET_DIR="$PWD/bridge/target"` to share only Cargo's build cache:

```sh
CARGO_TARGET_DIR="$PWD/bridge/target" nice -n 10 cargo test --manifest-path /tmp/build-685-before-source.i4UwKX/bridge/Cargo.toml --lib baseline_metadata -- --nocapture --test-threads=1
BUILD_CONCURRENCY_BASELINE_DIR=/tmp/build-685-before-six.VvBGY5 CARGO_TARGET_DIR="$PWD/bridge/target" nice -n 10 cargo test --manifest-path /tmp/build-685-before-source.i4UwKX/bridge/Cargo.toml --lib concurrency_timing -- --nocapture --test-threads=1
```

The first command ran with the regression tests and unchanged failure behavior: exit **101**, all three tests failed on missing repository, missing compiler and unsuccessful compiler execution. Log: `/tmp/build-685-metadata-red-archive.log`. No HOME/PATH or protected environment overrides were used; failures use injected temporary paths and the Cargo test binary's unsupported `--version` flag.

The second command ran after the fallback: exit **0**, seven tests passed, including all three regressions, artifact serialization and all three six-verb timing scenarios. Log: `/tmp/build-685-before-six.VvBGY5/gate.log`. Raw artifacts: `/tmp/build-685-before-six.VvBGY5/bridge-concurrency-baseline.jsonl` and `/tmp/build-685-before-six.VvBGY5/bridge-concurrency-baseline.md`.

## Before results

All durations are milliseconds. Each phase count is eight.

| Scenario | Verb | Phase | Count | Sum | p50 | p95 | Maximum |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: |
| isolated | tasks.list | queued | 8 | 2.706 | 0.422 | 0.422 | 0.422 |
| isolated | tasks.list | lock_wait | 8 | 0.003 | 0.002 | 0.002 | 0.002 |
| isolated | tasks.list | held | 8 | 44.567 | 5.0 | 11.984 | 11.984 |
| isolated | tasks.get | queued | 8 | 2.847 | 0.434 | 0.434 | 0.434 |
| isolated | tasks.get | lock_wait | 8 | 0.002 | 0.001 | 0.001 | 0.001 |
| isolated | tasks.get | held | 8 | 12.135 | 2.065 | 2.065 | 2.065 |
| isolated | tasks.waiting_on.blocks | queued | 8 | 2.892 | 0.477 | 0.477 | 0.477 |
| isolated | tasks.waiting_on.blocks | lock_wait | 8 | 0.001 | 0.001 | 0.001 | 0.001 |
| isolated | tasks.waiting_on.blocks | held | 8 | 5.795 | 1.0 | 1.051 | 1.051 |
| isolated | thread.page | queued | 8 | 3.128 | 0.453 | 0.453 | 0.453 |
| isolated | thread.page | lock_wait | 8 | 0.002 | 0.002 | 0.002 | 0.002 |
| isolated | thread.page | held | 8 | 12.485 | 2.5 | 2.79 | 2.79 |
| isolated | thread.activity | queued | 8 | 3.685 | 0.5 | 0.692 | 0.692 |
| isolated | thread.activity | lock_wait | 8 | 0.002 | 0.001 | 0.001 | 0.001 |
| isolated | thread.activity | held | 8 | 3.227 | 0.5 | 0.55 | 0.55 |
| isolated | thread.operation | queued | 8 | 3.202 | 0.5 | 0.607 | 0.607 |
| isolated | thread.operation | lock_wait | 8 | 0.0 | 0.0 | 0.0 | 0.0 |
| isolated | thread.operation | held | 8 | 2.946 | 0.441 | 0.441 | 0.441 |
| app_held_write | tasks.list | queued | 8 | 904.023 | 159.668 | 159.668 | 159.668 |
| app_held_write | tasks.list | lock_wait | 8 | 92.666 | 0.1 | 92.666 | 92.666 |
| app_held_write | tasks.list | held | 8 | 43.303 | 7.285 | 7.285 | 7.285 |
| app_held_write | tasks.get | queued | 8 | 918.206 | 162.565 | 162.565 | 162.565 |
| app_held_write | tasks.get | lock_wait | 8 | 119.208 | 5.0 | 95.058 | 95.058 |
| app_held_write | tasks.get | held | 8 | 13.989 | 2.5 | 2.518 | 2.518 |
| app_held_write | tasks.waiting_on.blocks | queued | 8 | 1048.769 | 167.933 | 167.933 | 167.933 |
| app_held_write | tasks.waiting_on.blocks | lock_wait | 8 | 2.741 | 0.1 | 0.897 | 0.897 |
| app_held_write | tasks.waiting_on.blocks | held | 8 | 7.724 | 1.0 | 2.122 | 2.122 |
| app_held_write | thread.page | queued | 8 | 1054.595 | 168.005 | 168.005 | 168.005 |
| app_held_write | thread.page | lock_wait | 8 | 2.747 | 0.25 | 1.385 | 1.385 |
| app_held_write | thread.page | held | 8 | 12.192 | 2.067 | 2.067 | 2.067 |
| app_held_write | thread.activity | queued | 8 | 1057.039 | 168.509 | 168.509 | 168.509 |
| app_held_write | thread.activity | lock_wait | 8 | 9.534 | 1.726 | 1.726 | 1.726 |
| app_held_write | thread.activity | held | 8 | 2.721 | 0.5 | 0.572 | 0.572 |
| app_held_write | thread.operation | queued | 8 | 1071.286 | 170.56 | 170.56 | 170.56 |
| app_held_write | thread.operation | lock_wait | 8 | 0.003 | 0.001 | 0.001 | 0.001 |
| app_held_write | thread.operation | held | 8 | 2.926 | 0.5 | 0.578 | 0.578 |
| off_lock_git_worker | tasks.list | queued | 8 | 1116.614 | 194.534 | 194.534 | 194.534 |
| off_lock_git_worker | tasks.list | lock_wait | 8 | 0.0 | 0.0 | 0.0 | 0.0 |
| off_lock_git_worker | tasks.list | held | 8 | 43.036 | 8.041 | 8.041 | 8.041 |
| off_lock_git_worker | tasks.get | queued | 8 | 1171.934 | 200.0 | 204.749 | 204.749 |
| off_lock_git_worker | tasks.get | lock_wait | 8 | 0.004 | 0.001 | 0.001 | 0.001 |
| off_lock_git_worker | tasks.get | held | 8 | 12.36 | 1.874 | 1.874 | 1.874 |
| off_lock_git_worker | tasks.waiting_on.blocks | queued | 8 | 1188.004 | 200.0 | 206.16 | 206.16 |
| off_lock_git_worker | tasks.waiting_on.blocks | lock_wait | 8 | 0.003 | 0.001 | 0.001 | 0.001 |
| off_lock_git_worker | tasks.waiting_on.blocks | held | 8 | 4.695 | 0.801 | 0.801 | 0.801 |
| off_lock_git_worker | thread.page | queued | 8 | 1192.619 | 200.0 | 206.418 | 206.418 |
| off_lock_git_worker | thread.page | lock_wait | 8 | 0.001 | 0.001 | 0.001 | 0.001 |
| off_lock_git_worker | thread.page | held | 8 | 10.598 | 1.937 | 1.937 | 1.937 |
| off_lock_git_worker | thread.activity | queued | 8 | 1226.149 | 200.0 | 212.064 | 212.064 |
| off_lock_git_worker | thread.activity | lock_wait | 8 | 0.005 | 0.002 | 0.002 | 0.002 |
| off_lock_git_worker | thread.activity | held | 8 | 2.525 | 0.45 | 0.45 | 0.45 |
| off_lock_git_worker | thread.operation | queued | 8 | 1230.094 | 200.0 | 212.727 | 212.727 |
| off_lock_git_worker | thread.operation | lock_wait | 8 | 0.001 | 0.001 | 0.001 | 0.001 |
| off_lock_git_worker | thread.operation | held | 8 | 2.607 | 0.429 | 0.429 | 0.429 |

## After provenance and commands

Production and fixture source: `57647a082543931cf7b822465c5c4109d943c0d6`, on the task branch with a clean worktree. The recorded revision before and after both captures is identical. The after fixture routes the six verbs through the production `ReadService` and the carrier SQL lane; the separate ordinary handler remains available for the gated Git request. Eight SQL workers are recorded separately from the ordinary dispatch worker counts, which remain two for isolated/app-held scenarios and one for Git-held. The before source had no SQL lane and no `sql_workers` field.

The seed data, six verbs and eight distinct requests per verb are unchanged. In each after contention scenario, the fixture sends all 48 reads, waits at least 75 ms, collects all 48 successful SQL replies while the synthetic app mutex or Git worker gate is still held, asserts the gate is held, then releases it. The Git reply is collected separately after release. The after fixture therefore checks independence from either gate directly; it does not infer independence from a smaller timing alone. The timing aggregator counts every computation, including zero-duration phases, so each lock-wait/hold count remains eight while all its duration fields are zero.

Run sequentially from `bridge/` after the full bridge and SPA gates finished:

```sh
BUILD_CONCURRENCY_BASELINE_DIR=/tmp/build-685-after.PWpCC9 nice -n 10 cargo test --lib concurrency_timing -- --nocapture --test-threads=1
nice -n 10 cargo test --lib readonly_unbounded_task_list_reports_checkout_and_transaction_duration -- --nocapture --test-threads=1
```

The first command exited **0**: eight tests passed, including all three contention samples, the metadata regressions and the complete 501-task query measurement. Log: `/tmp/build-685-after.PWpCC9/capture.log`; exit file: `/tmp/build-685-after.PWpCC9/capture.exit`. The second command exited **0**: one Store batch measurement passed. Log: `/tmp/build-685-after.PWpCC9/store-batch.log`; exit file: `/tmp/build-685-after.PWpCC9/store-batch.exit`. The captures used `rustc 1.98.1 (48a229cea 2026-09-01)` and crate version `0.2.12`.

Raw six-verb artifacts: `/tmp/build-685-after.PWpCC9/bridge-concurrency-baseline.jsonl` and `/tmp/build-685-after.PWpCC9/bridge-concurrency-baseline.md`. Source checks: `/tmp/build-685-after.PWpCC9/revision.before` and `/tmp/build-685-after.PWpCC9/revision.after`. The emitter serialization test also prints a deliberately constructed metric to the test log; that record is in its own temporary fixture and is excluded from the persisted three-scenario JSONL and these tables.

## Before/after comparison

All values are milliseconds. Each row represents eight computations. App-held queue p95 changes from 159.668–170.560 ms to 4.542–5.354 ms; Git-held queue p95 changes from 194.534–212.727 ms to 5.638–6.492 ms. Isolated queue p95 ranges from 0.422–0.692 ms before and 0.464–1.174 ms after. This sample demonstrates removal of app/Git contention; it does not show a uniform isolated-latency improvement.

| Scenario | Verb | Queue sum before | Queue sum after | Queue p95 before | Queue p95 after |
| --- | --- | ---: | ---: | ---: | ---: |
| isolated | tasks.list | 2.706 | 4.109 | 0.422 | 1.174 |
| isolated | tasks.get | 2.847 | 3.186 | 0.434 | 0.479 |
| isolated | tasks.waiting_on.blocks | 2.892 | 3.075 | 0.477 | 0.464 |
| isolated | thread.page | 3.128 | 2.848 | 0.453 | 0.504 |
| isolated | thread.activity | 3.685 | 4.152 | 0.692 | 0.637 |
| isolated | thread.operation | 3.202 | 3.702 | 0.607 | 0.674 |
| app_held_write | tasks.list | 904.023 | 25.486 | 159.668 | 4.723 |
| app_held_write | tasks.get | 918.206 | 24.633 | 162.565 | 4.542 |
| app_held_write | tasks.waiting_on.blocks | 1048.769 | 30.832 | 167.933 | 5.354 |
| app_held_write | thread.page | 1054.595 | 30.832 | 168.005 | 5.103 |
| app_held_write | thread.activity | 1057.039 | 32.024 | 168.509 | 5.091 |
| app_held_write | thread.operation | 1071.286 | 30.571 | 170.560 | 5.041 |
| off_lock_git_worker | tasks.list | 1116.614 | 31.794 | 194.534 | 5.638 |
| off_lock_git_worker | tasks.get | 1171.934 | 32.480 | 204.749 | 6.492 |
| off_lock_git_worker | tasks.waiting_on.blocks | 1188.004 | 37.825 | 206.160 | 6.319 |
| off_lock_git_worker | thread.page | 1192.619 | 37.845 | 206.418 | 6.209 |
| off_lock_git_worker | thread.activity | 1226.149 | 37.101 | 212.064 | 5.887 |
| off_lock_git_worker | thread.operation | 1230.094 | 36.532 | 212.727 | 5.779 |

The following sums include only the six moved verbs, 48 computations per scenario, excluding the synthetic write, Git request and delivery-service setup. They add observed durations across requests; they are not elapsed wall-clock durations or aggregated percentiles.

| Scenario | Queue sum before | Queue sum after | Lock-wait sum before | Lock-wait sum after | Held sum before | Held sum after |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| isolated | 18.460 | 21.072 | 0.010 | 0.000 | 81.155 | 0.000 |
| app_held_write | 6053.918 | 174.378 | 226.899 | 0.000 | 82.855 | 0.000 |
| off_lock_git_worker | 7125.414 | 213.577 | 0.014 | 0.000 | 75.821 | 0.000 |

## After results

All durations are milliseconds. Each phase count and each verb's served count is eight. For each verb in every scenario, app-lock wait and hold sum/p50/p95/maximum are all zero.

| Scenario | Verb | Phase | Count | Sum | p50 | p95 | Maximum |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: |
| isolated | tasks.list | queued | 8 | 4.109 | 0.5 | 1.174 | 1.174 |
| isolated | tasks.list | lock_wait | 8 | 0.0 | 0.0 | 0.0 | 0.0 |
| isolated | tasks.list | held | 8 | 0.0 | 0.0 | 0.0 | 0.0 |
| isolated | tasks.get | queued | 8 | 3.186 | 0.479 | 0.479 | 0.479 |
| isolated | tasks.get | lock_wait | 8 | 0.0 | 0.0 | 0.0 | 0.0 |
| isolated | tasks.get | held | 8 | 0.0 | 0.0 | 0.0 | 0.0 |
| isolated | tasks.waiting_on.blocks | queued | 8 | 3.075 | 0.464 | 0.464 | 0.464 |
| isolated | tasks.waiting_on.blocks | lock_wait | 8 | 0.0 | 0.0 | 0.0 | 0.0 |
| isolated | tasks.waiting_on.blocks | held | 8 | 0.0 | 0.0 | 0.0 | 0.0 |
| isolated | thread.page | queued | 8 | 2.848 | 0.5 | 0.504 | 0.504 |
| isolated | thread.page | lock_wait | 8 | 0.0 | 0.0 | 0.0 | 0.0 |
| isolated | thread.page | held | 8 | 0.0 | 0.0 | 0.0 | 0.0 |
| isolated | thread.activity | queued | 8 | 4.152 | 0.637 | 0.637 | 0.637 |
| isolated | thread.activity | lock_wait | 8 | 0.0 | 0.0 | 0.0 | 0.0 |
| isolated | thread.activity | held | 8 | 0.0 | 0.0 | 0.0 | 0.0 |
| isolated | thread.operation | queued | 8 | 3.702 | 0.5 | 0.674 | 0.674 |
| isolated | thread.operation | lock_wait | 8 | 0.0 | 0.0 | 0.0 | 0.0 |
| isolated | thread.operation | held | 8 | 0.0 | 0.0 | 0.0 | 0.0 |
| app_held_write | tasks.list | queued | 8 | 25.486 | 4.723 | 4.723 | 4.723 |
| app_held_write | tasks.list | lock_wait | 8 | 0.0 | 0.0 | 0.0 | 0.0 |
| app_held_write | tasks.list | held | 8 | 0.0 | 0.0 | 0.0 | 0.0 |
| app_held_write | tasks.get | queued | 8 | 24.633 | 4.542 | 4.542 | 4.542 |
| app_held_write | tasks.get | lock_wait | 8 | 0.0 | 0.0 | 0.0 | 0.0 |
| app_held_write | tasks.get | held | 8 | 0.0 | 0.0 | 0.0 | 0.0 |
| app_held_write | tasks.waiting_on.blocks | queued | 8 | 30.832 | 5.0 | 5.354 | 5.354 |
| app_held_write | tasks.waiting_on.blocks | lock_wait | 8 | 0.0 | 0.0 | 0.0 | 0.0 |
| app_held_write | tasks.waiting_on.blocks | held | 8 | 0.0 | 0.0 | 0.0 | 0.0 |
| app_held_write | thread.page | queued | 8 | 30.832 | 5.0 | 5.103 | 5.103 |
| app_held_write | thread.page | lock_wait | 8 | 0.0 | 0.0 | 0.0 | 0.0 |
| app_held_write | thread.page | held | 8 | 0.0 | 0.0 | 0.0 | 0.0 |
| app_held_write | thread.activity | queued | 8 | 32.024 | 5.0 | 5.091 | 5.091 |
| app_held_write | thread.activity | lock_wait | 8 | 0.0 | 0.0 | 0.0 | 0.0 |
| app_held_write | thread.activity | held | 8 | 0.0 | 0.0 | 0.0 | 0.0 |
| app_held_write | thread.operation | queued | 8 | 30.571 | 5.0 | 5.041 | 5.041 |
| app_held_write | thread.operation | lock_wait | 8 | 0.0 | 0.0 | 0.0 | 0.0 |
| app_held_write | thread.operation | held | 8 | 0.0 | 0.0 | 0.0 | 0.0 |
| off_lock_git_worker | tasks.list | queued | 8 | 31.794 | 5.0 | 5.638 | 5.638 |
| off_lock_git_worker | tasks.list | lock_wait | 8 | 0.0 | 0.0 | 0.0 | 0.0 |
| off_lock_git_worker | tasks.list | held | 8 | 0.0 | 0.0 | 0.0 | 0.0 |
| off_lock_git_worker | tasks.get | queued | 8 | 32.48 | 5.0 | 6.492 | 6.492 |
| off_lock_git_worker | tasks.get | lock_wait | 8 | 0.0 | 0.0 | 0.0 | 0.0 |
| off_lock_git_worker | tasks.get | held | 8 | 0.0 | 0.0 | 0.0 | 0.0 |
| off_lock_git_worker | tasks.waiting_on.blocks | queued | 8 | 37.825 | 6.319 | 6.319 | 6.319 |
| off_lock_git_worker | tasks.waiting_on.blocks | lock_wait | 8 | 0.0 | 0.0 | 0.0 | 0.0 |
| off_lock_git_worker | tasks.waiting_on.blocks | held | 8 | 0.0 | 0.0 | 0.0 | 0.0 |
| off_lock_git_worker | thread.page | queued | 8 | 37.845 | 6.209 | 6.209 | 6.209 |
| off_lock_git_worker | thread.page | lock_wait | 8 | 0.0 | 0.0 | 0.0 | 0.0 |
| off_lock_git_worker | thread.page | held | 8 | 0.0 | 0.0 | 0.0 | 0.0 |
| off_lock_git_worker | thread.activity | queued | 8 | 37.101 | 5.887 | 5.887 | 5.887 |
| off_lock_git_worker | thread.activity | lock_wait | 8 | 0.0 | 0.0 | 0.0 | 0.0 |
| off_lock_git_worker | thread.activity | held | 8 | 0.0 | 0.0 | 0.0 | 0.0 |
| off_lock_git_worker | thread.operation | queued | 8 | 36.532 | 5.0 | 5.779 | 5.779 |
| off_lock_git_worker | thread.operation | lock_wait | 8 | 0.0 | 0.0 | 0.0 | 0.0 |
| off_lock_git_worker | thread.operation | held | 8 | 0.0 | 0.0 | 0.0 | 0.0 |

## Unbounded 501-task transaction sizing

Both measurements use the same after source and return all 501 tasks. The complete production-query fixture omits the wire limit and verifies that no next cursor is returned; the partial Store fixture uses the default unbounded filter. They run separately on their own temporary file-backed fixtures. Checkout is measured independently from the read transaction.

| Measurement | Reader checkout | Read transaction | Scope |
| --- | ---: | ---: | --- |
| Complete production `TaskReadContext::tasks_list` query | 13 µs (0.013 ms) | 283,559 µs (283.559 ms) | Scope validation and the complete production list query, including row projections and per-task review, dependency, blocks and delivery reads |
| Partial Store batch only | 52 µs (0.052 ms) | 112,801 µs (112.801 ms) | `list_tracker_tasks`, batch timelines and batch waiting-on summaries; excludes production row projections and the other per-task reads |

The complete-query value is emitted as the `bridge_sql_read_sizing` JSON record in `capture.log`, with `wire_limit: null`, `task_count: 501` and the source/toolchain metadata. The partial Store value is the `sql_read_sizing` line in `store-batch.log`. Both tests assert that the writer's total change count is unchanged. The complete fixture normalizes the request before timing and validates the context fence and typed result after timing; these checks, carrier dispatch, wire serialization and transport are outside the measured transaction. This is a full query transaction duration, not a full `tasks.list` RPC latency. The partial measurement is retained as separate evidence and is not a substitute for it.

## Interpretation limits

These are single captures with eight samples per verb/scenario in small controlled fixtures. Histogram percentiles are approximate bucket values. The app/Git gates are synthetic and held for at least 75 ms, not an identical fixed elapsed duration between runs. The before archive uses the legacy handler and the after source uses the production SQL read service; their fixture changes are described above. No live daemon, production database, paired client or user browser was involved.

The sizing project contains 501 simple unassigned tasks; its complete query executes the real per-task lookup paths, including empty review and delivery lookups. More populated relationships, warm/cold cache differences, simultaneous writes and larger histories may change the duration. The 283.559 ms transaction illustrates why long unbounded reads need a separate bounded pool and supports retaining eight SQL workers as an initial size. A single uncongested checkout and transaction does not establish eight as optimal or measure pool saturation. No wire limit was introduced. Queue time still includes intake and scheduling; zero app-lock timing does not mean zero SQLite work, and these phase percentiles cannot be subtracted to infer SQLite cost.

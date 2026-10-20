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

The isolated **total p95 regressed in this first sample**: `thread.page` 4.077→6.547 ms, `thread.activity` 1.171→3.037 ms, `thread.operation` 1.036→2.010 ms and `tasks.list` 12.856→13.745 ms. These are measured intake-to-handler-completion totals, including queueing; they exclude outbound wire serialization and client receipt. This first capture does not separately measure checkout or query phases, so it cannot attribute those changes to pool waiting, scheduling or SQL. The direct diagnostic capture below supplies those missing boundaries. Neither percentile subtraction nor the app-lock-held total establishes SQLite cost.

| Scenario | Verb | Total p50 before | Total p50 after | Queue p95 before | Total p95 before | Queue p95 after | Total p95 after |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| isolated | tasks.list | 5.000 | 5.000 | 0.422 | 12.856 | 1.174 | 13.745 |
| isolated | tasks.get | 2.500 | 2.500 | 0.434 | 2.586 | 0.479 | 2.686 |
| isolated | tasks.waiting_on.blocks | 1.585 | 1.000 | 0.477 | 1.585 | 0.464 | 1.466 |
| isolated | thread.page | 2.500 | 5.000 | 0.453 | 4.077 | 0.504 | 6.547 |
| isolated | thread.activity | 1.000 | 2.500 | 0.692 | 1.171 | 0.637 | 3.037 |
| isolated | thread.operation | 1.000 | 2.010 | 0.607 | 1.036 | 0.674 | 2.010 |
| app_held_write | tasks.list | 166.535 | 10.000 | 159.668 | 166.535 | 4.723 | 10.999 |
| app_held_write | tasks.get | 167.481 | 7.902 | 162.565 | 167.481 | 4.542 | 7.902 |
| app_held_write | tasks.waiting_on.blocks | 168.805 | 7.369 | 167.933 | 168.805 | 5.354 | 7.369 |
| app_held_write | thread.page | 170.524 | 10.000 | 168.005 | 170.524 | 5.103 | 10.718 |
| app_held_write | thread.activity | 170.169 | 7.163 | 168.509 | 170.169 | 5.091 | 7.163 |
| app_held_write | thread.operation | 170.944 | 6.994 | 170.560 | 170.944 | 5.041 | 6.994 |
| off_lock_git_worker | tasks.list | 200.000 | 10.000 | 194.534 | 202.852 | 5.638 | 14.670 |
| off_lock_git_worker | tasks.get | 200.000 | 10.000 | 204.749 | 205.942 | 6.492 | 12.053 |
| off_lock_git_worker | tasks.waiting_on.blocks | 200.000 | 9.277 | 206.160 | 206.530 | 6.319 | 9.277 |
| off_lock_git_worker | thread.page | 200.000 | 10.000 | 206.418 | 208.607 | 6.209 | 11.257 |
| off_lock_git_worker | thread.activity | 200.000 | 8.828 | 212.064 | 212.387 | 5.887 | 8.828 |
| off_lock_git_worker | thread.operation | 200.000 | 10.000 | 212.727 | 213.136 | 5.779 | 10.467 |

The following sums include only the six moved verbs, 48 computations per scenario, excluding the synthetic write, Git request and delivery-service setup. They add observed durations across requests; they are not elapsed wall-clock durations or aggregated percentiles. **The app-lock-held sums falling to zero mean that read work moved off the app lock; the work did not disappear.** Query execution in the SQL lane is outside that instrumented app-lock phase.

| Scenario | Queue sum before | Queue sum after | Lock-wait sum before | Lock-wait sum after | App-lock held sum before | App-lock held sum after |
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

## Review follow-up: direct SQL diagnostics

The original comparison above remains the first captured sample, with its isolated total regression reported explicitly. The follow-up adds test-only observations to the actual SQL lane and read transaction. They compile only under `cfg(test)`; production `bridge.stats`, logs and wire replies receive no new fields. The fixture now reports `sql_workers` through the actual carrier worker constant rather than a copied value. It preserves eight distinct computations per verb and the same seeds, scenarios, worker counts and gate assertions.

| Diagnostic phase | Measured boundary |
| --- | --- |
| `checkout` | Coordinator starts requesting a read connection until it owns that connection, including asynchronous permit wait and connection retrieval. The coordinator checks out before choosing work, so this duration is attributed to the computation it subsequently takes; earlier admission/backlog wait remains in `queued`. |
| `worker_handoff` | The selected computation is handed to the dedicated worker until that worker begins handling it. This measures the additional coordinator-to-thread hop, independently of checkout. |
| `service` | Entry to return of the actual `ReadService::call`, including normalization, context load, transaction, fence/result validation and connection return. It includes the nested transaction phases rather than adding to them. |
| `transaction_setup` | Entry to SQLite `Connection::transaction` until the transaction object is created. SQLite's deferred first read remains in query time. |
| `query` | The complete request closure passed to `ReadConnection::read`: scope validation and production query/projection work inside the transaction. |
| `transaction_finish` | The query closure returns until SQLite's read transaction commit completes. |

Each diagnostic reports count, sum, p50, p95 and maximum; JSON also retains the per-request phase values in `samples_ms`. Diagnostic quantiles use the nearest rank of the eight direct samples, while the existing total/queued/wait/hold fields retain the bridge's fixed histogram buckets. They should not be interpreted as identically quantized distributions. Observer callbacks and timestamp reads add test-only overhead; query/setup/finish intervals exclude their reporting callback, while service and the existing intake-to-handler totals include the applicable fixture reporting work. No percentile is subtracted from another to attribute a cause.

The TDD regression first required all six verbs to publish each diagnostic phase with eight samples. It failed as intended with exit **101**, missing the `tasks.list.checkout` count (`/tmp/build-685-sql-phases-red.log`). The first attempted green build exited **101** on an unrelated concurrently added carrier test's `HashSet` type inference (`/tmp/build-685-sql-phases-green.log`); that compile failure is not a diagnostic test result. Observer and fixture source was committed as `a2cabfc01fe0bcee3e2d9f892d2ed4bbcb24cd6d` before the diagnostic capture.

### Before the query-work refinement

The preliminary instrumented capture ran from `bridge/` with:

```sh
BUILD_CONCURRENCY_BASELINE_DIR=/tmp/build-685-sql-preopt.31xNzX nice -n 10 cargo test --lib concurrency_timing -- --nocapture --test-threads=1
```

Exit **0**, eight tests passed; the new diagnostic assertions are green for all six verbs in all three scenarios. Raw artifacts and test log are under `/tmp/build-685-sql-preopt.31xNzX/` (`bridge-concurrency-baseline.jsonl`, `bridge-concurrency-baseline.md`, `capture.log`, `capture.exit`). Before/after source checks are `revision.before` and `revision.after`; `worktree.before` records the state at invocation. The invocation began at `c4f8ace43562bc70ab397265d14be0b32a403cff`, containing the test hooks and new admission tests. SPA-only commit `e6aeddf3a14eab2aace62eb81f497ed59aa42e75` landed during compilation; all emitted records identify `e6aeddf3a14eab2aace62eb81f497ed59aa42e75`. Rust production read code was unchanged during this capture. The pending thread-read regression tests were present but their filter was not run by this command; no query optimization was applied yet.

These are directly measured isolated means, calculated as each phase sum divided by its eight observations, in milliseconds. `service` contains the transaction phases. It is not a seventh additive part.

| Verb | Checkout mean | Worker handoff mean | Setup mean | Query mean | Finish mean | Service mean |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| tasks.list | 0.0146 | 0.0482 | 0.0183 | 5.1811 | 0.0184 | 5.7722 |
| tasks.get | 0.0160 | 0.0604 | 0.0230 | 1.3051 | 0.0153 | 1.6247 |
| tasks.waiting_on.blocks | 0.0164 | 0.0443 | 0.0275 | 0.6846 | 0.0140 | 0.8310 |
| thread.page | 0.0129 | 0.0515 | 0.0192 | 2.9448 | 0.0191 | 4.2121 |
| thread.activity | 0.0173 | 0.0545 | 0.0268 | 1.5289 | 0.0212 | 1.8920 |
| thread.operation | 0.0160 | 0.0746 | 0.0253 | 1.2636 | 0.0172 | 1.4717 |

For the three history verbs, these observations place checkout, worker handoff, setup and finish individually at roughly 0.013–0.075 ms on average, while the query closure averages 1.264–2.945 ms. That measures where the new lane spends time in this capture: it does not establish that a pool wait or the extra thread hop caused the earlier isolated regression. The earlier sample lacks these probes, so its exact additional cost cannot be retrospectively assigned. Independent query-count/decoded-row regression tests investigate repeated agent-roster reads and unnecessary conversation-summary restoration; their deterministic work counts, followed by a capture with the same probes, can assess that refinement without subtracting percentiles. The complete unbounded 501-task query in this preliminary run measured 20 µs checkout and 346,142 µs transaction; it remains a single synthetic sizing sample.

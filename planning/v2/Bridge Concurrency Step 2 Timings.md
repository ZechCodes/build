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

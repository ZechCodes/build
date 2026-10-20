# Step 4 observation timings and acceptance

All 22 migrated observations complete while the app mutex remains held,
and every captured after app-wait/hold metric is zero. All **44 gated total-p95
comparisons improve**, and **43/44 gated queue-p95 comparisons improve**.
Every isolated verb has higher total and queue p95: **22/22 increases for each**.
This capture therefore records a contended-latency improvement with an
isolated-latency tradeoff; its eight samples per verb do not establish the
cause of the isolated increases. Observed isolated total-p95 ratios range
from approximately **2.07× to 9.31×**, a material review limitation.

Total-p95 ranges across the 22 verbs, in milliseconds:

| Scenario | Before range | After range | Total-p95 increases | Queue-p95 increases |
| --- | ---: | ---: | ---: | ---: |
| Isolated | 0.568–8.860 | 1.698–24.497 | 22/22 | 22/22 |
| App mutex held | 76.043–116.832 | 6.714–39.758 | 0/22 | 1/22 |
| Ordinary Git worker held | 112.351–151.403 | 5.450–33.504 | 0/22 | 0/22 |

Queue-p95 ranges across the same verbs, in milliseconds:

| Scenario | Before range | After range |
| --- | ---: | ---: |
| Isolated | 0.401–0.693 | 0.849–4.994 |
| App mutex held | 0.628–116.511 | 3.495–20.380 |
| Ordinary Git worker held | 111.504–151.084 | 2.430–15.318 |

All observations and every exact gated improvement appear in the complete
22-verb tables below. The one gated queue increase is `git.log` with the app
mutex held: **0.628 → 4.019 ms**; its total p95 improves.

Task #694, 2026-10-20. The fixture uses a temporary file-backed Store,
real encrypted carrier intake and `AppState::handler`. All 22 migrated verbs
read successful seeded resources. Each scenario runs eight distinct
computations per verb; each batch finishes before the next, preventing
queued folding from changing counts.

The fixture includes one registered project, a managed workspace, a real
external worktree, a stored single plan document, a simulated two-stage
legacy plan/run lineage, a saved review snapshot, bounded task and thread
attachment reads, and an injected local `gh` script returning one repository.
The archive has a nonempty legacy plan row. No daemon, installed bridge,
real GitHub credentials, browser or protected home is used.

Three scenarios share the same fixture and one ordinary dispatcher worker.
The after execution has eight I/O workers and eight SQL metadata/read workers:


- `io_isolated`: serialized reads with no held work.
- `io_app_held_write`: the app mutex is held for 75 ms per batch.
- `io_ordinary_git_worker`: a real temporary-repository `git.stage` writer
  reaches its existing off-lock gate and is held for 75 ms per batch.

The writer scenarios measure this controlled delay plus their normal work;
these are small fixture captures, not production latency or confidence bounds.
Total latency ends at handler completion and excludes a network round trip.
Step 1 clock histograms estimate p95; sums and maxima preserve microsecond
resolution. Queue includes intake/admission, execution scheduling and any
blocking-pool handoff. App wait and hold are separate aggregates; percentile
subtraction does not measure filesystem, Git or SQL cost.

## Provenance and reproduction

Before production source: `54e8e1d791c4f93d596a4aafb06487207084c27e`, extracted
with `git archive` into `/tmp/task694-before-source`. Only test fixture and
test registration files are overlaid. The archive has no `.git` directory,
so its emitted revision is `unknown`; `/tmp/task694-before/provenance.json`
records the source revision and hashes of every test overlay. The included
`test-overlay.patch` reconstructs those exact test changes on the baseline
archive; reverse-application verification passed. The Cargo-built
binary is copied and hashed before other builds can replace it.

The final before capture passed all three scenarios (exit 0). Raw data:
`/tmp/task694-before/capture/bridge-concurrency-baseline.jsonl`; raw stdout/stderr:
`/tmp/task694-before/capture.log`. The fixture SHA-256 is
`7f32757f11df4dbe3e5f03afe9c12cba5690a3190db938f4297b8bfeafb1935c`;
the before binary SHA-256 is
`6ee6458138ac64600e61519846d285426a5813473d486ce3659b7637db990a46`.

After measured source: `a94b008d2841c33e35eb98cdcd4a594f33b319e6`. The same fixture SHA-256 is recorded in both provenance files.

Both captures report `rustc 1.98.1 (48a229cea 2026-09-01)` and crate version
`0.2.12`; the Cargo-built test binaries ran under `nice -n 10` on the same
machine, with one ordinary dispatcher worker. The before capture occurred
during the development/build sequence; the after capture waited until the
parent's final test/Clippy/fmt wrappers had completed. Machine-wide CPU,
storage-cache state and other users' workloads were not measured or held
constant. The provenance supports the source/fixture/binary identities and
controlled gates, not a causal explanation for the isolated increases.

The final timing fixture is
`bridge/src/app/tests/runtime/concurrency_timing/io_observations.rs`.
It verifies every request id and successful typed response, including
nonempty documents/archive, review version 1, the immutable saved README,
both 12-byte attachment chunks and the injected repository. Fixture setup
explicitly refreshes observations and publishes its complete projection
before the final handler is captured. The helper's queued scripted stage turn
is cleared before sharing measured work.

To recapture the current code from `bridge/` into a fresh directory:

```bash
task694_capture_dir=$(mktemp -d /tmp/task694-recapture.XXXXXX)
BUILD_CONCURRENCY_BASELINE_DIR="$task694_capture_dir" nice -n 10 cargo test --lib io_observations -- --nocapture --test-threads=1
```

This runs the all-verb held-app regression plus the three timing scenarios.
Only the three scenarios emit records; ordinary runs without the output
variable write artifacts to temporary fixture paths.

## Paired p95 results

All durations below are milliseconds, copied from the raw histogram summaries without rounding. Each before/after cell is an ordered pair. Every served/queue/wait/hold count is **8 per verb per scenario**, or **528 observation computations per capture**.

### io_isolated

| Verb | Total p95 before / after | Queue p95 before / after | App wait p95 before / after | App hold p95 before / after |
| --- | ---: | ---: | ---: | ---: |
| `archived.list` | 0.786 / 3.555 | 0.436 / 2.64 | 0.001 / 0.0 | 0.327 / 0.0 |
| `fs.list` | 0.896 / 2.492 | 0.568 / 1.665 | 0.002 / 0.0 | 0.295 / 0.0 |
| `fs.read` | 0.82 / 1.698 | 0.574 / 0.849 | 0.001 / 0.0 | 0.363 / 0.0 |
| `fs.tree` | 0.822 / 1.856 | 0.533 / 0.926 | 0.001 / 0.0 | 0.265 / 0.0 |
| `git.changeset_diff` | 2.299 / 9.867 | 0.569 / 3.428 | 0.004 / 0.0 | 0.293 / 0.0 |
| `git.diff` | 2.736 / 10.961 | 0.401 / 4.335 | 0.004 / 0.0 | 0.248 / 0.0 |
| `git.log` | 1.296 / 6.573 | 0.456 / 4.994 | 0.001 / 0.0 | 0.207 / 0.0 |
| `git.refs` | 1.921 / 11.141 | 0.555 / 1.68 | 0.002 / 0.0 | 0.211 / 0.0 |
| `git.show` | 1.488 / 5.917 | 0.411 / 1.109 | 0.001 / 0.0 | 0.21 / 0.0 |
| `git.status` | 3.242 / 6.825 | 0.422 / 1.105 | 0.002 / 0.0 | 0.27 / 0.0 |
| `git.unpushed` | 2.921 / 11.854 | 0.558 / 1.017 | 0.001 / 0.0 | 0.227 / 0.0 |
| `github.repos` | 7.674 / 24.173 | 0.517 / 3.507 | 0.002 / 0.0 | 0.125 / 0.0 |
| `run.diff` | 2.902 / 10.573 | 0.52 / 1.495 | 0.001 / 0.0 | 0.196 / 0.0 |
| `task.doc` | 0.568 / 3.625 | 0.449 / 3.004 | 0.001 / 0.0 | 0.17 / 0.0 |
| `task.stage_diff` | 1.683 / 7.834 | 0.442 / 2.818 | 0.001 / 0.0 | 0.237 / 0.0 |
| `task.stage_doc` | 0.57 / 3.618 | 0.454 / 2.847 | 0.0 / 0.0 | 0.19 / 0.0 |
| `tasks.attachment` | 1.102 / 10.257 | 0.614 / 3.129 | 0.001 / 0.0 | 0.611 / 0.0 |
| `tasks.review.diff` | 1.932 / 8.595 | 0.568 / 2.146 | 0.001 / 0.0 | 0.943 / 0.0 |
| `tasks.review.get` | 8.86 / 24.497 | 0.544 / 3.708 | 0.002 / 0.0 | 1.022 / 0.0 |
| `thread.attachment` | 1.007 / 4.832 | 0.693 / 3.379 | 0.001 / 0.0 | 0.272 / 0.0 |
| `workspace.git_init_options` | 1.53 / 5.263 | 0.585 / 1.822 | 0.0 / 0.0 | 0.954 / 0.0 |
| `worktree.diff` | 2.159 / 7.933 | 0.619 / 2.896 | 0.001 / 0.0 | 0.248 / 0.0 |

Total p95 increased for 22/22 verbs; queue p95 increased for 22/22.
Total increases: `archived.list` (0.786 → 3.555), `fs.list` (0.896 → 2.492), `fs.read` (0.82 → 1.698), `fs.tree` (0.822 → 1.856), `git.changeset_diff` (2.299 → 9.867), `git.diff` (2.736 → 10.961), `git.log` (1.296 → 6.573), `git.refs` (1.921 → 11.141), `git.show` (1.488 → 5.917), `git.status` (3.242 → 6.825), `git.unpushed` (2.921 → 11.854), `github.repos` (7.674 → 24.173), `run.diff` (2.902 → 10.573), `task.doc` (0.568 → 3.625), `task.stage_diff` (1.683 → 7.834), `task.stage_doc` (0.57 → 3.618), `tasks.attachment` (1.102 → 10.257), `tasks.review.diff` (1.932 → 8.595), `tasks.review.get` (8.86 → 24.497), `thread.attachment` (1.007 → 4.832), `workspace.git_init_options` (1.53 → 5.263), `worktree.diff` (2.159 → 7.933).
Queue increases: `archived.list` (0.436 → 2.64), `fs.list` (0.568 → 1.665), `fs.read` (0.574 → 0.849), `fs.tree` (0.533 → 0.926), `git.changeset_diff` (0.569 → 3.428), `git.diff` (0.401 → 4.335), `git.log` (0.456 → 4.994), `git.refs` (0.555 → 1.68), `git.show` (0.411 → 1.109), `git.status` (0.422 → 1.105), `git.unpushed` (0.558 → 1.017), `github.repos` (0.517 → 3.507), `run.diff` (0.52 → 1.495), `task.doc` (0.449 → 3.004), `task.stage_diff` (0.442 → 2.818), `task.stage_doc` (0.454 → 2.847), `tasks.attachment` (0.614 → 3.129), `tasks.review.diff` (0.568 → 2.146), `tasks.review.get` (0.544 → 3.708), `thread.attachment` (0.693 → 3.379), `workspace.git_init_options` (0.585 → 1.822), `worktree.diff` (0.619 → 2.896).

### io_app_held_write

| Verb | Total p95 before / after | Queue p95 before / after | App wait p95 before / after | App hold p95 before / after |
| --- | ---: | ---: | ---: | ---: |
| `archived.list` | 116.832 / 17.767 | 116.511 / 17.135 | 0.001 / 0.0 | 0.315 / 0.0 |
| `fs.list` | 90.208 / 6.714 | 89.936 / 6.131 | 0.001 / 0.0 | 0.33 / 0.0 |
| `fs.read` | 90.89 / 12.592 | 90.631 / 11.885 | 0.0 / 0.0 | 0.25 / 0.0 |
| `fs.tree` | 90.595 / 10.08 | 90.274 / 9.437 | 0.0 / 0.0 | 0.27 / 0.0 |
| `git.changeset_diff` | 89.74 / 17.047 | 88.425 / 8.836 | 0.002 / 0.0 | 0.234 / 0.0 |
| `git.diff` | 83.656 / 19.504 | 81.626 / 4.96 | 0.001 / 0.0 | 0.196 / 0.0 |
| `git.log` | 76.043 / 9.072 | 0.628 / 4.019 | 74.64 / 0.0 | 0.371 / 0.0 |
| `git.refs` | 84.979 / 17.796 | 83.929 / 9.484 | 0.001 / 0.0 | 0.194 / 0.0 |
| `git.show` | 77.857 / 18.226 | 76.506 / 4.898 | 0.002 / 0.0 | 0.289 / 0.0 |
| `git.status` | 81.097 / 23.24 | 78.29 / 3.495 | 0.001 / 0.0 | 0.263 / 0.0 |
| `git.unpushed` | 88.163 / 27.487 | 85.593 / 7.76 | 0.001 / 0.0 | 0.254 / 0.0 |
| `github.repos` | 116.244 / 39.758 | 108.911 / 20.38 | 0.001 / 0.0 | 0.112 / 0.0 |
| `run.diff` | 105.318 / 25.968 | 103.022 / 15.339 | 0.003 / 0.0 | 0.232 / 0.0 |
| `task.doc` | 101.173 / 13.531 | 101.01 / 13.235 | 0.001 / 0.0 | 0.226 / 0.0 |
| `task.stage_diff` | 102.691 / 26.84 | 101.52 / 13.817 | 0.001 / 0.0 | 0.2 / 0.0 |
| `task.stage_doc` | 101.374 / 17.053 | 101.224 / 16.344 | 0.0 / 0.0 | 0.148 / 0.0 |
| `tasks.attachment` | 108.563 / 20.438 | 108.064 / 14.123 | 0.0 / 0.0 | 0.493 / 0.0 |
| `tasks.review.diff` | 100.877 / 24.546 | 99.771 / 11.414 | 0.001 / 0.0 | 1.082 / 0.0 |
| `tasks.review.get` | 99.278 / 36.721 | 92.088 / 10.477 | 0.002 / 0.0 | 1.205 / 0.0 |
| `thread.attachment` | 108.866 / 18.701 | 108.568 / 17.968 | 0.0 / 0.0 | 0.281 / 0.0 |
| `workspace.git_init_options` | 91.992 / 11.088 | 90.915 / 9.683 | 0.0 / 0.0 | 1.094 / 0.0 |
| `worktree.diff` | 107.8 / 24.426 | 105.887 / 17.217 | 0.002 / 0.0 | 0.243 / 0.0 |

Total p95 increased for 0/22 verbs; queue p95 increased for 1/22.
Queue increases: `git.log` (0.628 → 4.019).

### io_ordinary_git_worker

| Verb | Total p95 before / after | Queue p95 before / after | App wait p95 before / after | App hold p95 before / after |
| --- | ---: | ---: | ---: | ---: |
| `archived.list` | 151.403 / 17.795 | 151.084 / 15.318 | 0.001 / 0.0 | 0.323 / 0.0 |
| `fs.list` | 124.642 / 8.826 | 124.452 / 8.229 | 0.002 / 0.0 | 0.298 / 0.0 |
| `fs.read` | 125.442 / 12.543 | 125.049 / 11.966 | 0.0 / 0.0 | 0.35 / 0.0 |
| `fs.tree` | 125.015 / 9.261 | 124.772 / 8.709 | 0.0 / 0.0 | 0.236 / 0.0 |
| `git.changeset_diff` | 124.333 / 15.143 | 122.718 / 8.012 | 0.002 / 0.0 | 0.222 / 0.0 |
| `git.diff` | 118.604 / 17.779 | 116.588 / 7.49 | 0.004 / 0.0 | 0.22 / 0.0 |
| `git.log` | 112.351 / 5.45 | 111.504 / 4.039 | 0.091 / 0.0 | 0.2 / 0.0 |
| `git.refs` | 119.923 / 15.451 | 118.856 / 7.811 | 0.0 / 0.0 | 0.185 / 0.0 |
| `git.show` | 113.083 / 8.746 | 112.439 / 2.43 | 0.003 / 0.0 | 0.284 / 0.0 |
| `git.status` | 116.132 / 16.134 | 113.301 / 3.213 | 0.003 / 0.0 | 0.296 / 0.0 |
| `git.unpushed` | 122.533 / 21.479 | 120.289 / 7.475 | 0.002 / 0.0 | 0.279 / 0.0 |
| `github.repos` | 150.881 / 28.469 | 144.797 / 10.119 | 0.001 / 0.0 | 0.125 / 0.0 |
| `run.diff` | 140.839 / 23.162 | 138.513 / 9.927 | 0.002 / 0.0 | 0.321 / 0.0 |
| `task.doc` | 136.377 / 12.21 | 136.249 / 11.879 | 0.001 / 0.0 | 0.262 / 0.0 |
| `task.stage_diff` | 138.067 / 18.307 | 136.657 / 10.048 | 0.001 / 0.0 | 0.246 / 0.0 |
| `task.stage_doc` | 136.56 / 12.098 | 136.419 / 11.724 | 0.001 / 0.0 | 0.192 / 0.0 |
| `tasks.attachment` | 144.259 / 12.357 | 143.752 / 11.393 | 0.001 / 0.0 | 0.54 / 0.0 |
| `tasks.review.diff` | 136.117 / 19.24 | 134.496 / 11.405 | 0.002 / 0.0 | 1.04 / 0.0 |
| `tasks.review.get` | 133.806 / 33.504 | 126.188 / 8.847 | 0.002 / 0.0 | 1.095 / 0.0 |
| `thread.attachment` | 144.684 / 11.236 | 144.323 / 10.043 | 0.014 / 0.0 | 0.311 / 0.0 |
| `workspace.git_init_options` | 126.124 / 11.456 | 125.478 / 8.816 | 0.0 / 0.0 | 1.081 / 0.0 |
| `worktree.diff` | 143.555 / 17.781 | 141.378 / 12.334 | 0.001 / 0.0 | 0.285 / 0.0 |

Total p95 increased for 0/22 verbs; queue p95 increased for 0/22.

After app wait and hold sums, medians, p95 and maxima are zero for every captured verb. This establishes the measured app-lock boundary; it does not imply zero I/O, metadata-query or worker queue cost. The capture has eight samples per verb and does not establish the cause of the isolated increases.

## Acceptance-test map

The final gate results below determine pass status. New isolation regressions
are complemented by the existing semantics suites because many path bounds
and error shapes already had coverage before this migration.

| Step 4 requirement | Specific coverage | Boundary exercised |
| --- | --- | --- |
| All migrated observations run with the app mutex held | `concurrency_timing::io_observations::io_observations_are_independent_of_application_mutex`; `runtime::scope_observations::file_and_git_observations_answer_while_the_app_mutex_is_held` | All 22 real successful payloads through encrypted intake; focused file/tree/Git control through the captured handler. |
| Gated filesystem/Git work leaves SQL, snapshots and controls free | `runtime::scope_observations::gated_scope_file_and_git_jobs_leave_sql_snapshot_and_control_traffic_free` | Real `fs.read` and `git.status` jobs gate independently; `tasks.get`, `board.list` and `ping` finish before release, with zero app wait/hold. |
| Ordinary writer execution does not occupy observation execution | `carrier::observation_tests::observation_reads_bypass_the_held_write_worker` | A held ordinary worker does not prevent an observation reply from a named I/O worker. The 22-verb worker-held timing scenario measures the complete successful set. |
| Mixed metadata preparation uses SQL workers and releases connections before I/O | `carrier::observation_tests::mixed_observation_prepares_on_sql_and_releases_all_connections_before_io`; `observation_mixed::mixed_attachment_external_gate_releases_every_sql_connection` | Named SQL preparation and I/O execution workers; all eight SQL connections remain borrowable while external work is gated. |
| Pure I/O reads avoid SQL admission; folding/cancellation preserve fresh work | `carrier::observation_tests::{pure_observation_does_not_wait_for_sql_pool_capacity,observations_fold_only_queued_callers,closing_mixed_preparation_waits_releases_io_slots_before_sql_connections}` | Full SQL pool, queued-only folding, released openings and waiting preparation slots. |
| Path fences and bounded filesystem reads | `runtime::scope_observations::observation_file_reads_keep_symlink_fences_and_byte_bounds`; existing `filesystem::{fs_read_rejects_lexical_and_symlink_escapes,fs_read_media_raw_pages_keep_every_byte_and_bound_file_size,fs_read_answers_a_range_in_pages_of_whole_lines}` | Symlink/traversal refusal, whole-file cap, raw media and bounded text pages. |
| Immutable source identity and resolver purity | `runtime::scope_observations::{scope_source_move_fences_captured_reads_and_publishes_the_new_location,scope_source_membership_and_non_git_errors_match_existing_contracts}` | A captured source move or removal is fenced; fresh reads use the new committed location; cross-project/plain-source errors remain. |
| Read-your-own-write | `runtime::scope_observations::scope_files_and_git_observe_acknowledged_mutations`; `observation_mixed::mixed_attachment_read_after_acknowledged_write_uses_the_initialized_handler` | An already captured handler reads acknowledged file bytes, staged Git status and newly attached task bytes. |
| Documents retain canonical storage and path/error contracts | `observation_mixed::mixed_plan_document_keeps_canonical_store_and_error_parity`; the 22-verb fixture's `task.doc`/`task.stage_doc` response checks | Store-backed contents, unavailable/escaping paths, stage prefix containment and successful nonempty document bodies. |
| Review versions remain bound to selected work | `observation_mixed::mixed_review_delayed_work_keeps_its_selected_version_and_next_read_is_fresh`; existing `reviews::review_rpc_reads_whole_saved_files_and_refuses_caller_supplied_locations` | A prepared version-1 get/diff stays selected across version 2; the next read is fresh. Saved immutable blobs ignore unsaved checkout changes. |
| Task/thread attachment fences, ranges and owner generations | `observation_mixed::{mixed_task_attachment_keeps_path_fences_ranges_and_write_visibility,mixed_thread_attachment_keeps_homes_ranges_and_reset_removal_fences}`; existing `conversations::attachments::thread_attachment_pages_are_bounded_and_cover_the_last_byte` | Store containment, symlink refusal, exact byte ranges, reset/removal generation fences and complete final-byte paging. |
| Archive and Git-init readers retain observations | `observation_mixed::{mixed_archive_preserves_finished_registry_rows,mixed_archive_uses_published_branch_without_reopening_git}`; successful `workspace.git_init_options` in every timing scenario | Finished legacy registry rows and published branch identity; both real workspace/source Git-init probes complete successfully. |
| Existing external errors/deadlines remain | `observation_reads::github_tests::{independent_github_handler_preserves_refusal_while_the_app_is_held,independent_github_handler_preserves_the_external_command_deadline}`; existing Git process deadline tests | Injected local CLI refusal with app held; existing 20-second GitHub deadline; bounded Git child termination. |
| Deletion and publication generations cannot authorize stale work | `observation_reads::tests::project_deletion_keeps_allowed_observations_and_refuses_task_reads`; `read_view::tests::stale_observation_context_rejects_the_entire_candidate_delta` | Existing deletion exceptions and whole-candidate generation rejection. |


Additional scope regressions cover successful deletion after a captured read,
failed run path/base/stage candidates followed by unrelated publication,
completed retained-stage lineage selection and resolver purity:
`scope_workspace_delete_retires_captured_workspace_and_run_addresses`,
`scope_failed_run_path_base_and_stage_candidate_stays_out_of_later_reads`,
`scope_stage_reads_choose_completed_retained_lineage_and_its_original_repository`
and `scope_reads_never_adopt_legacy_records_or_schedule_external_scans`.
Their setup explicitly pins the publication producers so reads cannot hide
maintenance or silently choose a later incomplete lineage.

Additional mixed-reader coverage uses the already initialized service across
real reset/removal writes (`mixed_thread_attachment_service_observes_real_reset_and_agent_removal`),
retains committed document paths after a failed save and unrelated publication
(`mixed_plan_documents_retain_committed_paths_after_failed_save_and_unrelated_publication`),
checks reclaim reservation/release publication
(`mixed_review_destinations_publish_reclaim_reservation_and_release`) and
bounds archive capture to one visit per retained row at 512 rows
(`mixed_archive_capture_visits_retained_records_once`).
`store::read_pool::tests::review_metadata_uses_one_readonly_snapshot_without_the_writer_connection`
pins review metadata to one query-only WAL transaction while the writer
connection is held; the mixed external gate tests separately verify that the
connection is returned before I/O.

`mixed_capture_indexes_repeated_lineages_and_workspaces_once_with_legacy_home_parity`
counts actual mixed-context capture at four plans with three retained
lineages each and three projects: each run is indexed once, with two
workspace listing traversals for the archive and review projections.
Attachment answers match the legacy resolver, including equal-time lineage
ties and terminal filtering. This is separate producer work-count evidence;
the small timing fixture does not establish large-project publication latency,
startup cost, memory growth or optimal worker counts.

The baseline archive ran the three existing-API isolation regressions before
production migration: `file_and_git_observations_answer_while_the_app_mutex_is_held`,
`observation_reads_bypass_the_held_write_worker` and
`io_observations_are_independent_of_application_mutex` each exited **101** on
its held-resource timeout. The symlink/byte-bound control exited **0**.
`/tmp/task694-before/red-exits.json` and the `*.final.log` files preserve these
outcomes against the final matching fixture/binary.

The integrated reclaim-reservation regression also failed before its producer
fix: `mixed_review_destinations_publish_reclaim_reservation_and_release`
exited **101**, finding a missing reserved-destination error. The initial
integrated binary/source provenance and `/tmp/task694-gates/reservation-red.log`
retain that evidence; the later producer fix publishes both reservation and
release before subsequent reads.

## Final validation

At measured source `a94b008d2841c33e35eb98cdcd4a594f33b319e6`, full unfiltered
`nice -n 10 cargo test` exited **0**: **5,389 total passed and 14 existing
ignored across 33 targets**, including 5,163 unit tests and the Cargo-built
`cli_version`/`pi_extension` integration binaries. Local and `+stable`
`cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` each
exited **0**. The final after recapture also exited **0**: four tests passed,
including the all-22 held-app regression and the three timing scenarios.

The measured production implementation last changed at `dcbbfe28b`; subsequent
changes through the measured `a94b008d2` corrected test fixtures only.
The timing fixture SHA-256 remains identical before and after.
Root gate exit records live in `/tmp/task694-gates`; final scan results and
the complete task handoff are recorded on #694.

After raw data: `/tmp/task694-after/capture/bridge-concurrency-baseline.jsonl`;
stdout/stderr: `/tmp/task694-after/capture.log`; source/fixture/binary/data hashes:
`/tmp/task694-after/provenance.json`. The after binary SHA-256 is
`676d646cc0378b663079c6e2fe8299dd463cb419ebeae840cfa13929ad52a972`.
The structured exact comparison is `/tmp/task694-comparison.json`.
The combined attachment-ready plaintext export is
`/tmp/task694-step4-evidence.txt`; it includes this report, both complete raw
captures, reconstructible baseline test overlays, provenance and valid red
and final-green evidence. Temporary paths require export to outlive this
workspace.

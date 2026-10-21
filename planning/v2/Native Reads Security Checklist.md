# Native reads security checklist

Scope: #752 moves `capture.get`, `task.get`, `task.stages`, `tasks.columns`,
`thread.revision`, `term.list`, `stream.events`, and `stream.state` to existing
read lanes. Authentication, pairing, write authorization, and deployment are
unchanged and are outside this score.

Status: implementation review and gates pending. Each control is worth ten
points; the final score is recorded only after its evidence passes.

| # | Control | Evidence to verify |
|---|---|---|
| 1 | Every enrolled native read replies within 100 ms while another thread holds the application mutex; timing records exist and app-lock wait and hold are zero. | `app::tests::runtime::native_reads` and `thread_revision_sql_lane_answers_with_zero_app_lock_time`; real carrier intake, encrypted replies, and `timing.rs` hooks. |
| 2 | Existing typed input/result validation and legacy refusal behavior remain in place. Unknown and malformed scopes cannot select another owner. | Native malformed/unknown scope tests, API contract suite, and terminal refusal comparisons against the existing route. |
| 3 | Task detail preserves explicit agent and conversation bindings, pagination, stale-address refusals, and the resident-history boundary of unwindowed reads. | `task_snapshot_preserves_named_conversation_pages_and_stale_refusals`, `task_snapshot_full_keeps_reloaded_resident_tail_and_complete_windows`, and existing conversation/snapshot regressions. |
| 4 | Revision reads use the existing read-only SQL transaction and final admission fence; they cannot return unsaved history or another conversation's revision. | `sql_thread_revision_matches_content_alias_and_address_errors`, `sql_thread_revision_keeps_one_committed_snapshot_across_reset`, and `store_free_thread_revision_publishes_only_successful_rosters`. |
| 5 | Task and stage snapshots change only at the existing committed owner boundary; linked implementation updates and owner retirement refresh dependent reads. Temporarily borrowed tasks keep their published document availability. | Native committed task/stage and retirement tests, `borrowed_task_keeps_document_availability_across_maintenance`, plus existing publication and rollback tests. |
| 6 | Capture snapshots expose saved records and remove retired records, including project deletion. | `capture_snapshot_is_current_after_save_and_retirement` and `project_deletion_retires_its_published_captures`. |
| 7 | Terminal listing resolves recorded roots on the observation lane, keeps source ownership checks, and validates captured bindings before replying. Published terminal facts retain screen handles rather than sessions or processes. | Terminal source-fence, workspace-container root, resize, and retirement tests. |
| 8 | Stream reads remain QA-only, preserve bounded replay, and publish each event with its corresponding head and summary. Output length/checksum handle empty and Unicode chunks exactly. | Native QA gate/replay tests and `app::streams::tests`, including retained-root immutability. |
| 9 | New reads preserve update admission and project-deletion policy without new locks, executors, or SQLite access under the application mutex. | Existing admission/deletion tests, native column deletion refusal, and review of snapshot/SQL/observation integration. |
| 10 | Required functional gates and security scans pass on the completed source, including cargo-built CLI and Pi integration tests. | Full bridge tests, both Clippy toolchains, format, vendored tests, Pi fixture checks, Semgrep, Gitleaks, and diff whitespace check; exit codes recorded in the task report. |

The no-app-lock assertion on #722's unmerged qualification branch was inspected
before writing these tests. These focused tests model its requirements without
importing or changing that harness. They do not claim the full #722 workload
qualification has run on this branch.

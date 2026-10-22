# Native reads security checklist

Scope: #752 moves `capture.get`, `task.get`, `task.stages`, `tasks.columns`,
`thread.revision`, `term.list`, `stream.events`, and `stream.state` to existing
read lanes. Authentication, pairing, write authorization, and deployment are
unchanged and are outside this score.

Status: **100/100 for the assigned local review scope**. Populated
screen contention, real update-service contention, borrowed-owner terminal
publication, and stalled filesystem probes now have actual red-to-green
evidence. The assigning agent delegated the full bridge suite to the serial
merge gate. Each control is worth ten points.

| # | Control | Verified evidence |
|---|---|---|
| 1 | Every enrolled native read replies within 100 ms while another thread holds the application mutex; timing records exist and app-lock wait and hold are zero. | `app::tests::runtime::native_reads` and `thread_revision_sql_lane_answers_with_zero_app_lock_time`; real carrier intake, encrypted replies, and `timing.rs` hooks. |
| 2 | Existing typed input/result validation and legacy refusal behavior remain in place. Unknown and malformed scopes cannot select another owner. | Native malformed/unknown scope tests, API contract suite, and terminal refusal comparisons against the existing route. |
| 3 | Task detail preserves explicit agent and conversation bindings, pagination, stale-address refusals, and the resident-history boundary of unwindowed reads. | `task_snapshot_preserves_named_conversation_pages_and_stale_refusals`, `task_snapshot_full_keeps_reloaded_resident_tail_and_complete_windows`, and existing conversation/snapshot regressions. |
| 4 | Revision reads use the existing read-only SQL transaction and final admission fence; they cannot return unsaved history or another conversation's revision. | `sql_thread_revision_matches_content_alias_and_address_errors`, `sql_thread_revision_keeps_one_committed_snapshot_across_reset`, and `store_free_thread_revision_publishes_only_successful_rosters`. |
| 5 | Task and stage snapshots change at the committed owner boundary; linked updates and retirement refresh dependent reads. Directory probes run outside the app mutex, and delayed observations cannot overwrite successful ingestion. | Native committed task/stage tests, borrowed-task maintenance, three plan-document regressions, and existing publication/rollback tests. |
| 6 | Capture snapshots expose saved records and remove retired records, including project deletion. | `capture_snapshot_is_current_after_save_and_retirement` and `project_deletion_retires_its_published_captures`. |
| 7 | Terminal listing preserves root/source fences, reads coherent atomic dimensions without the screen mutex, and publishes create/close membership while unrelated owners are borrowed. | Populated held-screen regression, borrowed-owner create/close regressions, source fences, workspace roots, resize, retirement, and screen tests. |
| 8 | Stream reads remain QA-only, preserve bounded replay, and publish each event with its corresponding head and summary. Output length/checksum handle empty and Unicode chunks exactly. | Native QA gate/replay tests and `app::streams::tests`, including retained-root immutability. |
| 9 | Update handoff runs its busy probe outside the admission mutex. Reads succeed during a pending claim, their leases prevent premature closure, and non-read work remains excluded. | Six real-service intake regressions, active SQL lease witness, writer exclusion, reopen generation and panic rollback tests, plus existing update/MCP admission tests. |
| 10 | Required local functional gates and security scans pass, including cargo-built CLI and Pi integration tests. | Focused tests, both Clippy toolchains, format, vendored tests, Pi fixture checks, Semgrep, Gitleaks and whitespace check. The full bridge suite remains assigned to the serial merge gate. |

The no-app-lock assertion on #722's unmerged qualification branch was inspected
before writing these tests. These focused tests model its requirements without
importing or changing that harness. They do not claim the full #722 workload
qualification has run on this branch.

Review validation uses `nice -n 10` throughout and records actual exits:

- Focused review run: 321 tests passed, every selected gate exit 0. This
  includes native carrier 28, terminals 22, screens 16, update-related 102,
  runtime updates 16, MCP reads 57, read view 38, snapshot projection 9,
  snapshot publication 7, conversation SQL 16, runtime SQL 2, observations 5,
  streams 2, and the initial matched owner measurement 1.
- The six intake reads returned successful unchanged results in 2.044–5.536 ms
  with the real update service's busy callback held through reply. The stalled
  directory probe allowed a real `capture.cancel` writer to finish in 1.882 ms.
  The populated held-screen test and both borrowed-owner terminal tests passed.
- `thread.revision` logged 2.910 ms alongside its 100 ms assertion. All eight
  verbs retain nonempty timing samples with zero app-lock wait and hold.
- Pi fixture install, compaction tests, and typecheck passed on Node 22.23.4
  and npm 12.0.2.

Final gates passed on the completed source: native tests 30/30; both default
and stable Clippy with `--all-targets -- -D warnings`; format; vendored rtc
54/54 and webrtc 172 passed with five existing ignored tests; API contract
46/46; complexity ratchet 2/2; cargo-built CLI version 2/2; Pi extension 25/25;
Semgrep (51 rules, 55 files, zero findings); Gitleaks; and whitespace checks.
Every gate exited 0. No CLI or Pi integration target was skipped. Two review
test helpers initially exceeded Clippy's complexity cap (exit 101); extracting
focused helpers preserved their assertions and both toolchains then passed.

The observed red runs exited 101 before their corresponding fixes. The resumed
red-first sequence established `term.list`'s app-lock wait and then three
lifecycle defects: capture retirement, borrowed-task document availability,
and resident task history after reload. A test-only expectation of a
`reply_sealed` hook was corrected because snapshot intake has no such hook;
the decrypted final reply is the completion witness. The held mutex, 100 ms
bound, and nonempty zero-lock timing assertions remain unchanged. No focused
failure was attributed to load and no sleeps or relaxed limits were added.

Review reds at committed `f11570e9` exited 101: held-screen 1, borrowed-owner
create/close 2, directory-probe 2, and admission 9 failures. Fixes were committed
after those failures. No deadline was relaxed and no sleeps were added.

The owner measurement compares identical realistic fixtures with the new task
projection enabled and disabled, including shared and independent run
conversations. It is a scoped test control, not a separate main checkout.
Workspace rebuild counts match. The coordinator assigned the existing large
owner holds to #764 and explicitly kept a publication rewrite out of #752.
The task report includes the measured control/enabled timings and work counts.
Across the 15-sample matrices, added work is at most two agent digests and one
history update per commit, independent of tested run-agent count; native
preparation peaked at 3.138 ms (2.681 ms in the final helper-refactor rerun).
SPA source has not changed in the task delta against merged main `c8ededc68`.

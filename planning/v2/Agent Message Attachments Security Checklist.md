# Agent message attachments security checklist

Scope: #696's `post_thread_message` and `message_agent` attachment entry points,
wire 3.23.0, and the shared thread attachment reader and display.

**Scoped score: 100/100.** All ten controls below are verified (ten points each).

The authenticated agent already has filesystem access. Its absolute source
paths use the existing task-tool policy; relative sources must resolve to
stored attachments. Browser callers do not gain arbitrary-path intake.
Existing stored-file pass-through and the mutable local attachment store retain
their prior trust assumptions. This is not a new sandbox or content sanitizer.

| # | Control | Evidence |
|---|---|---|
| 1 | Both working surfaces expose optional path/name attachments, reject malformed arguments, and preserve old text-only actions. | `chat_attachment_schema_matches_task_files_on_both_working_surfaces`, `malformed_chat_file_arguments_never_reach_the_daemon_on_either_surface`, `chat_attachment_actions_still_decode_older_text_only_messages`. |
| 2 | Current session, conversation and same-project target checks precede file intake; self and unknown recipients are refused. | Existing MCP generation and agent-message suites; `a_refused_agent_target_does_not_copy_its_files`. |
| 3 | New source files retain the shared ten-file, 50 MiB, regular-file, no-follow and nonblocking checks. | `chat_attachment_intake_refuses_bad_paths_types_counts_and_large_files_without_a_message`, `chat_attachment_intake_refuses_links_and_does_not_copy_partial_batches`; existing `tracker_agent_files` FIFO and symlink tests. |
| 4 | Media signatures and UTF-8 text rules apply to the bytes copied; a batch failing new-source validation copies none of its new sources. | New chat refusal tests and the existing `tracker_agent_files` byte-type, rewrite-during-intake and atomic-validation regressions. |
| 5 | Names are sanitized and durable copies survive source deletion and a store reload. | `chat_files_are_copied_typed_persisted_and_read_after_source_deletion`, `project_agent_can_attach_files_in_its_own_chat`. |
| 6 | Both conversations and the frozen delivery payload retain attachments; receiver paths open from its checkout, and shared bytes survive source-worktree deletion. Reset retains surviving references. | `sending_files_records_both_sides_and_freezes_them_in_the_delivery_payload`, `forwarding_a_stored_chat_attachment_keeps_shared_bytes_when_sender_is_cleared` tests reset’s surviving-reference selection and source-worktree deletion; existing reset suites and real wire probe. |
| 7 | A refused file cannot produce a successful Complete lifecycle report; browser read paths remain storage fenced. | `a_refused_chat_file_does_not_emit_a_complete_report`, existing conversation attachment and reset suites. |
| 8 | Large files use bounded replies with total size and exact offsets; readers complete or refuse, never cache a partial body. | `a_large_agent_chat_recording_reads_in_transport_sized_pieces`, `agent_attachment_chat_wire_probe`, SPA paging and browser large-file regressions. |
| 9 | Cached descriptors use escaped shared tiles, Blob downloads and the shared lightbox; outgoing cards have the same behavior as replies and arrivals. | `agentMessageAttachments.test.js`, desktop/phone Chromium captures, existing lightbox, retry and conversation cache/reset suites. |
| 10 | Required bridge/SPA gates, unchanged complexity ratchets and final security scans pass. | The validation record below and the attached #696 gate logs; no new ratchets or scanner exceptions. |

## Caller and size audit

The only production caller of `thread.attachment` is the SPA's
`agentRail.js` through `fetchThreadAttachment`/`readThreadAttachment` in
`threadAttachmentPages.js`. `wireThreadAttachments` shares the completed body
between inline image previews, video, downloads and the image lightbox.
The bridge's typed handler dispatches to `AppState::thread_attachment`;
MCP delivery carries readable paths and does not call this read verb.
Other repository matches are schemas, dispatch tables, tests and documentation.

The actual user-upload limit is **5,242,880 bytes (5 MiB)**, defined by
`ATTACHMENT_MAX_BYTES` at `bridge/src/app/conversations/attachments.rs:31` and
enforced by `thread_attach` at `bridge/src/app/conversations/attachments.rs:292`
before storage. Agent source intake permits
52,428,800 bytes (50 MiB). Since wire 3.23.0, the default read is bounded to
5 MiB too, with `size` still naming the whole file and `offset` the chunk start.
The SPA follows a partial initial response even without a known paging
capability and validates each continuation before publishing a complete body.

Only a legacy initial unranged reply without `size` is trusted as a whole file.
Explicitly ranged replies require total size and offset. This compatibility
exception does not apply to wire 3.23.0, whose replies always carry both.

## Validation — 2026-10-20

New schema, intake, rendering, wire-fixture and continuation regressions failed
before their implementation changes. The browser checks reconstruct image,
video and log files larger than 5 MiB with both paging capability states, check
the complete Blob size and SHA-256, open image/video viewers and verify a native
Chromium download. A separate probe uses actual MCP intake, source deletion,
store reload and Rust RPC replies, then the production SPA reader and download.
It verifies the bytes end to end across that boundary; it does not run a live
DataChannel or the installed daemon.

All gates run under `nice -n 10`, judged by their process exits:

- Full, unfiltered bridge tests: exit 0; 5,366 passed and 14 existing ignores. The test process selects
  the existing uv 0.12.15 with `BST_UV` and uses `RUST_TEST_THREADS=4` to reduce
  wall-clock fixture contention. `--no-fail-fast` retains every test binary.
- Local and stable all-target Clippy with `-D warnings`, and formatting: exit 0.
- Locked vendor library tests: exit 0, 226 passed and five existing ignores.
- Pi fixture dependency install, 46 compaction tests and typecheck: exit 0.
- Node 22.23.3 / npm 12.0.2 transport install, SPA fresh install, lint,
  full suite (713 files / 11,039 tests), all three capture checks and build:
  exit 0. No dependency or lockfile edits.
- Semgrep `--config auto --error`, Gitleaks over `origin/main..HEAD` and
  diff-check: exit 0. Semgrep scanned all 43 changed tracked files with 206
  rules and no findings; Gitleaks found no secrets in the task history.

Earlier full runs identified two old attachment contract expectations and one
old wire-version assertion; these now test the new contract. The unrelated
200 ms frame-timing test passed unchanged both alone and in the subsequent full
runs. An unconfigured uv shim was resolved with the existing test-only
executable selector, and Python interoperability passed without a skip.
No complexity ratchets, scanner exceptions or new ignored tests were added.

The final task handoff carries the gate logs, caller audit and headless Chromium
screenshots. The Chat Isolation and Task checklists link to this extension;
their existing controls and historical scores remain unchanged.

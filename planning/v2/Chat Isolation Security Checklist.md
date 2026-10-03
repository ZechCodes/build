# Chat isolation security checklist

**Status:** verified (10/10 controls)

**Scope:** the chat-isolation trust boundary described by
`Chat Isolation Foundation.md`: explicit entity, agent, conversation, session,
turn, draft, and delivery identities across bridge persistence, command
handling, harness lifecycle, and SPA reconciliation.

This checklist is evidence-driven. A control is verified only after its named
tests pass against the completed implementation. Repository scanner results are
recorded from the final uncommitted tree as well as committed history so neither
surface can hide a leak.

| # | Control | Status | Required evidence |
|---|---|---|---|
| 1 | An explicitly supplied chat address is resolved as one `(entity_id, agent_id, conversation_id)` context and fails closed when supplied members are unknown, malformed, or mismatched. Resolvable omissions, including defined legacy commands that omit agent/conversation, derive the recorded binding exactly once at the boundary; they do not authorize later aliasing of an explicit address. | [x] | `resolve_defaults_to_the_first_agent_and_rejects_invalid_explicit_ids`, `explicit_task_post_never_executes_its_implementation_alias`, `thread_post_refuses_terminal_and_unknown_entities`, and the distinct omitted-address task-nudge regression passed. |
| 2 | A canonical shared conversation owns its history metadata, including message seen markings. Model choice, draft state, pending work, operation-consumption ownership, and execution timing remain owned by the addressed agent/controller. | [x] | `a_conversation_binding_does_not_follow_roster_position`, `a_turn_spends_the_agents_own_choice`, `agent_choose_isolated_same_provider_siblings_and_rejects_a_stale_revision`, `a_search_reads_this_agents_conversations_and_no_one_elses`, `operation_reads_are_bounded_to_the_exact_agent_and_payload`, and the SPA agent-rail/chat-repository suites passed. |
| 3 | A session event can affect only its captured agent and session instance; delayed output or EOF from an old instance cannot retire or overwrite its replacement. | [x] | `a_replaced_sessions_late_activity_close_leaves_the_replacement_alone`, `a_replaced_sessions_late_eof_leaves_the_replacement_tab_alone`, `a_late_self_report_never_lands_on_the_session_that_replaced_it`, and `stale_session_end_cannot_clear_its_replacement_execution` passed. |
| 4 | Resume accepts only exact, persisted lineage for the addressed agent and checkout; absent, stale, cross-agent, or mismatched lineage fails visibly or starts fresh without guessing. | [x] | `a_spawn_resumes_exact_lineage_and_never_guesses_from_history`, `agent_history_without_an_exact_name_and_checkout_starts_fresh`, `revived_agents_without_exact_lineage_never_guess_by_checkout`, and `a_terminal_locator_never_authorizes_a_fresh_sessions_resume_identity` passed. |
| 5 | Send freezes its exact execution address, settings revision, draft revision, and operation identity before asynchronous work; every later queue, spawn, and harness-start step validates and uses that captured address and revision, so selection, settings changes, or agent removal cannot retarget it. | [x] | Bridge choice/revision/removal regressions passed, including `choosing_is_agent_owned_and_advances_a_monotonic_revision`, `a_drained_turn_cannot_spawn_an_agent_removed_before_delivery`, and the successive/queued frozen-choice harness tests; SPA switch, remount, pending-send, and provisional-creation races passed in the final suite. |
| 6 | Acceptance is distinct from execution and transport recovery. An accepted receipt may remain queued with `operation_error`, but never restores its consumed draft. A claimed operation whose provider handoff cannot be proven after restart becomes `uncertain` under the same durable identity. Retry/status handling is idempotent and cannot duplicate transcript entries or manufacture a new operation. | [x] | `thread_post_receipt_and_message_commit_together_and_retry_is_idempotent`, `boot_requeues_only_safe_intents_and_marks_claimed_handoffs_uncertain`, `failed_operation_acceptance_rolls_back_history_and_can_retry_once`, operation acknowledgement, historical-tail, and SPA operation-error/pruning regressions passed. |
| 7 | Migration preserves each agent's current effective settings and only defensible conversation/session lineage; unknown lineage remains explicitly unresolved. | [x] | `restoring_legacy_settings_freezes_each_agents_effective_choice`, `restoring_a_pre_agent_record_adopts_its_conversation_idempotently`, `v5_database_gains_operation_receipts_without_touching_conversations`, and the full store migration suite passed. |
| 8 | Removing a primary entity or agent never rebinds another controller, draft, send, or session to a different agent or conversation; deliberate retained history remains explicitly addressed. | [x] | `removing_an_implementation_alias_never_rebinds_its_secondary_to_the_task`, `agent_remove_kills_and_reaps_the_agents_live_session`, `a_drained_turn_cannot_spawn_an_agent_removed_before_delivery`, store removal tests, and SPA selection/controller-removal tests passed. |
| 9 | The identifiers introduced or changed here (`operation_id`, provisional creation identity, and execution address members) are strictly validated at their wire and persistence boundaries, and SPA controller/cache ownership remains scoped to the existing account/device context. The existing E2EE authentication boundary is unchanged and is not rescored here. | [x] | `a_creation_operation_is_idempotent_and_cannot_be_reused_differently`, `operation_id_reuse_with_a_different_request_is_rejected`, `operation_reads_are_bounded_to_the_exact_agent_and_payload`, `operation_payload_preserves_batches_attachments_and_normalized_options`, and SPA reconnect/device-scope/provisional-identity tests passed. |
| 10 | Full functional gates and both current-tree and history secret/security scans pass on the completed tree. | [x] | Final bridge format, strict Clippy, check, and test gates; SPA lint, full test, and production build gates; standard and large-file Semgrep; current-tree and full-history Gitleaks all passed as recorded below. |

## Final gate evidence

- Bridge: `cargo fmt --all -- --check`, `cargo check --tests`, and
  `cargo clippy --all-targets -- -D warnings` passed. `cargo test --all` passed
  2,129 tests with zero failures. Seven tests were intentionally ignored: six
  library tests and the opt-in real-store migration fixture, which requires
  `BUILD_MIGRATION_FIXTURE` to name a copy of a real store.
- SPA: `npm run lint -- --no-cache` passed; `npm test` passed 194 files and
  3,363 tests; `npm run build` completed successfully with existing advisory
  bundle/CommonJS warnings.
- Semgrep: the standard `semgrep --config auto .` scan found zero findings in
  494 tracked targets. The supplementary
  `semgrep --config auto --max-target-bytes 10000000 .` scan included the four
  large tracked files, ran 510 rules against 498 targets, and found zero
  findings. One rule timed out on the unrelated
  `planning/v2/Build Landing Page v2.html`; approximately 99.9% of lines parsed.
- Gitleaks: `gitleaks dir --no-banner --redact .` scanned 29.03 MB of the final
  current tree with no leaks. The full-history scan covered 1,653 commits and
  34.84 MB with no leaks.

**Final score: 100/100.** This score applies to the ten scoped chat-isolation
controls above and records the explicit ignored-test and scanner limitations;
it is not a claim that every optional external fixture was executed.

## Merge revalidation — `origin/main`

The completed chat-isolation tree was revalidated after merging the 15 incoming
commits from `origin/main`. The merge combined explicit agent isolation with
upstream per-message read reporting; the focused
`per_message_read_reports_preserve_explicit_agent_isolation` regression passed
before the full gates.

- Bridge: `cargo fmt --all -- --check` and
  `cargo clippy --all-targets -- -D warnings` passed. `cargo test --all` passed
  2,133 tests with zero failures. Seven tests remained intentionally ignored:
  six library tests that require real provider/network conditions and the
  opt-in real-store migration fixture requiring `BUILD_MIGRATION_FIXTURE`.
- SPA: `npm run lint -- --no-cache` passed; `npm test` passed 203 files and
  3,566 tests; `npm run build` completed successfully.
- Semgrep: `semgrep --config auto --max-target-bytes 10000000 .` ran 510 rules
  against all 503 tracked targets and found zero findings. One rule timed out on
  the unrelated `planning/v2/Build Landing Page v2.html`; approximately 99.9%
  of lines parsed.
- Gitleaks current tree: 29.15 MB scanned, no leaks found. Gitleaks history with
  `--full-history HEAD origin/main` covered both merge parents, 1,668 commits and
  35.40 MB, with no leaks found.

**Post-merge score: 100/100.** The same scoped controls remain verified on the
resolved merge tree; the ignored-test and scanner limitations above remain
explicit.

## Follow-up toolbar merge revalidation

After commit `9b5e69ba`, the one additional SPA toolbar commit from
`origin/main` was merged cleanly and reviewed without Bridge changes. SPA lint,
203 files / 3,573 tests, and the production build passed. Expanded Semgrep ran
510 rules against 503 tracked targets with zero findings (the same unrelated
landing-page HTML rule timed out; approximately 99.9% of lines parsed).
Gitleaks found no leaks in the 29.16 MB current tree or in the two-head
`--full-history HEAD origin/main` scan of 1,669 commits / 35.41 MB.

**Follow-up score: 100/100.** The toolbar-only follow-up introduced no change to
the previously verified Bridge isolation controls.

## Conversation reset — #358

This extension covers `conversation.reset` and retirement of the preceding
thread generation. The existing authenticated device/E2EE boundary still
authorizes bridge RPCs. A required `project_id` additionally checks that both
the addressed owner and the canonical conversation belong to that exact
project; it is not a new project-scoped authentication credential.

| # | Reset control | Evidence |
|---|---|---|
| 1 | Validate the project, entity, agent, canonical conversation and expected generation before changing history or processes. | Foreign-project/agent and stale-generation reset regressions; typed API contract and capability fixtures. |
| 2 | Preserve agent identity, names, membership, task assignments and trackers while replacing history for every bound alias. | Identity/settings and assigned/tracked-task reset regressions; shared aliases expose the same canonical generation revision. |
| 3 | Reserve the conversation, stop and reap only its processes outside the app lock, and revoke their queued work and MCP authority. | Established-turn stop and delivery-ticket settlement, post-admission handoff refusal, off-lock reset reservation and live-session retirement regressions; old MCP done/send and deferred task-handoff admission regressions; current internal wake and stale restart-roster tests. |
| 4 | Remove exact provider artifacts and compaction sidecars without deleting another conversation's lineage. | Seventeen native-cleanup tests cover Claude/ADK, Codex/app-server, Pi, canonical/encoded checkout keys, carrier families, symlink refusal, stopped PTYs and reversible staging. |
| 5 | Commit history, operation receipts, upload ownership and readings together; a pre-commit failure restores staged files and queued delivery. | Persisted-history/restart and store-refusal rollback reset regressions. Post-commit cleanup never restores retired artifacts. |
| 6 | Delete exclusively owned attachments, including unsent uploads, while preserving surviving conversation/task references. | Draft-upload/shared-sibling reset regression and reference checks repeated at commit. |
| 7 | Prevent retired pages, activity bodies, drafts, journals, revision bodies and attachment previews from becoming readable or writable again. | Generation, cross-tab UI purge, cached-body lifetime and content-reset suites, including late reads and delayed writes. |
| 8 | Admit rows, surfaces and pooled summaries with their canonical generation in the same cache transaction; scrub embedded feed copies. | Cross-client admission, session-list and feed-reset suites, including alias choice-revision independence, an old pending list reply and legacy embedded transcript copies. |
| 9 | Gate the menu from cached capabilities and preserve the old settings until the final reset action; a fresh project agent gets its standing instructions. | DOM picker/cancellation/refusal tests, Chromium harness-switch/reset flow and fresh project scaffolding regression. |
| 10 | Verify the completed change with the bridge/SPA gates and security scans. | Final gate evidence recorded below. |

### Reset gate evidence

Verified after rebasing onto `cad6ed45`, with `conversation.reset` on wire
3.11.0. The previous-release manifest is generated from upstream 3.10.0;
`fs.projectSources` and `tasks.bodyPrecondition` retain their earlier contracts.

- Bridge: `cargo test`, `cargo clippy --all-targets -- -D warnings` and
  `cargo fmt --check` passed. The full test gate passed 4,004 tests with zero
  failures. Twelve opt-in tests remained ignored: five real-provider turns,
  one fixture recorder, two installed-CLI probes, two timing checks and two
  real-store migration fixtures.
- SPA: `npm run lint`, `npm test` (548 files / 8,607 tests, zero failures or
  unhandled errors) and `npm run build` passed. Chromium checks cover the
  keyboard/phone menu with Clear below the compaction slider, confirmation
  and picker cancellation, harness switch,
  cleared transcript/readings and empty composer in place. The four review
  screenshots were visually checked. The build retained its existing
  libsodium/module and bundle-size advisories.
- Semgrep: `--config auto --error` ran 200 rules on all 48 changed tracked
  JavaScript, MJS and CSS targets, with zero findings and approximately 100%
  parsed lines.
- Gitleaks: the task history (`origin/main..HEAD`) and an archive of the complete
  tracked tree passed with zero findings. Five exact current-tree fingerprints
  now recognize the existing public `diff_key`/`if_diff_key` fixture values;
  these name content rather than granting access, like their already recorded
  history fingerprints. `git diff --check origin/main..HEAD` also passed.
- Every gate ran under `nice -n 10` with all inherited `BRIDGE_*` variables
  removed. No serving or pairing bridge process was started for this work.

**Reset score: 100/100.** All ten reset controls are verified by the completed
gates above. The existing authentication boundary and the explicit opt-in test
and scanner scopes remain as described here.

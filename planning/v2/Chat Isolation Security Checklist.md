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
| 1 | An explicitly supplied chat address is resolved as one `(entity_id, agent_id, conversation_id)` context and fails closed when supplied members are unknown, malformed, or mismatched. Resolvable omissions, including defined legacy commands that omit agent/conversation, derive the recorded binding exactly once at the boundary; they do not authorize later aliasing of an explicit address. | [x] | `resolve_defaults_to_the_first_agent_and_rejects_invalid_explicit_ids`, `explicit_issue_post_never_executes_its_implementation_alias`, `thread_post_refuses_terminal_and_unknown_entities`, and the distinct omitted-address issue-nudge regression passed. |
| 2 | A canonical shared conversation owns its history metadata, including message seen markings. Model choice, draft state, pending work, operation-consumption ownership, and execution timing remain owned by the addressed agent/controller. | [x] | `a_conversation_binding_does_not_follow_roster_position`, `a_turn_spends_the_agents_own_choice`, `agent_choose_isolated_same_provider_siblings_and_rejects_a_stale_revision`, `a_search_reads_this_agents_conversations_and_no_one_elses`, `operation_reads_are_bounded_to_the_exact_agent_and_payload`, and the SPA agent-rail/chat-repository suites passed. |
| 3 | A session event can affect only its captured agent and session instance; delayed output or EOF from an old instance cannot retire or overwrite its replacement. | [x] | `a_replaced_sessions_late_activity_close_leaves_the_replacement_alone`, `a_replaced_sessions_late_eof_leaves_the_replacement_tab_alone`, `a_late_self_report_never_lands_on_the_session_that_replaced_it`, and `stale_session_end_cannot_clear_its_replacement_execution` passed. |
| 4 | Resume accepts only exact, persisted lineage for the addressed agent and checkout; absent, stale, cross-agent, or mismatched lineage fails visibly or starts fresh without guessing. | [x] | `a_spawn_resumes_exact_lineage_and_never_guesses_from_history`, `agent_history_without_an_exact_name_and_checkout_starts_fresh`, `revived_agents_without_exact_lineage_never_guess_by_checkout`, and `a_terminal_locator_never_authorizes_a_fresh_sessions_resume_identity` passed. |
| 5 | Send freezes its exact execution address, settings revision, draft revision, and operation identity before asynchronous work; every later queue, spawn, and harness-start step validates and uses that captured address and revision, so selection, settings changes, or agent removal cannot retarget it. | [x] | Bridge choice/revision/removal regressions passed, including `choosing_is_agent_owned_and_advances_a_monotonic_revision`, `a_drained_turn_cannot_spawn_an_agent_removed_before_delivery`, and the successive/queued frozen-choice harness tests; SPA switch, remount, pending-send, and provisional-creation races passed in the final suite. |
| 6 | Acceptance is distinct from execution and transport recovery. An accepted receipt may remain queued with `operation_error`, but never restores its consumed draft. A claimed operation whose provider handoff cannot be proven after restart becomes `uncertain` under the same durable identity. Retry/status handling is idempotent and cannot duplicate transcript entries or manufacture a new operation. | [x] | `thread_post_receipt_and_message_commit_together_and_retry_is_idempotent`, `boot_requeues_only_safe_intents_and_marks_claimed_handoffs_uncertain`, `failed_operation_acceptance_rolls_back_history_and_can_retry_once`, operation acknowledgement, historical-tail, and SPA operation-error/pruning regressions passed. |
| 7 | Migration preserves each agent's current effective settings and only defensible conversation/session lineage; unknown lineage remains explicitly unresolved. | [x] | `restoring_legacy_settings_freezes_each_agents_effective_choice`, `restoring_a_pre_agent_record_adopts_its_conversation_idempotently`, `v5_database_gains_operation_receipts_without_touching_conversations`, and the full store migration suite passed. |
| 8 | Removing a primary entity or agent never rebinds another controller, draft, send, or session to a different agent or conversation; deliberate retained history remains explicitly addressed. | [x] | `removing_an_implementation_alias_never_rebinds_its_secondary_to_the_issue`, `agent_remove_kills_and_reaps_the_agents_live_session`, `a_drained_turn_cannot_spawn_an_agent_removed_before_delivery`, store removal tests, and SPA selection/controller-removal tests passed. |
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

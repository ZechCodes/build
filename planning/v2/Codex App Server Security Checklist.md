# Codex app-server harness security checklist

**Status:** 100/100 (10/10 controls verified)
**Verified:** 2026-09-09
**Scope:** the trust boundary added by the headless Codex app-server harness in
`bridge/src/harness/codex_app_server/`. Everything the `codex app-server
--stdio` child sends over stdout — responses, notifications, server requests,
and stderr — is untrusted input to the bridge. Product completeness is tracked
separately from this security score.

Native subagent coverage additionally lives in `subagents::tests`: bounded
identities and agent counts, distinct tool/agent completion, restart/replay
handling, and preserving authoritative errors.
`session::tests::parent_subagent_items_publish_surfaces_while_child_items_stay_isolated`
checks the surface subscription and parent/child boundary together.

Every verification names a test in the crate. Unqualified names live in
`bridge/src/harness/codex_app_server/tests.rs`; a name marked with its module
lives in that module's inline suite. Run them with `cargo test
codex_app_server` from `bridge/`.

| # | Control | Score | Verification |
|---|---|---:|---|
| 1 | Every app-server server request is refused or read-only, answered from one static table on every route, and unknown methods or missing routing ids fail closed before any param is inspected | 10/10 | `every_server_request_has_a_refusing_or_read_only_policy`, `untrusted_request_params_never_reach_the_response_or_the_report`, `server_request_decoder_types_known_requests_and_retains_only_unknown_methods`, `known_thread_scoped_requests_without_a_routing_id_fail_before_policy`, `unknown_server_requests_ignore_arbitrary_params_before_policy`, `unscoped_requests_keep_their_tabled_session_failure_on_every_route` |
| 2 | Child-thread traffic cannot read, mutate, or terminate the parent session, and every child request still gets the same safe response with no parent effect | 10/10 | `parent_thread_filter_isolates_every_child_notification_before_decoding`, `every_known_child_thread_request_receives_a_safe_continue_response`, `child_requests_with_malformed_non_routing_params_still_receive_the_safe_response`, `child_thread_events_are_isolated_while_parent_subagent_activity_is_retained`, `session::tests::child_thread_traffic_never_contaminates_the_parent_session`, `session::tests::child_traffic_never_advances_the_parent_quiet_clock`, `session::tests::child_thread_start_is_routed_away_before_and_after_parent_readiness` |
| 3 | A thread opens only in the requested worktree, with `approvalPolicy: never` and the requested model, and only on an exact persisted thread id — never a guessed transcript | 10/10 | `thread_open_settings_must_match_the_requested_session`, `exact_resume_id_selects_resume_and_fresh_never_guesses`, `thread_open_response_operation_must_match_the_persisted_resume_id`, `request_shapes_put_model_and_effort_only_where_the_protocol_accepts_them` |
| 4 | Untrusted bytes are bounded before retention on every path: inbound and outbound frames, queued input, open items, deduplication keys, and stderr | 10/10 | `connection::tests::oversized_frame_is_discarded_through_newline_before_the_error_returns`, `connection::tests::crlf_frames_decode_up_to_the_exact_inbound_limit`, `connection::tests::a_carriage_return_at_the_limit_does_not_lift_the_inbound_bound`, `connection::tests::capped_outbound_serializer_never_retains_more_than_the_limit`, `every_write_path_enforces_the_outbound_frame_limit`, `queue_limits_fail_without_partial_insertion`, `completed_item_lru_evicts_non_fatally_and_refreshes_duplicates`, `process::tests::stderr_tail_caps_each_line_and_the_aggregate`, `limits::tests::each_component_reads_only_the_bounds_it_enforces` |
| 5 | Tool actions and results expose only selected, clipped one-line excerpts; full output streams, patch bodies, image data, and arbitrary JSON objects never enter the conversation | 10/10 | `completed_speech_and_tools_translate_with_bounded_readable_details`, `tool_summaries_select_safe_fields_and_never_dump_objects_or_diffs`, `tool_results_report_errors_exit_codes_and_text_without_dumping_objects`, `long_tool_call_and_result_summaries_remain_bounded`, `speech_summaries_share_the_activity_summary_bound`, `a_hostile_error_message_reports_within_the_activity_bound`, `every_schema_known_item_has_one_explicit_classification`, `suppressed_items_need_no_id_and_consume_no_ledgers` |
| 6 | Correlation cannot be hijacked: a response resolves only its own typed operation, malformed or duplicate ids fail the connection, and request ids never leave the connection | 10/10 | `correlation_resolves_out_of_order_to_typed_operations`, `a_response_body_that_does_not_match_its_operation_fails_the_connection`, `a_second_response_on_a_resolved_id_is_unknown`, `malformed_and_unknown_responses_fail_without_stealing_another_request`, `pending_overflow_and_failed_write_leave_correlation_unchanged`, `close_is_idempotent_and_writes_after_close_fail` |
| 7 | Build MCP stays the only lifecycle authority, its tools stay owner-scoped behind a per-session token, and the child inherits no agent identity from the daemon | 10/10 | `the_app_server_child_inherits_no_agent_identity_and_scopes_its_mcp_token`, `app_server_spec_reuses_codex_mcp_config_without_experimental_flags`, `build_mcp_dynamic_and_unknown_items_are_suppressed`, `observed_mcp_fixture_suppresses_build_and_pairs_non_build_activity` |
| 8 | No thread or turn request precedes a verified 0.153.0 handshake, and the client advertises no capability that could admit attestation, elicitation, or dynamic tools | 10/10 | `initialize_is_first_and_a_turn_waits_for_readiness`, `a_below_floor_user_agent_fails_without_asking_the_probe`, `initialize_version_floor_uses_only_the_leading_matching_component`, `version_probe_evidence_accepts_the_floor_and_preserves_every_failure`, `initialize_error_fails_the_session`, `request_shapes_put_model_and_effort_only_where_the_protocol_accepts_them` |
| 9 | The child is killed at most once and reaped exactly once, an interrupt never kills it, and a pipe held open by a detached grandchild cannot strand the session | 10/10 | `process::tests::shutdown_is_idempotent_and_reaps_the_child_exactly_once`, `process::tests::concurrent_shutdowns_kill_once_and_reap_once`, `process::tests::failed_wait_keeps_the_child_for_a_retry_without_killing_twice`, `process::tests::reap_lag_is_the_only_fact_a_caller_can_read_from_the_process`, `session::tests::interrupt_asks_codex_to_stop_and_never_kills_the_process`, `session::tests::stdout_held_open_past_the_grace_still_publishes_ended`, `session::tests::stderr_held_open_past_the_grace_still_publishes_ended` |
| 10 | Checked-in protocol fixtures carry no account, token, or machine material, the suite needs no network or Codex account, and repository scanners are clean | 10/10 | `checked_in_fixtures_retain_no_account_or_machine_material`, `bridge/tests/fixtures/codex-app-server/0.153.0/PROVENANCE.md`, `semgrep --config auto bridge/src/harness/codex_app_server` (0 blocking findings), `gitleaks detect --no-banner` over the repository (0 leaks) |

**Total: 100/100.**

## Boundary notes

These are deliberate, design-approved exposures. They are not gaps against the
controls above, and each is named so a later reviewer does not mistake it for
one.

- **The agent runs with `sandbox: "danger-full-access"`.** Build's model is
  enforcement by observation: the agent works in a git worktree and the human
  reviews the diff. The harness verifies that Codex opened the thread with
  exactly the requested cwd and policy rather than sandboxing the agent.
- **The Build MCP token reaches Codex as a `--config` argument.** The
  app-server harness reuses the Codex TUI's one config builder, so the token is
  visible in the local process list exactly as it already is for the TUI
  provider. The token stays per session and owner-scoped; changing how it is
  delivered belongs to the shared Codex config builder, not to this harness.
- **The epitaph is bounded by the frame and stderr limits, not by the
  240-character activity bound.** A terminal reason may carry up to one inbound
  frame (1 MiB) or the retained stderr tail (32 KiB) so a crash stays
  diagnosable. Every reason that reaches the conversation as activity is clipped
  to one line first.
- **A child-thread request is answered, not ignored.** Refusing to reply would
  hang the child; the reply is the tabled safe response and carries no parent
  effect, which is what control 2 verifies.
- **Tool details are visible to the owner.** Commands, selected tool arguments,
  and output/error excerpts are intentionally included in bounded activity
  summaries so the owner can understand what ran and its result. These fields
  can contain workspace content, like the existing tool activity from other
  harnesses. Full protocol payloads and output streams are not retained.
- **Native subagents are observed through parent-owned events.** Their bounded
  metadata feeds Build's existing subagent panel and change subscription. This
  does not grant child threads control over the parent session or make a native
  agent's completion an authority over the Build task lifecycle.

# Pi compaction adapter security checklist

**Status: 100/100 (10/10 controls verified).**

Scope: #659, Pi 0.86.1 between-turn fallback, scoped extension policy,
context and lifecycle telemetry. Native threshold injection is not qualified;
other Pi versions remain unsupported. See [Pi Compaction Qualification.md](Pi%20Compaction%20Qualification.md).

| # | Control | Evidence |
| --- | --- | --- |
| 1 | Only the qualified hook version enables fallback; unknown versions cannot claim support | `pi_compaction_qualifies_only_exact_hook_version_and_keeps_threshold`, `pi_compaction_unknown_version_is_unsupported_without_rounding_intent`; six real pinned-runtime qualification scenarios |
| 2 | Zero is rejected and unsafe legacy values retain exact intent without JS rounding | Shared resolver rejection tests; `pi_compaction_preserves_unsafe_legacy_values_and_rejects_zero`; disabled decimal extension fixture |
| 3 | Captured choice and identities cannot be silently substituted at spec construction | `pi_compaction_spec_rejects_forged_policy_and_identity_before_writing`; canonical colon-separated thread identity fixture |
| 4 | Policies and logs stay isolated per opening and outside the checkout; no user/workspace settings writes | `pi_compaction_spec_preserves_exact_session_and_scopes_each_launch`; existing Pi containment tests; private exclusive Rust log creation and no-follow extension writes; Node sink privacy tests |
| 5 | Threshold cancellation never disables overflow recovery or manual compaction | Actual extension hook tests and pinned binary qualification, including native manual commands, failed summaries and overflow retry |
| 6 | Long turns and tool work cannot be interrupted by Build's fallback; only confirmed idle boundaries allow manual sends | `pi_long_turn_crosses_threshold_but_only_settled_allows_fallback`, `pi_fallback_waits_for_settled_then_compacts_and_delivers_follow_up_once`; deferred manual settlement fixture |
| 7 | Null context cannot restore stale readings, including coalesced watch updates | `pi_null_reading_invalidates_old_context_and_identical_fresh_usage_is_new`, `pi_overflow_recovery_preserves_the_open_turn_and_clears_stale_context`; cumulative reset counter and current-reading comparison |
| 8 | Failure, abort and cancellation cannot report successful compaction or duplicate deferred input | `pi_summary_failure_abort_and_cancel_deliver_follow_up_without_success`; real queue/pump tests, paired compaction observations, actual runtime failure qualification |
| 9 | An unavailable extension explicitly withdraws support; foreign/replayed observations cannot authorize work | `pi_extension_readiness_and_failure_are_explicit_unsupported_states`, `pi_foreign_and_replayed_observations_never_unlock_another_turn`; `pi_extension_failure_ends_and_reaps_the_owned_child`; terminal EOF final-record drain/activity-closure tests; hook exceptions, identity mismatch and sink failure tests |
| 10 | Regression, complexity, type and security gates preserve predecessor contracts | Full bridge tests, clippy, fmt, vendor lib tests; pinned Node hook suite and scoped TypeScript check; changed-file semgrep, main..HEAD gitleaks, diff check. Exact exit evidence is recorded on #659 |

No new complexity allowances are added. Security scoring covers the controls
above, not support for unqualified Pi versions or native threshold injection.
The inherited Pi TUI and compaction session-policy checklists remain applicable.

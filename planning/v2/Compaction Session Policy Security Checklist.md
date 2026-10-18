# Compaction session policy security checklist

Scope: #655, the internal launch/session contract and automatic between-turn
decision. Native adapters, native argv/environment translation, wire changes,
and live qualification are outside this step. **Score: 100/100**, verified
2026-10-18. Each control below scores 10/10.

| # | Control | Verification |
|---|---|---|
| 1 | Admission uses the frozen launch provider and canonical conversation scopes; aliases cannot borrow local overrides | `frozen_provider_resolves_canonical_alias_scopes_after_the_choice_changes`, existing policy alias/precedence tests |
| 2 | Settings edits cannot mutate an admitted or warm snapshot | `admitted_policy_survives_setting_edits_warm_reuse_and_browser_attach`, `a_warm_policy_does_not_follow_a_later_setting_edit`, `a_warm_policy_keeps_its_threshold_when_settings_are_raised` |
| 3 | Failed or pending startup cannot advertise an active policy | `failed_open_never_publishes_an_active_policy`, `an_async_opening_installs_no_policy_until_the_protocol_accepts_it`, `a_child_running_another_model_than_asked_is_ended_with_that_as_its_epitaph`; ADK accepted init and Codex accepted-thread hooks |
| 4 | A stale generation cannot publish or mutate the replacement's runtime choice, resume ID, token or waiting clients | `a_session_opened_for_an_old_thread_cannot_publish_its_policy`; existing reset/delivery fences |
| 5 | Only an idle warm session with trustworthy context can schedule one inclusive threshold compaction | `captured_fallback_is_inclusive_and_unknown_or_native_never_schedules_it`, `one_compaction_per_reading_and_the_turn_follows_it`, working/cold/interrupt eligibility tests |
| 6 | Native mode cannot duplicate automatic compaction; manual commands and existing ledger/receipt behavior remain available | `native_policy_does_not_schedule_automatic_compaction_but_manual_commands_still_work`, `compaction_tools`, delivery/receipt/recovery suites |
| 7 | Unknown/old/failed versions cannot gain native authority; zero is refused and unsafe legacy values remain exact | `all_unqualified_versions_and_values_declare_fallback_or_unsupported`, `zero_is_refused_and_unsafe_legacy_values_remain_exact_but_unsupported`, `compaction_version_is_passive_and_a_failed_refresh_removes_its_authority` |
| 8 | Native failed, cancelled or interrupted items cannot become successful events, clear context or consume receipts | both `unsuccessful_native_compactions` tests; `native_compaction_activity_preserves_the_active_turn_and_operation` |
| 9 | Reopening resolves the policy again and preserves context for exact resume, idle reopen and bridge recovery; a missing-session fresh fallback clears it; warm/idle/attach retains the snapshot and native model changes need requalification | lifecycle boot/resume/idle tests; `first_user_turn_after_exact_reopen_compacts_with_the_saved_context_before`, `missing_recorded_resume_starts_fresh_and_clears_the_old_context`; native/in-place model-change tests |
| 10 | Policy handling adds no credentials, child arguments, environment changes or persistence files; validation is isolated | fake sessions/children, throwaway HOME, read-only Cargo/Rustup/Node/uv mounts; full test and lint gates, changed-file semgrep, `main..HEAD` gitleaks and diff check |

The wrapper delegates all session capabilities (including interruption,
termination, observations, receipts and recovery identity). Existing scoped
MCP tokens and routing/session ownership stay in their original code paths.
Legacy sessions without a snapshot remain unknown until reopened. Pi's missing
fallback command is reported as unsupported. No native support is claimed.

Validation: full `cargo test -j 2`, `cargo clippy --all-targets -j 2 -- -D warnings`,
`cargo fmt --check`, and `cargo test --locked --manifest-path vendor/Cargo.toml
--lib -j 2` passed. Changed-file semgrep (`--config auto --error --jobs 2`),
`main..HEAD` gitleaks and `git diff --check` passed. All gates ran under
`nice -n 10`; tests used a throwaway HOME and fake sessions/children. Existing
ignored tests remained ignored; no test skip flags were added.

The warm-policy and failed-native-compaction regressions failed before their
fixes. The first full gate also exposed the architecture guard's missing
`PolicySession` entry. Isolated-runner PATH omissions for Node and uv were
corrected without changing the application or tests. A repeat semgrep run hit
an engine memory-allocation error; limiting it to two workers completed the
same changed-file scan with zero findings.

Review regressions cover preservation of context tokens and timestamps through
exact resume, idle retirement/reopen and boot recovery; the first eligible
turn after exact reopen compacts with its saved `context_before`. A probe that
finds the recorded provider session missing clears context only for that fresh
replacement. The ADK model-mismatch fixture also verifies that a refused init
never exposes an applied policy. These five assertions failed before their
fixes (exit 101). ADK acceptance checks announcement and absence of refusal
under one lock; the Codex accepted-thread and PTY opening paths were audited
for the same pattern and required no changes.

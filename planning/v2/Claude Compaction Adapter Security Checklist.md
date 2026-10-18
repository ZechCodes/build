# Claude compaction adapter security checklist

Scope: #656, shared Claude ADK/TUI translation, per-launch compaction settings,
managed-policy diagnostics and native activity observation. Production native
promotion stays closed. **Score: 100/100** (ten controls at 10/10), 2026-10-18.

| # | Control | Evidence |
|---|---|---|
| 1 | Version/model labels alone never grant native enforcement | `claude_compaction_production_is_unqualified_even_for_the_observed_model_and_version`; isolated-run qualification record explicitly limits its conclusions |
| 2 | Zero, overflow and targets beyond model/native limits cannot be silently enforced by clamping | `claude_compaction_bounds_and_versions_never_silently_clamp`; inherited unsafe-value and zero-refusal contract tests |
| 3 | Translation uses actual output reserve and preserves exact positive threshold intent | `claude_compaction_translation_uses_the_real_output_reserve`; 400000 small-threshold round trips in `claude_compaction_small_thresholds_round_trip_without_rounding_intent` |
| 4 | Compaction normalization changes only child-specific controls; hooks, model suffixes and unrelated output settings survive | `claude_compaction_merge_preserves_unrelated_settings_and_hooks`; fake children capture both carriers, preserve `[1m]`, settings file bytes and output-token value |
| 5 | Managed policy is read-only, bounded and never executes helpers; diagnostics reveal key names but no values or arbitrary paths | Nine managed-policy fixture tests: 8 MiB/file, 16 MiB total and 256 directory-entry bounds; descriptor regular-file verification; unreadable/path/platform limitations explicit |
| 6 | Admitted settings remain one session's captured policy, and mismatches cannot apply to another launch choice | `claude_compaction_refuses_mismatched_native_policy_or_choice`; existing admission/edit, failed-start, resume-context and generation tests from #655 |
| 7 | Fresh, exact-resume and continue carry independent settings for ADK and TUI without config writes | `claude_compaction_fake_children_override_stale_settings_and_preserve_manual_compact`; independent fallback/150k/200k/small native fixtures for each carrier and resume mode |
| 8 | Fallback disables automatic compaction without disabling manual commands; managed interference is reported best effort | Child `DISABLE_AUTO_COMPACT=1`, `DISABLE_COMPACT=0` in both environment and flag settings; fake manual `/compact`; managed conflicting-key tests |
| 9 | Native compaction is activity within the ongoing turn, never a new message or a turn end | `native_compaction_preserves_the_live_turn_and_delivers_queued_input_once`, status/hook dedup and unsuccessful-attempt tests; existing native no-duplicate-threshold tests |
| 10 | Validation and qualification do not modify live state or original credentials, and add no complexity exceptions | Throwaway HOME with read-only toolchains; four small isolated CLI runs with read-only credential copy deleted afterward; full tests/lint/format/vendor/scanner/diff gates recorded on #656 |

No RPC fields/events, SPA, Codex or Pi adapters, spawn routing or ownership were
changed. Local managed-policy observations do not prove future remote policy;
unknown effective budgets remain explicit fallback. The dedicated qualification
record includes exact version, executable hash, command shape and observed
compaction boundaries. Gates and their exit codes are recorded in the task's
final evidence; no result is inferred from a printed success summary.

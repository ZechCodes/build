# Codex compaction adapter security checklist

Scope: #657, shared Codex app-server/TUI translation and per-launch config.
Production native promotion remains closed. **Score: 100/100** (ten controls at
10/10), 2026-10-18. Final gate exit codes and logs are recorded on the task.

| # | Control | Evidence |
|---|---|---|
| 1 | A version/model label cannot grant unverified native enforcement | `codex_compaction_unqualified_models_and_versions_keep_positive_fallback`; qualification records source and live-run limits; production `prepare` never returns Native |
| 2 | Translation preserves positive intent without silently accepting Codex's clamp | `codex_compaction_translation_preserves_total_tokens_within_established_model_ceiling`; N=T within floor(9W/10), with exact-boundary checks |
| 3 | Zero, unsafe thresholds, unknown versions/windows and upstream integer overflow are refused | `codex_compaction_translation_rejects_unknown_or_unrepresentable_inputs`; window multiplication constrained to signed native range; inherited zero/unsafe resolver coverage |
| 4 | Only child compaction keys are normalized, without parent environment or real file writes; unverified selected profiles block promotion | `codex_compaction_normalizes_only_child_compaction_overrides`; ordinary fake home/worktree config files retain exact bytes and unrelated window/personality values. `compaction_selected_profile_files_keep_both_public_carriers_on_fallback` covers legacy selector/table and `--profile x`/`x.config.toml`, positive fallback with `codex_model_window_buffer_and_profile_unqualified` and unchanged files. The credential-free 0.160.0 probe rejects legacy `profile`/`profiles` tables; `--profile x` is unavailable to app-server. No native profile precedence is claimed or simulated |
| 5 | Fresh, exact-resume and continue apply the captured policy through real carrier entry points | `compaction_fake_children_keep_ordinary_file_start_resume_snapshots_distinct` uses public app-server open; `compaction_tui_fake_child_uses_ordinary_root_overrides_for_fresh_exact_and_continue` executes generated TUI argv |
| 6 | Config cannot leak between agents or into unrelated protocol requests | Concurrent fake app-server children retain distinct thresholds; `compaction_config_is_scoped_to_thread_open_requests_only`; connection config is immutable and only serialized on start/resume |
| 7 | Stale or malformed native policies are refused during spec construction; app-server rechecks at opening | `codex_compaction_rejects_another_choice_and_corrupted_native_translation`; `compaction_open_rejects_another_launch_choice_before_spawning`; existing admission, generation, failed-open and resume-context contract tests |
| 8 | Fallback makes no native-disable claim; native never adds a duplicate Build threshold request | `codex_compaction_fallback_does_not_claim_to_disable_native_defaults`; both lifecycle tests assert automatic compaction is not due and only two user turns reach the child |
| 9 | Only a successful observed boundary reports compaction, preserving open turns and queued input | Both `codex_*_native_compaction_preserves_work_and_delivers_follow_up_once` tests run real activity/status pumps and delivery queue; failed/cancelled/interrupted items retain context, foreign/old records and terminal banners cannot succeed, and queued follow-up is delivered once; existing pending-receipt regressions retained |
| 10 | Qualification and validation do not modify live state, original credentials or complexity exceptions | Throwaway HOME/CODEX_HOME and read-only credential copies deleted after isolated runs; read-only toolchains for gates; no bridge binary, installer, pairing/browser, network/firewall changes or new complexity allowances |

The qualification record documents exact commands, executable/version/hash,
observed boundaries, probe failure and corrected retry, and all promotion
limits. Native trigger equality requires more than the candidate arithmetic:
effective raw/usable windows, fallback buffer, post-turn settings and model
changes must be established for an actual launch. No version/model range is
advertised as native-capable. Selected-profile precedence is an additional
explicit blocker; fallback does not repair rejected legacy profile settings.
The wrapper's between-turn fallback can coexist
with Codex's independent default context management.

No Build wire fields/events, SPA, Claude/Pi adapters, spawn routing, session
ownership, alias resolution or managed-policy inspection changes are included.

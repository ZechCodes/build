# Installed CLI probe security checklist

**Status:** 100/100 (11/11 controls verified)
**Verified:** 2026-09-28
**Scope:** the spawn paths and trust boundary added by #203 in
`bridge/src/harness/installed/`. The bridge now starts the agent CLIs on its
own, outside any agent session, to learn what they run: `claude --version`
(`probe.rs`, `VersionFlag`) and a short-lived `codex app-server` asked
`initialize` and `model/list` (`probe/codex_list.rs`), both through
`probe/child.rs`. Everything those children write is untrusted input, and what
they list reaches the model pickers and, once picked, a spawn's argv.

Every verification names a test in the crate. Unqualified names live in
`bridge/src/harness/installed/probe/tests.rs`; a name marked with its module
lives in that module's suite (`installed::tests`, `offer::tests`) or in
`bridge/src/app/tests/installed_models.rs` (`app::installed_models`). Run them
with `cargo test installed` from `bridge/`.

| # | Control | Score | Verification |
|---|---|---:|---|
| 1 | A probe runs the CLI by name with fixed arguments (`--version`, `app-server`) and no shell; nothing a user, agent or CLI said reaches its argv | 10/10 | `the_version_flag_reads_the_cli_s_answer` (the fake exits unless argv is exactly `--version`), `codex_lists_every_model_on_every_page_and_its_version` (exits unless `app-server`), `ProbeChild::start` builds a `Command` with a static argument list |
| 2 | A probe that does not answer is cut off at a 3 s deadline, and its whole process group is killed and reaped, so nothing a wrapper started outlives it | 10/10 | `a_hanging_cli_is_killed_with_everything_it_started`, `a_codex_that_never_answers_is_cut_off` (both check the grandchild's pid is gone) |
| 3 | What a probe reads into memory is bounded, per line (1 MiB) and in total (4 MiB), and a child that says more is cut off by that bound rather than by the clock; stderr is never read | 10/10 | `a_cli_that_says_too_much_is_cut_off` (endless lines and one endless line; fails with the bounds lifted) |
| 4 | A listed model reaches a picker only if its id has the shape a spawn accepts (`models::is_model_id`, the check `ModelChoice::validate` applies); labels are clipped and stripped of control characters; at most 200 models are kept; efforts are only those the harness itself can pass | 10/10 | `codex_keeps_only_what_could_be_a_model` (fails with the id check removed), `offer::tests::a_listed_model_keeps_the_catalog_s_words_where_it_has_them`, `harness::tests` `every_catalogued_model_validates_against_its_own_provider` |
| 5 | A probe starts in the home directory, not in a checkout (no project-local pin or hook is read), and inherits none of the agent-identity markers (`INHERITED_AGENT_MARKERS`) | 10/10 | `a_probe_runs_in_the_home_directory_as_nobody_s_agent` (the fake exits unless it stands in `$HOME`; the command it is started by removes every marker, read off `Command::get_envs` rather than by setting one in the test process) |
| 6 | A CLI that is missing, fails, or answers unreadably reads as knowing nothing, and a harness that knows nothing offers its whole catalog and refuses nothing: a probe can never hide a model or block a spawn on a guess | 10/10 | `a_cli_that_cannot_answer_reads_as_knowing_nothing`, `a_missing_codex_reads_as_knowing_nothing`, `a_codex_that_cannot_list_still_says_its_version`, `offer::tests::an_unread_version_offers_the_whole_catalog`, `offer::tests::a_cli_that_listed_nothing_is_offered_the_whole_catalog`, `offer::tests::a_cli_that_listed_an_empty_list_is_offered_the_whole_catalog` and `a_codex_that_lists_nothing_usable_reads_as_listing_nothing` (an empty `data`, and a list none of whose ids could be a model: both fail with their filter removed), `installed::tests::what_the_installed_cli_runs_is_started` |
| 7 | No RPC waits on a probe: a read answers from what is held, one ask per CLI runs at a time on a background thread, and a session's reported version can only schedule an ask, never supply the answer. The one caller that waits on a probe is the spawn gate, off every RPC on the delivery thread, and only before refusing on an answer over 30 s old; `agent.add` refuses on a fresh held answer alone | 10/10 | `installed::tests::a_stale_refusal_asks_the_cli_again_first` (fails with the re-read removed), `installed::tests::a_held_refusal_needs_a_fresh_answer`, `installed::tests::a_runnable_model_asks_nothing_before_it_starts`, `installed::tests::one_ask_at_a_time_per_cli`, `installed::tests::a_background_ask_lands_and_is_announced`, `installed::tests::a_session_reporting_another_version_asks_again_at_once`, `installed::tests::a_session_reporting_the_held_version_asks_nothing` |
| 8 | A model the installed CLI cannot run is refused before anything is written or started, with a plain sentence, on both Claude carriers and against codex's full (hidden-included) list, by the gate on every spawn (`AgentSpawnPlan::probe_and_scaffold`) reading the `AppState`'s readings; the refused message is settled as failed, not uncertain; defaults (roles, project agent) fall back rather than fail | 10/10 | `app::installed_models::a_refused_model_never_spawns_and_says_why` (`agent.add` and a delivery respawn; fails with the spawn gate, the `agent.add` check, or the certain settlement removed, each alone), `installed::tests::a_model_newer_than_the_installed_claude_code_is_refused_on_both_carriers`, `installed::tests::a_model_codex_does_not_list_is_refused`, `offer::tests::a_listing_cli_starts_hidden_models_and_refuses_unlisted_ones`, `app::installed_models::a_role_passes_over_a_model_the_installed_cli_cannot_run`, `app::installed_models::a_project_agent_set_to_an_unrunnable_model_starts_on_the_harness_default` |
| 9 | The `models.changed` push carries nothing but its type, goes only to greeted sessions, and ends with the session | 10/10 | `app::installed_models::a_greeted_session_hears_when_an_installed_cli_changes` (fails with the subscription removed), `tests/api_contract.rs` `every_event_example_is_what_the_bridge_serialises`, `rpc.rs` close frame calls `unsubscribe_models_changed` |
| 10 | Tests need no installed CLI, account or network (every probe test runs fake scripts; the unit-test readings never probe), and the scanners are clean | 10/10 | `installed::tests::the_unit_test_readings_refuse_nothing`, `semgrep --config auto` over the changed files (0 blocking findings), `gitleaks git --log-opts=main..HEAD` (0 leaks) |
| 11 | A probe never installs a CLI: it runs with `MISE_OFFLINE=1`, so a mise wrapper runs what is installed or fails at once, rather than starting a download the 3 s deadline would kill halfway | 10/10 | `a_probe_tells_mise_to_stay_offline` (fails without it); checked by hand with mise 2026.9.9 and the `~/.local/bin` wrappers (`mise use -g claude`, then exec), in an empty mise home (`MISE_DATA_DIR`, `MISE_CONFIG_DIR`, `MISE_CACHE_DIR`, `MISE_STATE_DIR` set to a scratch directory): online and killed at 3 s, the wrapper left `claude 2.1.284 (missing)` with a 17 MB partial download (the next plain run finished it, so mise recovers); offline, the wrapper and `the_installed_clis_answer` read nothing and installed nothing; with the real mise home, offline reads `2.1.280` and `0.155.1` as before |

**Total: 100/100.**

## Boundary notes

- **A probe runs whatever `claude` or `codex` is on the bridge's `PATH`.** That
  is the same program an agent session would exec; the probe adds no new
  program, only a new time it runs. On this machine those are mise wrapper
  scripts. A spawn runs them online and may install; a probe runs them offline
  (control 11). mise 2026.9.9 resolves `latest` to the newest *installed*
  version while any is installed (`mise use -g --dry-run claude` answers
  `2.1.280 already installed` with 2.1.284 released), so the install a probe
  could have started is the first one on a machine. Another version manager's
  wrapper is not told to stay offline.
- **The version a session reports is trusted only as a hint.** A differing
  `claude_code_version` or codex `userAgent` makes the bridge ask the CLI
  again; the answer that is kept is always the probe's own.
- **`min_cli` values are curated, not probed.** Each cites the Claude Code
  changelog entry it comes from. A wrong value can hide a model the CLI runs,
  or offer one it does not; it cannot run anything the user did not pick.

# Harness authentication security checklist

**Status:** inventory controls (#434) 100/100 (10/10 verified). Sign-in
controls (#432 tasks 2–6: login supervisor, pasted codes, challenges, typed
auth failures, SPA sheet) are added here as each lands; until then this
checklist covers the passive inventory only.
**Verified:** 2026-10-09
**Scope:** `bridge/src/harness/inventory/` (the service, its passive
adapters, its persistence), `bridge/src/api/v1/harnesses.rs`
(`harnesses.list`, `harnesses.refresh`), the `harnesses.changed` push
(`bridge/src/app/harness_inventory.rs`), and the probe-child changes in
`bridge/src/harness/installed/probe/child.rs` it reuses. The design is
#432/c/tc-01M4GNY74W8TET2QG52M504C1F.

The inventory reads the device's saved credentials for three CLIs and runs
one of them (`claude auth status`). Everything those files and that child
contain is untrusted and much of it is secret: OAuth access and refresh
tokens, API keys, account emails, organisation ids, config paths. What leaves
the inventory is enums, version strings, Pi provider ids and timestamps.

Unqualified test names live in `bridge/src/harness/inventory/tests.rs` (run
`cargo test harness::inventory` from `bridge/`); others name their module.
Every test uses fake CLIs and fixture credential files under a temporary
home; none touches a real CLI, account or network.

| # | Control | Score | Verification |
|---|---|---:|---|
| 1 | No credential value is read into anything that outlives the parse, and none reaches the snapshot, the push, the persisted file or a log. Token and key fields are parsed as presence only (`Present` skips the value); a Pi key is classified by its first characters in place; `claude auth status` is parsed into an allowlist (`loggedIn`, `authMethod`, `apiKeySource`) and the rest (email, organisation, paths) is dropped unread; probe output is never logged | 10/10 | `claude_status_is_read_through_its_allowlist_and_nothing_else_is_kept`, `an_expired_claude_access_token_with_a_refresh_token_is_refresh_pending`, `codex_is_read_from_its_saved_metadata_and_never_run`, `pi_lists_each_provider_without_resolving_keys_or_running_commands`, `claude_without_its_cli_reads_the_environment_by_name_only`, `a_restart_restores_nonsecret_observations_as_stale` (each asserts no `SECRET`, email or org string in the serialised snapshot or saved file), `adapters::tests::presence_skips_the_value`, `adapters::pi::tests::a_key_is_classified_by_its_first_characters_only` |
| 2 | No configured helper command ever runs. Claude Code's status command is not run at all while any settings file it reads (user, local, managed policy) names `apiKeyHelper`, `awsAuthRefresh`, `awsCredentialExport`, `gcpAuthRefresh` or `otelHeadersHelper` (the inspected 2.1.284 status path resolves an API key through `apiKeyHelper`); Pi's `!command` keys and `$VAR` keys are never resolved | 10/10 | `a_configured_claude_helper_is_never_run_and_neither_is_the_status_command` (helper and CLI both tripwired; also the managed-policy path), `pi_lists_each_provider_without_resolving_keys_or_running_commands` (the key command is tripwired) |
| 3 | Nothing signs in, logs out or refreshes a token. Codex and Pi are never run to observe them: `codex login status` prints part of an API key, and an app-server account read in the pinned 0.160.0 goes through the auth manager that refreshes, so Codex is metadata only; Pi has no passive status verb. `claude auth status` reads saved token sources synchronously (2.1.284 bundle: `ac()`/`Mu()`), with nonessential traffic and the autoupdater disabled. `harnesses.refresh` only schedules an observation | 10/10 | `codex_is_read_from_its_saved_metadata_and_never_run`, `pi_lists_each_provider_without_resolving_keys_or_running_commands` (both CLIs tripwired), `the_status_probe_runs_fixed_argv_from_home_without_agent_or_bridge_identity` (asserts `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1`), `a_refresh_answers_at_once_and_completes_with_the_next_sweep` |
| 4 | The one probe runs the PATH hit by absolute path with fixed argv (`auth status`), no shell, stdin closed, from the effective device home, in its own process group, under the 3 s probe deadline with bounded stdout and drained stderr (`probe/child.rs`, Installed CLI Probe checklist controls 2–3) | 10/10 | `the_status_probe_runs_fixed_argv_from_home_without_agent_or_bridge_identity` (argv and cwd), `ProbeChild::spawn` shares the deadline, bounds and group kill tested in `installed::probe::tests` |
| 5 | The probe's environment is the device environment less every agent marker, the daemon's identity and every `BRIDGE_*`/`BUILD_*` variable (MCP socket and token, Pi MCP owner), offline to mise and Pi; the version probes get the same | 10/10 | `the_status_probe_runs_fixed_argv_from_home_without_agent_or_bridge_identity` (the fake dumps its environment), `installed::probe::tests::bridge_and_build_variables_are_withheld_from_probes`, `installed::probe::tests::a_probe_tells_pi_to_stay_offline`, `harness::identity_env_tests::a_cli_probe_loses_the_identity` |
| 6 | Reads are bounded: a metadata file over 1 MiB, not a regular file, or not the expected JSON is not read; Pi provider ids reach the wire only when short and plain (`[a-z0-9][a-z0-9._-]{0,63}`) | 10/10 | `adapters::read_json` size and type checks, `pi_lists_each_provider_without_resolving_keys_or_running_commands` (an id with markup is dropped), `adapters::pi::tests::provider_ids_are_plain` |
| 7 | A status never claims more than was checked: every observation is `verification: saved_configuration`; access expiry with a refresh credential is `refresh_pending`, not expired; external and unresolvable sources are `unknown`; a CLI that answers nonsense keeps its prior facts marked `stale`, and nothing invents `not_installed` | 10/10 | `an_expired_claude_access_token_with_a_refresh_token_is_refresh_pending`, `a_configured_claude_helper_is_never_run_and_neither_is_the_status_command`, `codex_credentials_in_the_keyring_are_unknown_rather_than_absent`, `a_sign_in_check_runs_at_most_once_a_minute_and_backs_off_after_failures`, `adapters::tests::expiry_with_a_refresh_credential_is_pending_not_expired` |
| 8 | Probe frequency is bounded whatever a client does: one sweep thread, each context at most once a minute after success, exponential failure backoff to ten minutes, forced checks (refresh, file or executable change) no more often than every 5 s and coalesced; no RPC waits on a probe and no probe runs under any lock | 10/10 | `a_sign_in_check_runs_at_most_once_a_minute_and_backs_off_after_failures`, `a_refresh_answers_at_once_and_completes_with_the_next_sweep`, `installs_removals_and_retargeted_links_are_seen_and_check_sign_in_again`, `the_sweep_runs_with_no_client_connected`, `app::harness_inventory::a_greeted_session_lists_refreshes_and_hears_the_inventory_move` |
| 9 | Ordering: an older observation can never overwrite a newer one, and a restart restores observations stale with the revision continuing upward so a client's revision fence holds across it. The saved file is owner-only and replaced atomically | 10/10 | `an_older_observation_never_overwrites_a_newer_one`, `a_restart_restores_nonsecret_observations_as_stale` (asserts mode 0600) |
| 10 | The wire carries only the snapshot types: typed params refuse anything undeclared, `harnesses.changed` carries only its type and revision to greeted sessions and ends with them, and the scanners are clean | 10/10 | `app::harness_inventory::a_greeted_session_lists_refreshes_and_hears_the_inventory_move` (an undeclared param is refused), `api::v1::harnesses::tests::fixture_shapes_match_the_typed_inventory`, `tests/api_contract.rs` `the_harness_inventory_is_announced_with_its_push` and `every_event_example_is_what_the_bridge_serialises`, `rpc.rs` close frame calls `unsubscribe_harnesses_changed`; `semgrep --config auto` and `gitleaks` on the branch |

**Total: 100/100.**

## Boundary notes

- **"Signed in" is saved configuration, not a provider check.** No passive
  observation can tell a revoked token from a good one. The wire says so in
  `verification`; task 2 of #432 adds provider evidence from failed turns.
- **The Claude adapter trusts the inspected 2.1.284 status path.** A later
  Claude Code could resolve helpers or refresh in `auth status`; the helper
  gate is on settings Build reads, so a helper configured somewhere Build does
  not read (a project's own settings, which the probe never loads because it
  runs from the home directory) is out of reach either way.
- **Codex's API-key environment.** Only `CODEX_API_KEY` counts as an
  environment credential. `OPENAI_API_KEY` is commonly set for other tools and
  is not evidence Codex will use it.
- **Pi's environment table** is the 0.86.1 documented list. A provider whose
  variable is not listed there, or a custom provider in `models.json`, is not
  reported rather than guessed.

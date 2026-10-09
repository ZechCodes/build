# Harness authentication security checklist

**Status:** inventory controls (#434) 100/100 (9/9 verified). Sign-in
controls (#432 tasks 2–6: login supervisor, pasted codes, challenges, typed
auth failures, SPA sheet) are added here as each lands; until then this
checklist covers the passive inventory only.
**Verified:** 2026-10-09, revised after the #466 review (the first version's
Claude status probe, helper gate, missing-versus-malformed reads and Codex
config parsing were each shown unsafe there).
**Scope:** `bridge/src/harness/inventory/` (the service, its metadata-only
adapters, its persistence), `bridge/src/api/v1/harnesses.rs`
(`harnesses.list`, `harnesses.refresh`) and the `harnesses.changed` push
(`bridge/src/app/harness_inventory.rs`). The design is
#432/c/tc-01M4GNY74W8TET2QG52M504C1F.

The inventory reads the device's saved credentials and settings for three
CLIs. Everything those files contain is untrusted and much of it is secret:
OAuth access and refresh tokens, API keys, account ids. What leaves the
inventory is enums, version strings, Pi provider ids and timestamps. Its
authentication reads start no process, no CLI and no helper: none of the
three has a status path that can be shown passive through its startup (#466 traced Claude Code 2.1.284's `auth status`
through a root pre-action hook that can refresh OAuth and run policy
helpers; Codex's `login status` prints part of an API key and an app-server
account read goes through its refreshing auth manager; Pi has no status
verb). The installed versions it lists come from the existing CLI readings
(`harness::installed`), whose own sanitized version and model probes keep
running on their schedule under `Installed CLI Probe Security Checklist.md`
(fixed argv, 3 s deadline, bounded output, process-group kill, home
directory, withheld agent/bridge/Build variables, offline switches:
controls 1–3, 5 and 11 there); the inventory adds no probe of its own.

Unqualified test names live in `bridge/src/harness/inventory/tests.rs` (run
`cargo test harness::inventory` from `bridge/`); others name their module.
Every test uses fixture files and tripwire CLIs under a temporary home; none
touches a real CLI, account or network.

| # | Control | Score | Verification |
|---|---|---:|---|
| 1 | No credential value is kept past its parse or reaches the snapshot, the push, the persisted file or a log. Token and key fields are parsed as presence only (`Present` looks at a value in place and skips it; `null`, empty and whitespace-only values are absent, as are blank variables); a Pi key is classified by its first characters in place | 10/10 | `claude_is_read_from_its_saved_credentials_and_never_run`, `an_expired_claude_access_token_with_a_refresh_token_is_refresh_pending`, `claude_without_saved_credentials_reads_the_environment_by_name_only`, `codex_is_read_from_its_saved_metadata_and_never_run`, `pi_lists_each_provider_without_resolving_keys_or_running_commands`, `a_restart_restores_nonsecret_observations_as_stale` (snapshot, persisted file and push all free of the planted `SECRET` values), `adapters::tests::presence_skips_the_value`, `adapters::pi::tests::a_key_is_classified_by_its_first_characters_only`, `empty_or_blank_credential_values_count_as_absent` (Claude settings `env`, global config and credentials, Codex `auth.json` and `CODEX_API_KEY`, Pi keys, OAuth tokens and variables); the #466 reviewer's independent secret-export probe (#466/c/tc-01M4H8JVYP83F9AHEXYF9KYC2X) |
| 2 | No authentication read runs a CLI, helper or key command: Claude Code, Codex and Pi are not started to observe sign-in, and neither is an `apiKeyHelper`, a `policyHelper` or a Pi `!command` key | 10/10 | Tripwire CLIs and helpers that leave a marker if run: `claude_is_read_from_its_saved_credentials_and_never_run`, `a_claude_api_key_helper_in_any_settings_source_reads_as_external_and_never_runs`, `a_claude_policy_helper_never_runs`, `codex_is_read_from_its_saved_metadata_and_never_run`, `pi_lists_each_provider_without_resolving_keys_or_running_commands`, `installs_removals_and_retargeted_links_are_seen_and_check_sign_in_again` |
| 3 | Nothing signs in, logs out or refreshes a token: the adapters only read files and variable names, and `harnesses.refresh` only schedules a read | 10/10 | follows from control 2 (no process is started); `a_refresh_answers_at_once_and_completes_with_the_next_sweep` |
| 4 | Every source a Claude Code session started in the home directory signs in from is read: the credentials file under `CLAUDE_CONFIG_DIR` (OAuth tokens and expiry), the global config `.claude.json` (an API key saved by `/login` as `primaryApiKey`; in the config directory when `CLAUDE_CONFIG_DIR` is set), and, for an `apiKeyHelper` or a token or key variable in `env`, the user's settings under `CLAUDE_CONFIG_DIR`, the home's own project and local `.claude` settings even when the config directory is elsewhere, and managed policy with its sorted `managed-settings.d/*.json` drop-ins | 10/10 | `a_claude_api_key_helper_in_any_settings_source_reads_as_external_and_never_runs` (all four locations), `a_custom_claude_config_dir_still_reads_the_home_project_settings`, `a_claude_api_key_saved_by_login_reads_as_signed_in` (both global config locations); negative control: removing the drop-ins or the home project settings fails them |
| 5 | Reads fail closed: a metadata file is opened non-blocking and refused unless its descriptor is a regular file (a FIFO cannot stall the sweep), and read up to 1 MiB. A file that is missing means nothing is saved; one that is there but non-regular, oversized, unreadable or malformed fails the observation, so the prior facts stand, stale, and never become a fresh "signed out" | 10/10 | `a_fifo_in_place_of_a_metadata_file_never_blocks_the_sweep` (Codex `config.toml` and `auth.json`, Claude credentials and settings, Pi `auth.json`), `malformed_credential_files_keep_the_prior_facts_stale` (all three contexts), `unreadable_claude_settings_fail_the_observation_closed` (an oversized settings file); negative controls: dropping `O_NONBLOCK` and the regular-file check, or reading a malformed file as absent, fail them |
| 6 | Configuration is parsed as what it is: Codex's `config.toml` with a TOML parser (comments, literal strings, tables), so only the top-level `cli_auth_credentials_store` counts; Pi provider ids reach the wire only when short and plain | 10/10 | `codex_credentials_in_the_keyring_are_unknown_rather_than_absent` (a trailing comment, a literal string, and a table's same-named key that must not count), `pi_lists_each_provider_without_resolving_keys_or_running_commands` (an id with markup is dropped), `adapters::pi::tests::provider_ids_are_plain` |
| 7 | A status never claims more than was read: every observation is `verification: saved_configuration`; access expiry with a refresh credential is `refresh_pending`, not expired; helpers, cloud providers, keyring storage and unresolved keys are `unknown` | 10/10 | `an_expired_claude_access_token_with_a_refresh_token_is_refresh_pending`, `a_claude_api_key_helper_in_any_settings_source_reads_as_external_and_never_runs`, `codex_credentials_in_the_keyring_are_unknown_rather_than_absent`, `pi_lists_each_provider_without_resolving_keys_or_running_commands`, `adapters::tests::expiry_with_a_refresh_credential_is_pending_not_expired` |
| 8 | Observation frequency and ordering are bounded: one sweep thread that runs with no client; each context at most once a minute after success, failures backing off to ten minutes, forced checks (refresh, file or executable change) at most every 5 s and coalesced; nothing waits on an observation under `AppState`'s lock; an older observation never overwrites a newer one; a restart restores observations stale with the revision continuing upward, saved owner-only and replaced atomically | 10/10 | `the_sweep_runs_with_no_client_connected` (waits on the service's own change event), `a_sign_in_check_runs_at_most_once_a_minute_and_backs_off_after_failures`, `a_refresh_answers_at_once_and_completes_with_the_next_sweep`, `installs_removals_and_retargeted_links_are_seen_and_check_sign_in_again`, `an_older_observation_never_overwrites_a_newer_one`, `a_restart_restores_nonsecret_observations_as_stale` (mode 0600), `app::harness_inventory::a_greeted_session_lists_refreshes_and_hears_the_inventory_move` |
| 9 | The wire carries only the snapshot types: typed params refuse anything undeclared, and `harnesses.changed` carries only its type and revision to greeted sessions and ends with them | 10/10 | `app::harness_inventory::a_greeted_session_lists_refreshes_and_hears_the_inventory_move` (an undeclared param is refused), `api::v1::harnesses::tests::fixture_shapes_match_the_typed_inventory`, `tests/api_contract.rs` `the_harness_inventory_is_announced_with_its_push` and `every_event_example_is_what_the_bridge_serialises`, `rpc.rs` close frame calls `unsubscribe_harnesses_changed` |

**Total: 100/100.**

## Boundary notes

- **"Signed in" is saved configuration, not a provider check.** No passive
  observation can tell a revoked token from a good one. The wire says so in
  `verification`; task 2 of #432 adds provider evidence from failed turns.
- **Settings Build does not read.** A session started in a project reads that
  project's `.claude` settings too; the inventory reports the device default
  (a session started in the home directory). A project-only `apiKeyHelper`
  is that project's, outside this context.
- **Codex's API-key environment.** Only `CODEX_API_KEY` counts as an
  environment credential. `OPENAI_API_KEY` is commonly set for other tools and
  is not evidence Codex will use it.
- **Pi's environment table** is the 0.86.1 documented list. A provider whose
  variable is not listed there, or a custom provider in `models.json`, is not
  reported rather than guessed.
- **Platform credential stores.** Claude Code on macOS keeps its sign-in in
  the Keychain and Codex with `keyring` in the OS keyring; no file shows
  either, so both read as `unknown` rather than signed out, and the store
  itself is never read.

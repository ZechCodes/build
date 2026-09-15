# Rift isolation security checklist

Scope: Build's optional, user-installed Rift CLI adapter. Rift owns filesystem
cloning, initialization, its SQLite registry, and physical garbage collection.
These checks cover Build's integration, not an audit of Rift's implementation.

| Control | Evidence |
| --- | --- |
| CLI arguments are passed literally without a shell. | `provider_arguments_are_literal_and_failures_keep_the_exit_status` and the fake CLI lifecycle's exact argument assertions. |
| Capability checks do not initialize the source or create a registry. | `availability_validates_help_without_creating_storage_or_a_database` and `availability_rejects_an_incomplete_cli`. |
| Registry and checkout storage remain outside the source, including symlinks. | `availability_rejects_storage_inside_the_project`, `availability_resolves_a_symlink_before_checking_storage_ownership`, and `a_registry_symlink_into_the_project_is_refused_before_init`. |
| Git normalization cannot follow a copied Git directory or worktree pointer into the source. | `availability_rejects_a_symlinked_git_directory` and `a_copied_core_worktree_cannot_send_git_cleanup_into_the_source`. |
| Discovery and deletion respect Build ownership. | Foreign-marker and unowned-directory refusal regressions; removal/recreation through the real Rift CLI. |
| Missing or unreadable Rift state does not prevent ordinary worktrees. | `missing_cli_keeps_optional_walks_working_and_refuses_live_removal` and `an_unreadable_private_record_never_blocks_worktree_record_walks`. |
| Hooks are disabled and cleanup uses only Build's private registry. | Exact `--database`, `--no-hooks`, create/remove/GC coverage in the fake CLI lifecycle. |
| Provider timeouts stop child helpers and report failures. | `a_timed_out_provider_cannot_leave_a_helper_modifying_the_checkout`, bounded Git-process tests, and CLI stderr regression. |
| Creation through clones of one `WorktreeManager` serializes first-time initialization and branch preparation. | `cloned_managers_serialize_first_rift_initialization`; the shared lock covers branch preparation through materialization. |
| Task branch identity, ignored caches, publishing, and base synchronization survive the provider change. | Fake CLI lifecycle plus real Rift create/restore/finish/discovery/removal tests. |

## Validation environment

Real CLI testing uses upstream Rift commit
`757a22cb247f9b24a849c9d6bd56f49c0ec494f8` on Linux/XFS with native reflinks.
The executable is built for testing only; Build neither bundles nor installs it.
Rift tests use a temporary directory on that filesystem and report no skips.
An empty `XDG_CONFIG_HOME`, `GIT_CONFIG_GLOBAL=/dev/null`, and
`GIT_CONFIG_NOSYSTEM=1` isolate tests from this machine's Git diff-prefix setting.
SPA checks use Node 22.

## Final validation

- Rust formatting and strict Clippy: passed.
- Full Rust suite with real Rift on XFS: 2,250 passed, zero failures, seven
  existing tests intentionally ignored (external provider/store fixtures).
- Rift capability and lifecycle tests: no filesystem/provider skips.
- SPA: 3,775 tests passed; lint and production build passed under Node 22.
- Semgrep: 510 rules over 637 tracked targets, zero findings (~99.9% parsed).
- Gitleaks current-tree scan: zero leaks.

**Score: 100/100 for the ten integration controls above.**

macOS/APFS and Btrfs initialization have not been exercised on this machine. Upstream's
experimental CLI, including its private `--database` option, is an external
compatibility dependency. An interrupted provider operation can require manual
recovery; Build reports cleanup errors and never falls back to deleting an
unowned directory.

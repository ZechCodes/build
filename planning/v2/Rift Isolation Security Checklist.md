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


## #522 planned extension — not yet verified

These rows are proposed by [Project workspace](Project%20Workspace%20Spec.md).
Existing scores above cover their original implemented scope only. Each owning
child must supply passing regression/gate evidence before its new rows can
count toward 100/100; planning does not satisfy these controls.

| ID | Pending control | Required evidence |
| --- | --- | --- |
| RI522-1 | Project workers are explicitly unisolated; creating an isolated workspace preserves configured-base/local-commit and existing provider copy rules, never silently switches to current source HEAD. | C2/C10: workspace_cut_preserves_local_base_commits; existing Rift/worktree cut regressions. |
| RI522-2 | Project scratch owner and original sources never enter workspace reclaim; worker removal and whole-project deletion remove only owned artifacts and preserve source trees. | C5/C10: workspace_reclaim_never_reclaims_project_agents; project deletion with workers and shells. |

C5 evidence (#533; RI522-2 stays pending until C10 adds project deletion
with source shells): `workspace_reclaim_never_reclaims_project_agents` (the
owner is no reclaim subject, and reclaiming a named workspace leaves the pin,
a worker its agent made, that worker's session and the scratch),
`project_deletion_retires_workers_and_preserves_sources` and the end-to-end
`project_workers_take_tasks_hold_sources_and_leave_them_on_delete` (scratch
removed, sources and an uncommitted edit kept).

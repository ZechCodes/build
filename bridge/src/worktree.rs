//! Worktree lifecycle facade; `WorktreeManager` owns the single backend selector.
mod command;
mod comparison;
mod discovery;
mod identity;
mod manager;
mod mutation;

pub use crate::isolation::{
    branch_teardown, copy_directory, copy_directory_with_rift_root, BranchTeardown,
    ResolvedIsolation, WorktreeError,
};
pub(crate) use command::{
    bounded_git_fetch, configured_remote_for_branch, git_default_branch, git_in, git_remote_origin,
    git_stdout, remotes_match, unix_now,
};
pub(crate) use comparison::branch_comparison;
pub use discovery::{describe_checkout, primary_checkout_holder, sort_checkouts, ExternalWorktree};
pub use identity::{
    branch_name_for, canonical_planned_path, canonical_root, checked_out_branch,
    derive_adoption_goal, external_worktree_id, is_checkout_id, is_ref_name, is_usable_branch_name,
    rfc3339_from_unix, slugify, BRANCH_PREFIX,
};
pub use manager::WorktreeManager;
pub use mutation::{NamedBranchCheckout, UnregisteredRestore, Worktree};

#[cfg(test)]
mod tests;

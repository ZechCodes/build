//! Workspace-cut failures report whether their checkout and ref fully unwound.
use super::{NamedBranchCheckout, Worktree, WorktreeManager};
use crate::isolation::{
    record_branch_teardown, BranchTeardown, Isolation, IsolationBackend, WorktreeError,
};
use std::path::Path;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum CheckoutUnwind {
    #[default]
    NoEffects,
    Complete,
    Incomplete,
}
impl CheckoutUnwind {
    pub(crate) fn proves_absent(self) -> bool {
        matches!(self, Self::NoEffects | Self::Complete)
    }
}

impl WorktreeManager {
    /// The original failure is unchanged; the journal separately receives proof
    /// for its rollback gate. Every failure after ref creation must unwind both
    /// halves before this reports Complete.
    pub(crate) fn create_workspace_checkout_from_observed(
        &self,
        workspace_slug: &str,
        base_branch: &str,
        start: Option<&str>,
        destination: &Path,
        isolation: Isolation,
        report: impl FnOnce(CheckoutUnwind),
    ) -> Result<NamedBranchCheckout, WorktreeError> {
        let mut unwind = CheckoutUnwind::NoEffects;
        let result = self.cut_workspace_checkout(
            workspace_slug,
            base_branch,
            start,
            destination,
            isolation,
            &mut unwind,
        );
        if result.is_err() {
            report(unwind);
        }
        result
    }

    fn cut_workspace_checkout(
        &self,
        workspace_slug: &str,
        base_branch: &str,
        start: Option<&str>,
        destination: &Path,
        isolation: Isolation,
        unwind: &mut CheckoutUnwind,
    ) -> Result<NamedBranchCheckout, WorktreeError> {
        let _creation = self.lock_creation()?;
        if destination.exists() {
            return Err(WorktreeError::Refused(format!(
                "{} already exists",
                destination.display()
            )));
        }
        let backend = self.backend(isolation)?;
        let repo = git2::Repository::open(&self.repo_path)?;
        let base_commit = match start {
            Some(start) => repo.find_commit(git2::Oid::from_str(start)?)?,
            None => repo.revparse_single(base_branch)?.peel_to_commit()?,
        };
        let name = self.unique_checkout_name(workspace_slug, |candidate| {
            self.branch_taken(&repo, candidate)
        })?;
        let branch = self.branch_name(&name);
        repo.branch(&branch, &base_commit, false)?;
        *unwind = CheckoutUnwind::Incomplete;
        let materialized = self.materialize_workspace_cut(backend, destination, &branch);
        if let Err(error) = materialized {
            *unwind = self.unwind_workspace_checkout(
                backend,
                &repo,
                &branch,
                destination,
                "failed materialization",
            );
            return Err(error);
        }
        let teardown = BranchTeardown::DeletesBranch;
        let stamped = stamp_workspace_teardown(destination, &branch, teardown);
        if let Err(error) = stamped {
            *unwind = self.unwind_workspace_checkout(
                backend,
                &repo,
                &branch,
                destination,
                "after recording teardown failed",
            );
            return Err(error);
        }
        Ok(NamedBranchCheckout {
            worktree: Worktree {
                name,
                path: destination.to_path_buf(),
                recorded_branch: branch,
                base_branch: base_branch.to_string(),
            },
            teardown,
        })
    }

    fn materialize_workspace_cut(
        &self,
        backend: &dyn IsolationBackend,
        destination: &Path,
        branch: &str,
    ) -> Result<(), WorktreeError> {
        #[cfg(test)]
        super::workspace_checkout_test::at(
            super::workspace_checkout_test::Boundary::Materialize,
            destination,
            branch,
        )?;
        backend.materialize(&self.repo_path, branch, destination)
    }

    fn unwind_workspace_checkout(
        &self,
        backend: &dyn IsolationBackend,
        repo: &git2::Repository,
        branch: &str,
        destination: &Path,
        reason: &str,
    ) -> CheckoutUnwind {
        #[cfg(test)]
        super::workspace_checkout_test::at(
            super::workspace_checkout_test::Boundary::BeforeRemove,
            destination,
            branch,
        )
        .unwrap();
        let removed = backend.remove(&self.repo_path, destination);
        #[cfg(test)]
        super::workspace_checkout_test::at(
            super::workspace_checkout_test::Boundary::AfterRemove(removed.is_ok()),
            destination,
            branch,
        )
        .unwrap();
        if let Err(error) = &removed {
            eprintln!(
                "checkout {}: could not unwind {reason}: {error}",
                destination.display()
            );
        }
        // Always try both halves, even if the checkout removal failed.
        let ref_removed = discard_workspace_ref(repo, branch, destination);
        if let Err(error) = &ref_removed {
            eprintln!("branch {branch}: could not unwind workspace creation: {error}");
        }
        if removed.is_ok() && ref_removed.is_ok() {
            CheckoutUnwind::Complete
        } else {
            CheckoutUnwind::Incomplete
        }
    }
}

fn stamp_workspace_teardown(
    destination: &Path,
    branch: &str,
    teardown: BranchTeardown,
) -> Result<(), WorktreeError> {
    let _ = branch;
    #[cfg(test)]
    super::workspace_checkout_test::at(
        super::workspace_checkout_test::Boundary::Teardown,
        destination,
        branch,
    )?;
    record_branch_teardown(destination, teardown)
}

fn discard_workspace_ref(
    repo: &git2::Repository,
    branch: &str,
    _destination: &Path,
) -> Result<(), WorktreeError> {
    #[cfg(test)]
    super::workspace_checkout_test::at(
        super::workspace_checkout_test::Boundary::BeforeRefRemoval,
        _destination,
        branch,
    )?;
    let found = repo.find_branch(branch, git2::BranchType::Local);
    #[cfg(test)]
    super::workspace_checkout_test::at(
        super::workspace_checkout_test::Boundary::AfterRefLookup(
            found.as_ref().err().map(|error| error.code()),
        ),
        _destination,
        branch,
    )
    .unwrap();
    match found {
        Ok(mut created) => created.delete().map_err(WorktreeError::from),
        Err(error) if error.code() == git2::ErrorCode::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

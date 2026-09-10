use crate::isolation::{Isolation, ResolvedIsolation};
use crate::lifecycle::holders::{BranchHolder, ProjectCheckouts};
use crate::lifecycle::{
    adopt, DispatchReached, DispatchedCheckout, JoinedCheckout, Performed, WorktreeChange,
    WorktreeMutation,
};
use crate::models::ModelChoice;
use crate::orchestrator::{AdoptionScope, Orchestrator};
use crate::worktree::{ExternalWorktree, NamedBranchCheckout};

/// Tests only: where to fail a `branch.dispatch`, so the cleanup that has to
/// undo what the call created can be exercised at each seam it opens. One
/// variant is one seam, and every seam is checked through [`fail_dispatch_at`].
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BranchDispatchStep {
    /// The git phase, after the checkout is resolved or cut, before Build's
    /// ownership is written into it.
    Adopt,
    /// The git phase, after the checkpoint and the scaffold, before the run
    /// record exists.
    Own,
    /// The apply phase, under the mutex again, before the run this dispatch's
    /// checkout is opened around.
    Open,
    /// The apply phase, after the agent has been handed the instruction, before
    /// the write that makes its run real — the window where a turn is queued
    /// for a run the store never got.
    Settle,
    /// A branch Build already runs: before its new agent and first message land.
    Post,
}

/// Fail a dispatch where a test asked it to. One carrier for the injected
/// fault, so a seam cannot be checked in one arm and forgotten in another.
#[cfg(test)]
pub fn fail_dispatch_at(
    fault: Option<BranchDispatchStep>,
    step: BranchDispatchStep,
) -> Result<(), String> {
    if fault == Some(step) {
        return Err(format!("branch.dispatch: injected failure at {step:?}"));
    }
    Ok(())
}

/// Where one dispatch's work goes, named before any git runs.
///
/// A branch the caller spelled out is used exactly as it stands — re-deriving a
/// name from a name is how `build/csv-export` became `build/build-csv-export`.
/// Anything else is words about the work (the router's guess, or the
/// instruction itself when no branch was named), and words are slugified into
/// Build's namespace.
///
/// It is settled under the app mutex because it is what one dispatch reserves
/// against another: two calls that name the same ref must collide on the board
/// rather than in `git worktree add`.
pub enum DispatchTarget {
    /// The caller named a ref. The checkout already on it is where the work
    /// goes; with none, the ref is cut exactly as given.
    Named(String),
    /// Nobody named a ref, so the words did. The branch is this call's own, and
    /// nothing already on disk is taken over into it.
    Minted { branch: String, slug: String },
}

impl DispatchTarget {
    /// Read the ref out of what the caller said. Words with nothing to name a
    /// branch after are refused here, before anything is reserved or cut.
    pub fn of(branch: Option<&str>, instruction: &str) -> Result<DispatchTarget, String> {
        if let Some(name) = branch.filter(|name| crate::worktree::is_usable_branch_name(name)) {
            return Ok(DispatchTarget::Named(name.to_string()));
        }
        let words = branch.unwrap_or(instruction);
        if !words.chars().any(|c| c.is_ascii_alphanumeric()) {
            return Err(format!(
                "branch.dispatch: {words:?} has no letter or number to name a branch after"
            ));
        }
        let slug = crate::worktree::slugify(words);
        Ok(DispatchTarget::Minted {
            branch: crate::worktree::branch_name_for(&slug),
            slug,
        })
    }

    /// The ref this dispatch claims: what the board's row stands for, and what
    /// a second claim on it collides with.
    pub fn branch(&self) -> &str {
        match self {
            DispatchTarget::Named(branch) => branch,
            DispatchTarget::Minted { branch, .. } => branch,
        }
    }

    /// The branch whose existing checkout this dispatch takes over, when there
    /// is one to take: a ref the caller named may already have work in it, while
    /// a ref minted from words is this call's alone.
    pub fn adopted_branch(&self) -> Option<&str> {
        match self {
            DispatchTarget::Named(branch) => Some(branch),
            DispatchTarget::Minted { .. } => None,
        }
    }

    /// Add the checkout this dispatch's work goes in. A named ref is cut under
    /// that exact name (or checked out, when it is already a branch); a minted
    /// one goes through the slug namespace, which suffixes rather than collides.
    fn cut(
        &self,
        project: &Orchestrator,
        base_branch: &str,
        isolation: Isolation,
    ) -> Result<NamedBranchCheckout, String> {
        match self {
            DispatchTarget::Named(branch) => project
                .create_worktree_cutting_named_branch(branch, base_branch, isolation)
                .map_err(|error| error.to_string()),
            DispatchTarget::Minted { slug, .. } => project
                .create_bare_worktree(slug, base_branch, isolation)
                .map_err(|error| error.to_string()),
        }
    }
}

/// `branch.dispatch` — reach the checkout the instruction is meant for: the
/// bare one already on that branch, or one cut for it. Whichever it is, the
/// checkout comes back checkpointed and scaffolded, ready for the run the
/// epilogue opens around it.
pub struct DispatchCheckout {
    pub project: Orchestrator,
    pub project_id: String,
    pub base_branch: String,
    pub run_id: String,
    /// The ref this dispatch claims, settled before any git ran.
    pub target: DispatchTarget,
    pub checkouts: ProjectCheckouts,
    pub model_choice: ModelChoice,
    /// How a checkout this dispatch has to cut is made. A fallback is said on
    /// the dispatched run's conversation, and only when this dispatch cut a
    /// checkout of its own.
    pub resolved: ResolvedIsolation,
    #[cfg(test)]
    pub fault: Option<BranchDispatchStep>,
}

impl WorktreeMutation for DispatchCheckout {
    type Output = DispatchReached;
    fn perform(mut self) -> Result<Performed<Self::Output>, String> {
        if let Some(branch) = self.target.adopted_branch() {
            let ownership = self.checkouts.holders()?;
            let holder = BranchHolder::of(&ownership, branch);
            if let Some(run_id) = holder.held_by(crate::branch::BranchSource::Run) {
                return Ok(Performed {
                    change: WorktreeChange::nothing(),
                    output: DispatchReached::Joined(JoinedCheckout {
                        run_id: run_id.to_string(),
                        branch: branch.to_string(),
                        root: crate::worktree::canonical_root(
                            &self
                                .checkouts
                                .run_checkouts
                                .iter()
                                .find(|(id, _)| id == run_id)
                                .expect("the holder came from this snapshot")
                                .1
                                .path,
                        ),
                    }),
                });
            }
            if holder
                .held_by(crate::branch::BranchSource::PrimaryCheckout)
                .is_some()
            {
                return Err(holder
                    .refusal(branch)
                    .expect("the primary holds this branch"));
            }
            if let Some(found) = ownership
                .external
                .iter()
                .find(|checkout| checkout.branch.as_deref() == Some(branch))
            {
                return self.take_ownership(found, None);
            }
        }
        // A checkout that was already there is never this call's to remove:
        // taking ownership of one touches nothing that has to be put back.
        let minted = self.cut_branch()?;
        let described = self
            .project
            .describe_checkout(&minted.worktree.path, &self.base_branch)
            .map_err(|error| error.to_string());
        let downgrade = self.resolved.downgrade.take();
        let dispatched = match described {
            Ok(checkout) => self.take_ownership(&checkout, downgrade),
            Err(error) => Err(error),
        };
        if dispatched.is_err() {
            // What this call cut, this call removes — and the branch under it
            // only if this call cut that too.
            self.project
                .discard_checkout(&minted.worktree, !minted.teardown.deletes_branch());
        }
        dispatched
    }
}

impl DispatchCheckout {
    /// Cut the branch this dispatch has nowhere else to put its work.
    fn cut_branch(&self) -> Result<NamedBranchCheckout, String> {
        self.target
            .cut(&self.project, &self.base_branch, self.resolved.isolation)
    }

    /// Take Build's ownership of the checkout this dispatch reached, and owe the
    /// apply phase the instruction on top of it. `downgrade` is the resolver's
    /// sentence for a checkout this dispatch cut; a checkout that was already
    /// there was made by nobody's setting and carries none.
    fn take_ownership(
        &mut self,
        checkout: &ExternalWorktree,
        downgrade: Option<String>,
    ) -> Result<Performed<DispatchReached>, String> {
        #[cfg(test)]
        fail_dispatch_at(self.fault, BranchDispatchStep::Adopt)?;
        let adopted = adopt(
            &self.project,
            &self.project_id,
            checkout,
            &self.base_branch,
            AdoptionScope::ExternalWorktree,
            &self.run_id,
            self.model_choice.clone(),
        )?;
        #[cfg(test)]
        fail_dispatch_at(self.fault, BranchDispatchStep::Own)?;
        Ok(Performed {
            change: WorktreeChange::appeared(checkout.clone()),
            output: DispatchReached::Adopted(DispatchedCheckout {
                adoption: adopted,
                downgrade,
            }),
        })
    }
}

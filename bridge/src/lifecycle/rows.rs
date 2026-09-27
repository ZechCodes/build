use crate::isolation::Isolation;
use crate::worktree::ExternalWorktree;
use std::path::PathBuf;
use std::time::Instant;

#[derive(Default)]
pub struct WorktreeChange {
    pub appeared: Vec<ExternalWorktree>,
    pub gone: Vec<PathBuf>,
    /// The mutation changed a checkout it could not describe, so the amendment
    /// is not enough and the project's scan is claimed instead.
    pub rescan: bool,
}

impl WorktreeChange {
    pub fn nothing() -> Self {
        WorktreeChange::default()
    }

    pub fn appeared(worktree: ExternalWorktree) -> Self {
        WorktreeChange {
            appeared: vec![worktree],
            ..WorktreeChange::default()
        }
    }

    /// A checkout is on disk that nothing could describe. The board finds it
    /// through the scan this claims rather than through an amendment.
    pub fn undescribed() -> Self {
        WorktreeChange {
            rescan: true,
            ..WorktreeChange::default()
        }
    }
}

/// One finished job, on its way back under the app mutex.
pub struct PendingRow {
    /// The id the settled record is expected to carry — for a create, the
    /// checkout id its path will hash to; for a dispatch, the run it opens.
    pub entity_id: String,
    /// The project whose board renders this row. A project verb has none: there
    /// is no project id until its git lands, and what it settles into is the
    /// project itself rather than a card, so nothing renders it.
    pub project_id: Option<String>,
    pub title: String,
    pub branch: Option<String>,
    pub state: PendingState,
    pub checkout_id: Option<String>,
    pub implements: Option<String>,
    /// How the checkout this verb is making will be isolated, resolved before
    /// the row was reserved. A verb that makes no checkout of its own — a
    /// discard, an adoption of one already on disk — has none: what such a
    /// checkout is is read off the checkout itself.
    pub isolation: Option<Isolation>,
    pub since: Instant,
}

impl PendingRow {
    /// A card the board is about to have: the run or checkout `entity_id` names
    /// once the git lands.
    pub fn creating(entity_id: String, project_id: Option<String>, title: String) -> PendingRow {
        PendingRow::of(entity_id, project_id, title, PendingState::Creating)
    }

    /// A card the board is about to lose.
    pub fn discarding(entity_id: String, project_id: Option<String>, title: String) -> PendingRow {
        PendingRow::of(entity_id, project_id, title, PendingState::Discarding)
    }

    /// A row for the directory a project verb reached for, which is the only
    /// identity two of them share before either has a project id: two clones
    /// into one folder, or two remotes written into one repository, collide
    /// here. No card stands where it does, so no board renders it.
    pub fn on_directory(entity_id: String, title: String, state: PendingState) -> PendingRow {
        PendingRow::of(entity_id, None, title, state)
    }

    fn of(
        entity_id: String,
        project_id: Option<String>,
        title: String,
        state: PendingState,
    ) -> PendingRow {
        PendingRow {
            entity_id,
            project_id,
            title,
            branch: None,
            state,
            checkout_id: None,
            implements: None,
            isolation: None,
            since: Instant::now(),
        }
    }

    /// How the checkout this verb is about to make is isolated — the answer the
    /// app resolved before reserving the row, so the board reads the same fact
    /// off the row it will read off the card.
    pub fn isolated_as(self, isolation: Isolation) -> PendingRow {
        PendingRow {
            isolation: Some(isolation),
            ..self
        }
    }

    /// The branch this verb is claiming. A second verb claiming the same branch
    /// is refused while this row stands: the checkout it would work in does not
    /// exist yet, so the branch is the only identity the two share.
    pub fn on_branch(self, branch: String) -> PendingRow {
        PendingRow {
            branch: Some(branch),
            ..self
        }
    }

    /// The existing card this verb acts on: the state is rendered on that card
    /// rather than as a second row, and a second verb reaching for the same
    /// checkout is refused while this row stands.
    pub fn on_checkout(self, checkout_id: String) -> PendingRow {
        PendingRow {
            checkout_id: Some(checkout_id),
            ..self
        }
    }

    /// The Task this verb is opening an implementation of. A Task has one
    /// active writer, and the run that will be it is not in the run map until
    /// the git has landed — so the row is the gate for the length of that git,
    /// and a second implementation of the same Task is refused while it
    /// stands.
    pub fn implementing(self, task_id: String) -> PendingRow {
        PendingRow {
            implements: Some(task_id),
            ..self
        }
    }
}

/// What is happening to the row. Rendered, never branched on.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PendingState {
    Creating,
    Discarding,
    /// A record that already exists is being rewritten in place. Nothing is
    /// minted and nothing is taken away; what the row holds is the right to be
    /// the one verb rewriting it.
    Updating,
}

impl PendingState {
    pub fn as_str(self) -> &'static str {
        match self {
            PendingState::Creating => "creating",
            PendingState::Discarding => "discarding",
            PendingState::Updating => "updating",
        }
    }
}

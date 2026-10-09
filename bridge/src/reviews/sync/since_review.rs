//! Commits since the user's last reviewed head, under a traversal budget so
//! one large history cannot stall the serial sync worker (#453).
use git2::{Oid, Repository};

/// Commits examined at most per calculation; past it the count is absent.
pub(super) const WALK_LIMIT: usize = 2000;

#[derive(Debug, PartialEq, Eq)]
pub(super) enum SinceReview {
    /// The reviewed head is an ancestor; this many commits came after it.
    Count(u64),
    /// The reviewed head is no longer an ancestor of the current head.
    Rewritten,
}

#[cfg(test)]
thread_local! { static WALKS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) }; }

#[cfg(test)]
pub(crate) fn walks() -> u64 {
    WALKS.with(std::cell::Cell::get)
}

/// `None` when the budget runs out or a commit is unreadable.
pub(super) fn since_review(
    repository: &Repository,
    head: Oid,
    reviewed: Oid,
    limit: usize,
) -> Option<SinceReview> {
    #[cfg(test)]
    WALKS.with(|walks| walks.set(walks.get() + 1));
    if head == reviewed {
        return Some(SinceReview::Count(0));
    }
    let mut walk = Walk::new(repository, limit);
    walk.paint(head, AFTER)?;
    walk.paint(reviewed, REVIEWED)?;
    walk.run()?;
    Some(walk.outcome(reviewed))
}

const AFTER: u8 = 1;
const REVIEWED: u8 = 2;

/// Git's two-colour, newest-first walk: commits painted only from the head
/// are the ones since review; it stops once every queued commit is reachable
/// from the reviewed head, or when the budget is spent.
struct Walk<'r> {
    repository: &'r Repository,
    budget: usize,
    colours: std::collections::HashMap<Oid, u8>,
    queue: std::collections::BinaryHeap<(i64, Oid)>,
}

impl<'r> Walk<'r> {
    fn new(repository: &'r Repository, budget: usize) -> Self {
        Self {
            repository,
            budget,
            colours: Default::default(),
            queue: Default::default(),
        }
    }

    fn paint(&mut self, oid: Oid, colour: u8) -> Option<()> {
        let painted = self.colours.entry(oid).or_default();
        if *painted & colour == colour {
            return Some(());
        }
        *painted |= colour;
        let time = self.repository.find_commit(oid).ok()?.time().seconds();
        self.queue.push((time, oid));
        Some(())
    }

    fn only_after(&self, oid: &Oid) -> bool {
        self.colours.get(oid) == Some(&AFTER)
    }

    fn run(&mut self) -> Option<()> {
        while self.queue.iter().any(|(_, oid)| self.only_after(oid)) {
            let (_, oid) = self.queue.pop()?;
            self.budget = self.budget.checked_sub(1)?;
            let colour = self.colours[&oid];
            let commit = self.repository.find_commit(oid).ok()?;
            for parent in commit.parent_ids() {
                self.paint(parent, colour)?;
            }
        }
        Some(())
    }

    fn outcome(&self, reviewed: Oid) -> SinceReview {
        if self.colours[&reviewed] & AFTER == 0 {
            return SinceReview::Rewritten;
        }
        let count = self.colours.values().filter(|&&c| c == AFTER).count();
        SinceReview::Count(count as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git_fixture::{git_in, init_repo};
    use std::path::Path;

    fn commit(repo: &Path, name: &str) -> Oid {
        std::fs::write(repo.join(name), name).unwrap();
        git_in(repo, &["add", name]);
        git_in(repo, &["commit", "-m", name]);
        head(repo)
    }
    fn head(repo: &Path) -> Oid {
        Repository::open(repo)
            .unwrap()
            .head()
            .unwrap()
            .target()
            .unwrap()
    }

    #[test]
    fn counts_linear_and_merged_history_like_rev_list() {
        let (_dir, path) = init_repo();
        let reviewed = head(&path);
        commit(&path, "a");
        commit(&path, "b");
        git_in(
            &path,
            &["checkout", "-b", "side", reviewed.to_string().as_str()],
        );
        commit(&path, "c");
        commit(&path, "d");
        git_in(&path, &["checkout", "-"]);
        git_in(&path, &["merge", "--no-ff", "-m", "merge", "side"]);
        let repo = Repository::open(&path).unwrap();
        let tip = head(&path);
        assert_eq!(
            since_review(&repo, tip, reviewed, WALK_LIMIT),
            Some(SinceReview::Count(5))
        );
        assert_eq!(
            since_review(&repo, reviewed, reviewed, WALK_LIMIT),
            Some(SinceReview::Count(0))
        );
    }

    #[test]
    fn a_reviewed_head_off_the_history_is_rewritten() {
        let (_dir, path) = init_repo();
        let old = commit(&path, "old");
        git_in(&path, &["commit", "--amend", "-m", "rewritten"]);
        let repo = Repository::open(&path).unwrap();
        assert_eq!(
            since_review(&repo, head(&path), old, WALK_LIMIT),
            Some(SinceReview::Rewritten)
        );
    }

    #[test]
    fn the_budget_bounds_the_walk_and_leaves_the_count_absent() {
        let (_dir, path) = init_repo();
        let reviewed = head(&path);
        for name in ["a", "b", "c", "d"] {
            commit(&path, name);
        }
        let repo = Repository::open(&path).unwrap();
        assert_eq!(since_review(&repo, head(&path), reviewed, 3), None);
        assert_eq!(
            since_review(&repo, head(&path), reviewed, 6),
            Some(SinceReview::Count(4))
        );
        let old = commit(&path, "old");
        git_in(&path, &["commit", "--amend", "-m", "rewritten"]);
        assert_eq!(
            since_review(&repo, head(&path), old, 2),
            None,
            "an exhausted budget is not proof of a rewrite"
        );
    }
}

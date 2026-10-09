//! Commits since the user's last reviewed head, under a commit budget so one
//! large history cannot stall the serial sync worker (#453).
use git2::{Oid, Repository};
use std::collections::{BinaryHeap, HashMap};

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
    walk.paint(head, AFTER, false)?;
    walk.paint(reviewed, REVIEWED, false)?;
    walk.run()?;
    Some(walk.outcome(reviewed))
}

const AFTER: u8 = 1;
const REVIEWED: u8 = 2;
const BOTH: u8 = AFTER | REVIEWED;

#[cfg(test)]
thread_local! { static LOOKUPS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) }; }

/// libgit2's ahead/behind paint: commits reached only from the head are the
/// ones since review. A commit popped with both colours marks its parents
/// stale, and the walk ends once only stale commits are queued, so a commit
/// first reached from the head is still repainted when the reviewed side
/// arrives later, whatever the timestamps. Every commit lookup spends one unit
/// of the budget, parents of a wide merge included.
struct Walk<'r> {
    repository: &'r Repository,
    budget: usize,
    commits: HashMap<Oid, Painted>,
    queue: BinaryHeap<(i64, Oid)>,
}

struct Painted {
    time: i64,
    parents: Vec<Oid>,
    colour: u8,
    stale: bool,
}

impl<'r> Walk<'r> {
    fn new(repository: &'r Repository, budget: usize) -> Self {
        Self {
            repository,
            budget,
            commits: HashMap::new(),
            queue: BinaryHeap::new(),
        }
    }

    fn load(&mut self, oid: Oid) -> Option<Painted> {
        self.budget = self.budget.checked_sub(1)?;
        #[cfg(test)]
        LOOKUPS.with(|lookups| lookups.set(lookups.get() + 1));
        let commit = self.repository.find_commit(oid).ok()?;
        Some(Painted {
            time: commit.time().seconds(),
            parents: commit.parent_ids().collect(),
            colour: 0,
            stale: false,
        })
    }

    fn paint(&mut self, oid: Oid, colour: u8, stale: bool) -> Option<()> {
        if !self.commits.contains_key(&oid) {
            let loaded = self.load(oid)?;
            self.commits.insert(oid, loaded);
        }
        let painted = self.commits.get_mut(&oid)?;
        let changed = painted.colour | colour != painted.colour || (stale && !painted.stale);
        painted.colour |= colour;
        painted.stale |= stale;
        if changed {
            self.queue.push((painted.time, oid));
        }
        Some(())
    }

    fn run(&mut self) -> Option<()> {
        while self.queue.iter().any(|(_, oid)| !self.commits[oid].stale) {
            let (_, oid) = self.queue.pop()?;
            let painted = &self.commits[&oid];
            let (colour, parents) = (painted.colour, painted.parents.clone());
            let stale = painted.stale || colour == BOTH;
            for parent in parents {
                self.paint(parent, colour, stale)?;
            }
        }
        Some(())
    }

    fn outcome(&self, reviewed: Oid) -> SinceReview {
        if self.commits[&reviewed].colour & AFTER == 0 {
            return SinceReview::Rewritten;
        }
        let count = self.commits.values().filter(|c| c.colour == AFTER).count();
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

    /// ancestor ← reviewed, ancestor ← side, merge of [reviewed, side].
    fn merge_fixture(repo: &Repository, times: [i64; 4], salt: usize) -> (Oid, Oid) {
        let tree = repo.head().unwrap().peel_to_tree().unwrap();
        let at = |label: &str, seconds, parents: &[Oid]| {
            let sig =
                git2::Signature::new("t", "t@example.com", &git2::Time::new(seconds, 0)).unwrap();
            let parents: Vec<_> = parents
                .iter()
                .map(|p| repo.find_commit(*p).unwrap())
                .collect();
            let parents: Vec<_> = parents.iter().collect();
            let message = format!("{label} {salt}");
            repo.commit(None, &sig, &sig, &message, &tree, &parents)
                .unwrap()
        };
        let ancestor = at("ancestor", times[0], &[]);
        let reviewed = at("reviewed", times[1], &[ancestor]);
        let side = at("side", times[2], &[ancestor]);
        (at("merge", times[3], &[reviewed, side]), reviewed)
    }

    fn assert_counts_like_libgit2(repo: &Repository, merge: Oid, reviewed: Oid) {
        let (ahead, _) = repo.graph_ahead_behind(merge, reviewed).unwrap();
        assert_eq!(ahead, 2);
        assert_eq!(
            since_review(repo, merge, reviewed, WALK_LIMIT),
            Some(SinceReview::Count(ahead as u64))
        );
    }

    #[test]
    fn clock_skew_counts_like_graph_ahead_behind() {
        let (_dir, path) = init_repo();
        let repo = Repository::open(&path).unwrap();
        let (merge, reviewed) = merge_fixture(&repo, [100, 10, 110, 120], 0);
        assert_counts_like_libgit2(&repo, merge, reviewed);
    }

    #[test]
    fn same_second_merges_count_like_graph_ahead_behind() {
        let (_dir, path) = init_repo();
        let repo = Repository::open(&path).unwrap();
        for salt in 0..32 {
            let (merge, reviewed) = merge_fixture(&repo, [100; 4], salt);
            assert_counts_like_libgit2(&repo, merge, reviewed);
        }
    }

    #[test]
    fn the_budget_caps_lookups_on_a_wide_merge() {
        let (_dir, path) = init_repo();
        let repo = Repository::open(&path).unwrap();
        let reviewed = head(&path);
        let base = repo.find_commit(reviewed).unwrap();
        let tree = base.tree().unwrap();
        let sig = repo.signature().unwrap();
        let sides: Vec<_> = (0..60)
            .map(|n| {
                let oid = repo
                    .commit(None, &sig, &sig, &format!("side {n}"), &tree, &[&base])
                    .unwrap();
                repo.find_commit(oid).unwrap()
            })
            .collect();
        let parents: Vec<_> = sides.iter().collect();
        let merge = repo
            .commit(None, &sig, &sig, "wide", &tree, &parents)
            .unwrap();
        let before = LOOKUPS.with(std::cell::Cell::get);
        assert_eq!(since_review(&repo, merge, reviewed, 20), None);
        let lookups = LOOKUPS.with(std::cell::Cell::get) - before;
        assert!(lookups <= 21, "{lookups} lookups for a budget of 20");
        assert_eq!(
            since_review(&repo, merge, reviewed, WALK_LIMIT),
            Some(SinceReview::Count(61))
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
        commit(&path, "e");
        commit(&path, "f");
        assert_eq!(
            since_review(&repo, head(&path), old, 2),
            None,
            "an exhausted budget is not proof of a rewrite"
        );
        assert_eq!(
            since_review(&repo, head(&path), old, WALK_LIMIT),
            Some(SinceReview::Rewritten)
        );
    }
}

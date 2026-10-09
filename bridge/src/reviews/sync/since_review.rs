//! Commits since the user's last reviewed head, under a commit budget so one
//! large history cannot stall the serial sync worker (#453).
use git2::{Oid, Repository};
use std::collections::HashMap;

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
    Some(walk.outcome(head, reviewed))
}

const AFTER: u8 = 1;
const REVIEWED: u8 = 2;
const BOTH: u8 = AFTER | REVIEWED;

#[cfg(test)]
thread_local! { static LOOKUPS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) }; }

/// libgit2's ahead/behind (`graph.c`, 1.9): the paint of `mark_parents`,
/// then the count of `ahead_behind`. A commit popped with both colours marks
/// its parents stale, and the paint ends once every queued commit and every
/// root it popped is stale. Every commit lookup spends one unit of the budget,
/// parents of a wide merge included.
struct Walk<'r> {
    repository: &'r Repository,
    budget: usize,
    commits: HashMap<Oid, Painted>,
    queue: TimeQueue,
    roots: Vec<Oid>,
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
            queue: TimeQueue::default(),
            roots: Vec::new(),
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
            self.queue.insert(oid, &self.commits);
        }
        Some(())
    }

    fn run(&mut self) -> Option<()> {
        // The count follows libgit2's walk and stop rule, not `git rev-list`:
        // they can disagree, for example when commit times are skewed (#462).
        while self.interesting() {
            let Some(oid) = self.queue.pop(&self.commits) else {
                break;
            };
            let painted = &self.commits[&oid];
            let (colour, parents) = (painted.colour, painted.parents.clone());
            let stale = painted.stale || colour == BOTH;
            if parents.is_empty() {
                self.roots.push(oid);
            }
            for parent in parents {
                self.paint(parent, colour, stale)?;
            }
        }
        Some(())
    }

    fn interesting(&self) -> bool {
        let fresh = |oid: &Oid| !self.commits[oid].stale;
        self.queue.items.iter().any(fresh) || self.roots.iter().any(fresh)
    }

    /// `ahead_behind`: head-only commits reachable from either tip without
    /// passing through a commit painted both colours.
    fn outcome(&self, head: Oid, reviewed: Oid) -> SinceReview {
        if self.commits[&reviewed].colour & AFTER == 0 {
            return SinceReview::Rewritten;
        }
        let mut seen = std::collections::HashSet::new();
        let mut pending = vec![head, reviewed];
        let mut count = 0;
        while let Some(oid) = pending.pop() {
            let Some(painted) = self.commits.get(&oid) else {
                continue;
            };
            if painted.colour == BOTH || !seen.insert(oid) {
                continue;
            }
            count += u64::from(painted.colour == AFTER);
            pending.extend(&painted.parents);
        }
        SinceReview::Count(count)
    }
}

/// libgit2's `git_pqueue` under `git_commit_list_time_cmp`: newest first, and
/// equal times compare equal, so ties fall where its sift rules leave them.
#[derive(Default)]
struct TimeQueue {
    items: Vec<Oid>,
}

impl TimeQueue {
    /// Positive when `a` belongs below `b`, as in libgit2.
    fn cmp(commits: &HashMap<Oid, Painted>, a: Oid, b: Oid) -> std::cmp::Ordering {
        commits[&b].time.cmp(&commits[&a].time)
    }

    fn insert(&mut self, oid: Oid, commits: &HashMap<Oid, Painted>) {
        self.items.push(oid);
        let mut at = self.items.len() - 1;
        while at > 0 {
            let parent = (at - 1) >> 1;
            if Self::cmp(commits, self.items[parent], oid).is_le() {
                break;
            }
            self.items[at] = self.items[parent];
            at = parent;
        }
        self.items[at] = oid;
    }

    fn pop(&mut self, commits: &HashMap<Oid, Painted>) -> Option<Oid> {
        let top = *self.items.first()?;
        let last = self.items.pop()?;
        if !self.items.is_empty() {
            self.items[0] = last;
            self.sift_down(commits);
        }
        Some(top)
    }

    fn sift_down(&mut self, commits: &HashMap<Oid, Painted>) {
        let moving = self.items[0];
        let mut at = 0;
        loop {
            let mut kid = (at << 1) + 1;
            let Some(&left) = self.items.get(kid) else {
                break;
            };
            let mut child = left;
            if let Some(&right) = self.items.get(kid + 1) {
                if Self::cmp(commits, left, right).is_gt() {
                    child = right;
                    kid += 1;
                }
            }
            if Self::cmp(commits, moving, child).is_le() {
                break;
            }
            self.items[at] = child;
            at = kid;
        }
        self.items[at] = moving;
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

    fn commit_at(repo: &Repository, message: &str, seconds: i64, parents: &[Oid]) -> Oid {
        let tree = repo.head().unwrap().peel_to_tree().unwrap();
        let sig = git2::Signature::new("t", "t@example.com", &git2::Time::new(seconds, 0)).unwrap();
        let parents: Vec<_> = parents
            .iter()
            .map(|p| repo.find_commit(*p).unwrap())
            .collect();
        let parents: Vec<_> = parents.iter().collect();
        repo.commit(None, &sig, &sig, message, &tree, &parents)
            .unwrap()
    }

    /// ancestor ← reviewed, ancestor ← side, merge of [reviewed, side].
    fn merge_fixture(repo: &Repository, times: [i64; 4], salt: usize) -> (Oid, Oid) {
        let at = |label: &str, seconds, parents: &[Oid]| {
            commit_at(repo, &format!("{label} {salt}"), seconds, parents)
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
    fn a_root_reached_first_from_the_head_is_repainted_before_the_walk_ends() {
        let (_dir, path) = init_repo();
        let repo = Repository::open(&path).unwrap();
        let root = commit_at(&repo, "root", 100, &[]);
        let intermediate = commit_at(&repo, "intermediate", 1, &[root]);
        let reviewed = commit_at(&repo, "reviewed", 10, &[intermediate]);
        let side = commit_at(&repo, "side", 110, &[root]);
        let merge = commit_at(&repo, "merge", 120, &[reviewed, side]);
        assert_counts_like_libgit2(&repo, merge, reviewed);
    }

    /// Every head/baseline pair of a small DAG (parents by index) matches
    /// libgit2: the count when the baseline is an ancestor, else a rewrite.
    fn assert_dag_matches_libgit2(times: [i64; 12], edges: &[&[usize]]) {
        let (_dir, path) = init_repo();
        let repo = Repository::open(&path).unwrap();
        let mut nodes = Vec::new();
        for (n, parents) in edges.iter().enumerate() {
            let parents: Vec<_> = parents.iter().map(|&p| nodes[p]).collect();
            nodes.push(commit_at(&repo, &format!("node {n}"), times[n], &parents));
        }
        for &head in &nodes {
            for &reviewed in &nodes {
                let expected = if head == reviewed {
                    SinceReview::Count(0)
                } else if repo.graph_descendant_of(head, reviewed).unwrap() {
                    let (ahead, _) = repo.graph_ahead_behind(head, reviewed).unwrap();
                    SinceReview::Count(ahead as u64)
                } else {
                    SinceReview::Rewritten
                };
                assert_eq!(
                    since_review(&repo, head, reviewed, WALK_LIMIT),
                    Some(expected),
                    "head {head} reviewed {reviewed}"
                );
            }
        }
    }

    // Overcounting histories from the #453 round-3 probe (seed 0x453547e146c).
    #[test]
    fn skewed_dags_count_like_graph_ahead_behind() {
        assert_dag_matches_libgit2(
            [99, 162, 80, 87, 195, 2, 158, 121, 109, 139, 178, 83],
            &[
                &[],
                &[0],
                &[0],
                &[2],
                &[0],
                &[1, 3],
                &[4],
                &[2],
                &[1, 2, 3],
                &[0, 1, 6],
                &[0, 2, 8],
                &[1, 5, 9],
            ],
        );
        assert_dag_matches_libgit2(
            [56, 21, 159, 131, 39, 89, 182, 157, 96, 107, 154, 30],
            &[
                &[],
                &[0],
                &[1],
                &[0, 1],
                &[],
                &[0, 3],
                &[0, 2, 5],
                &[4, 5, 6],
                &[7],
                &[2, 3],
                &[2, 5, 6, 8],
                &[1, 8],
            ],
        );
    }

    #[test]
    fn same_second_dags_count_like_graph_ahead_behind() {
        assert_dag_matches_libgit2(
            [100; 12],
            &[
                &[],
                &[],
                &[],
                &[0],
                &[2, 3],
                &[3, 4],
                &[5],
                &[],
                &[0, 2, 4, 6],
                &[8],
                &[7],
                &[1, 6],
            ],
        );
        assert_dag_matches_libgit2(
            [100; 12],
            &[
                &[],
                &[0],
                &[1],
                &[2],
                &[0],
                &[0],
                &[0],
                &[1, 3],
                &[4, 7],
                &[1, 3, 5, 8],
                &[1, 2, 5, 7],
                &[2],
            ],
        );
    }

    /// The #453 round-3 nested shape, every commit at the same second: whether
    /// the walk stops in time depends on how its queue breaks ties.
    #[test]
    fn equal_time_nested_merges_follow_libgit2_queue_order() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init_bare(dir.path()).unwrap();
        let tree = repo.treebuilder(None).unwrap().write().unwrap();
        let tree = repo.find_tree(tree).unwrap();
        let sig =
            git2::Signature::new("Probe", "probe@example.test", &git2::Time::new(100, 0)).unwrap();
        let commit = |message: String, parents: &[Oid]| {
            let parents: Vec<_> = parents
                .iter()
                .map(|p| repo.find_commit(*p).unwrap())
                .collect();
            let parents: Vec<_> = parents.iter().collect();
            repo.commit(None, &sig, &sig, &message, &tree, &parents)
                .unwrap()
        };
        for (salt, head_oid) in [
            (1, "71642a293a103a43c2a6b4af4c4db71ced087fc6"),
            (6, "430a734b38403791cefa17e8209099723b2fa39f"),
            (13, "9c5ddd1b74c06a083a306cdebdb2f2f098a4ce95"),
            (19, "c240947ae83457631f67f989f640e1bccca0ea37"),
            (21, "e879555c97b11a835eee8e96b0048be7e69f7c62"),
            (37, "5a282c86932cfe1867bb3c0a488e5a232b896101"),
            (40, "91fbc8a1014a96cd80b477615f32f8c5c5fe1482"),
            (46, "f3b9ff1d685988488d0d69f324c1c2b10d7a4602"),
            (52, "a7f45a0dccc166ce77d9c5fdf87420cff220d1b9"),
            (72, "dd7a79892cb34b85b4fd5bb635320d2a65e23e51"),
            (73, "d2defbf9616c7f0e5c36652417bfd6fc611cdc36"),
            (77, "3b3649feb38d169c5905f8260b01baea5487462a"),
            (80, "7f2c0e9c7e51c8c4a46c66920c0264f2abf6c972"),
            (81, "f45fd78cf60319a9d9deba49351364a20511b53f"),
            (86, "5506498390521f86bd2af2573955df095db91273"),
            (97, "627e8e25f81b5242759e0af2ed3bf6aa3687f440"),
            (108, "bf84dfc8695eef4361e1882c43ad85b6d9768aec"),
            (111, "3e66082d3dc61e2dc0eea7b096d21be03c1614fd"),
            (112, "b53c441d1e25ccfede53f1be8ed256a8b429516f"),
            (119, "b03d7649b9f7495b83ec6a83456798d60e9ad4c6"),
            (121, "f0e7c8bb051fc9be935f6a8cb482f09881bbe50c"),
        ] {
            let at =
                |label: &str, parents: &[Oid]| commit(format!("equal-{salt}-{label}"), parents);
            let a = at("A", &[]);
            let b = at("B", &[a]);
            let c = at("C", &[b]);
            let d = at("D", &[a]);
            let reviewed = at("R", &[c, d]);
            let s = at("S", &[b]);
            let head = at("H", &[reviewed, s]);
            assert_eq!(head.to_string(), head_oid, "salt {salt} fixture identity");
            let (ahead, _) = repo.graph_ahead_behind(head, reviewed).unwrap();
            assert_eq!(
                since_review(&repo, head, reviewed, WALK_LIMIT),
                Some(SinceReview::Count(ahead as u64)),
                "salt {salt}"
            );
        }
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

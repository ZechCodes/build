//! A fast-forward whose checkout is slow: a filter (git-lfs's smudge
//! downloading a large file) can outlast any deadline a fetch is given
//! (review #268, scenario 4).

use super::*;

/// Upstream adds `a.bin` behind a smudge filter that sleeps `seconds`
/// first, and `b.txt` beside it.
fn slow_filter_upstream(pair: &Pair, seconds: u32) {
    git_in(
        &pair.base,
        &[
            "config",
            "filter.slow.smudge",
            &format!("sleep {seconds}; cat"),
        ],
    );
    git_in(&pair.base, &["config", "filter.slow.clean", "cat"]);
    std::fs::write(pair.upstream.join(".gitattributes"), "*.bin filter=slow\n").unwrap();
    std::fs::write(pair.upstream.join("a.bin"), "payload\n").unwrap();
    std::fs::write(pair.upstream.join("b.txt"), "b\n").unwrap();
    git_in(&pair.upstream, &["add", "."]);
    git_in(&pair.upstream, &["commit", "-q", "-m", "slow"]);
}

fn porcelain(repo: &Path) -> String {
    let status = git_command(repo, &["status", "--porcelain", "--untracked-files=all"])
        .output()
        .unwrap();
    assert!(status.status.success(), "git status failed");
    String::from_utf8_lossy(&status.stdout).into_owned()
}

/// The reviewer's scenario: a 40 s filter, past the 30 s every other local
/// git of a sync gets. Once started, the checkout is let finish: no lock
/// left, no half-written tree.
#[test]
fn a_checkout_slower_than_the_local_deadline_is_let_finish() {
    let pair = pair();
    slow_filter_upstream(&pair, 40);

    let report = sync_base(&pair.base, "main", NOW);

    assert_eq!(report.outcome, SyncOutcome::FastForwarded { commits: 1 });
    assert!(!pair.base.join(".git/index.lock").exists());
    assert_eq!(porcelain(&pair.base), "");
    assert_eq!(
        std::fs::read_to_string(pair.base.join("a.bin")).unwrap(),
        "payload\n"
    );
}

/// Past even the checkout's own cap, git is killed, and what it leaves is
/// cleaned up as far as it is Build's: the index lock it took is removed, so
/// the user's own git and the next sync still work. HEAD and the index never
/// moved; files the checkout had already written are left, untracked, and
/// the next sync names them rather than overwrite them.
#[test]
fn a_checkout_killed_at_its_cap_leaves_no_lock_behind() {
    let pair = pair();
    slow_filter_upstream(&pair, 30);
    let before = rev(&pair.base, "HEAD");

    let started = Instant::now();
    let report = sync_base_capped(&pair.base, "main", NOW, Duration::from_secs(1));

    assert!(
        started.elapsed() < Duration::from_secs(20),
        "was not killed"
    );
    assert!(
        matches!(&report.outcome, SyncOutcome::Failed(failure) if failure.reason.contains("stopped")),
        "{report:?}"
    );
    assert!(
        !pair.base.join(".git/index.lock").exists(),
        "the lock was left"
    );
    assert_eq!(rev(&pair.base, "HEAD"), before);
    assert!(
        !porcelain(&pair.base)
            .lines()
            .any(|line| !line.starts_with("??")),
        "the index moved: {}",
        porcelain(&pair.base)
    );
    std::fs::write(
        pair.base.join("mine.txt"),
        "the user's own git still works\n",
    )
    .unwrap();
    git_in(&pair.base, &["add", "mine.txt"]);
    git_in(&pair.base, &["reset", "-q", "mine.txt"]);

    let again = sync_base_capped(&pair.base, "main", NOW, Duration::from_secs(1));

    assert!(
        skipped(&again).contains(".gitattributes"),
        "the leftover is named: {again:?}"
    );
}

/// A lock the user's own git holds is theirs: a sync that could not take it
/// leaves it where it is.
#[test]
fn a_lock_someone_else_holds_is_left_alone() {
    let pair = pair();
    commit(&pair.upstream, "theirs.txt", "theirs\n");
    let lock = pair.base.join(".git/index.lock");
    std::fs::write(&lock, "").unwrap();

    let report = sync_base_capped(&pair.base, "main", NOW, Duration::from_secs(1));

    assert!(
        matches!(report.outcome, SyncOutcome::Skipped(_)),
        "{report:?}"
    );
    assert!(lock.exists(), "someone else's lock was removed");
}

/// A cut's sync never starts the checkout: the base checked out here and
/// behind is left, however fast its checkout would have been, and says so.
#[test]
fn a_cut_leaves_the_checkout_and_says_so() {
    let pair = pair();
    slow_filter_upstream(&pair, 40);
    let before = rev(&pair.base, "main");

    let started = Instant::now();
    let report = sync_base_for_a_cut(&pair.base, "main", NOW);

    assert!(started.elapsed() < Duration::from_secs(10));
    assert_eq!(report.outcome, SyncOutcome::CheckoutLeft);
    assert!(report.fetched);
    assert_eq!((report.ahead, report.behind), (0, 1));
    assert_eq!(rev(&pair.base, "main"), before);
    assert_eq!(porcelain(&pair.base), "");
}

/// Moving a ref is instant, so a cut still does it: a base checked out
/// nowhere is fast-forwarded.
#[test]
fn a_cut_still_moves_a_base_checked_out_nowhere() {
    let pair = pair();
    git_in(&pair.base, &["switch", "-q", "--detach"]);
    slow_filter_upstream(&pair, 40);

    let report = sync_base_for_a_cut(&pair.base, "main", NOW);

    assert_eq!(report.outcome, SyncOutcome::FastForwarded { commits: 1 });
    assert_eq!(rev(&pair.base, "main"), rev(&pair.upstream, "main"));
}

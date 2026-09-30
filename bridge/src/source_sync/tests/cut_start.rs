//! Where a cut branches from when its base is checked out in the source's
//! checkout and behind (#271): the commit the fetch brought, when the
//! service's fast-forward of that checkout would go through; the base as it
//! stands, and the reason, when it would not.

use super::*;

fn cut_from(report: &SyncReport) -> &str {
    match &report.outcome {
        SyncOutcome::CheckoutLeft { cut_from } => cut_from,
        other => panic!("expected the checkout left with a commit to cut from, got {other:?}"),
    }
}

#[test]
fn a_cut_branches_from_the_fetched_commit_and_leaves_the_checkout() {
    let pair = pair();
    commit(&pair.upstream, "theirs.txt", "theirs\n");
    let before = rev(&pair.base, "main");

    let report = sync_base_for_a_cut(&pair.base, "main", NOW);

    assert_eq!(cut_from(&report), rev(&pair.upstream, "main"));
    assert_eq!((report.ahead, report.behind), (0, 1));
    assert_eq!(rev(&pair.base, "main"), before);
    assert!(!pair.base.join("theirs.txt").exists());
}

/// An untracked file nothing upstream adds is no reason to hold the cut back,
/// as it is none to hold the checkout back.
#[test]
fn an_untracked_file_out_of_the_way_still_gives_the_fetched_commit() {
    let pair = pair();
    commit(&pair.upstream, "theirs.txt", "theirs\n");
    std::fs::write(pair.base.join("scratch.txt"), "notes\n").unwrap();

    let report = sync_base_for_a_cut(&pair.base, "main", NOW);

    assert_eq!(cut_from(&report), rev(&pair.upstream, "main"));
}

/// A tracked directory upstream replaces with a file is removed by the
/// checkout itself, so it is not in the way.
#[test]
fn a_tracked_directory_upstream_turns_into_a_file_is_not_in_the_way() {
    let pair = pair();
    std::fs::create_dir(pair.upstream.join("notes")).unwrap();
    commit(&pair.upstream, "notes/a.md", "a\n");
    git_in(&pair.base, &["pull", "-q", "--ff-only"]);
    git_in(&pair.upstream, &["rm", "-q", "-r", "notes"]);
    commit(&pair.upstream, "notes", "now a file\n");

    let report = sync_base_for_a_cut(&pair.base, "main", NOW);

    assert_eq!(cut_from(&report), rev(&pair.upstream, "main"));
    let service = sync_base(&pair.base, "main", Fetch::Skip);
    assert_eq!(service.outcome, SyncOutcome::FastForwarded { commits: 1 });
}

#[test]
fn uncommitted_changes_keep_the_cut_on_the_base_and_say_why() {
    let pair = pair();
    commit(&pair.upstream, "README.md", "upstream edit\n");
    std::fs::write(pair.base.join("README.md"), "my unsaved edit\n").unwrap();

    let report = sync_base_for_a_cut(&pair.base, "main", NOW);

    assert!(skipped(&report).contains("uncommitted"), "{report:?}");
    assert_eq!(report.behind, 1);
}

#[test]
fn an_untracked_file_upstream_adds_keeps_the_cut_on_the_base_and_is_named() {
    let pair = pair();
    commit(&pair.upstream, "clash.txt", "theirs\n");
    std::fs::write(pair.base.join("clash.txt"), "mine, untracked\n").unwrap();

    let report = sync_base_for_a_cut(&pair.base, "main", NOW);

    assert!(skipped(&report).contains("clash.txt"), "{report:?}");
    assert_eq!(report.behind, 1);
    assert_eq!(
        std::fs::read_to_string(pair.base.join("clash.txt")).unwrap(),
        "mine, untracked\n"
    );
}

/// The reviewer's scenario 1, at a cut: git keeps an ignored file upstream
/// starts tracking, so the checkout will not move and the cut says so.
#[test]
fn an_ignored_file_upstream_starts_tracking_keeps_the_cut_on_the_base() {
    let pair = pair();
    commit(&pair.upstream, ".gitignore", "secret.env\n");
    git_in(&pair.base, &["pull", "-q", "--ff-only"]);
    std::fs::write(pair.base.join("secret.env"), "MY_LOCAL_KEY=precious\n").unwrap();
    std::fs::write(pair.upstream.join("secret.env"), "TRACKED=1\n").unwrap();
    git_in(&pair.upstream, &["add", "-f", "secret.env"]);
    git_in(&pair.upstream, &["commit", "-q", "-m", "track secret.env"]);

    let report = sync_base_for_a_cut(&pair.base, "main", NOW);

    assert!(skipped(&report).contains("secret.env"), "{report:?}");
    assert_eq!(
        std::fs::read(pair.base.join("secret.env")).unwrap(),
        b"MY_LOCAL_KEY=precious\n"
    );
}

/// A file of the user's where upstream adds a directory stops the checkout
/// just as one at the same path does.
#[test]
fn an_untracked_file_where_upstream_adds_a_directory_keeps_the_cut_on_the_base() {
    let pair = pair();
    std::fs::create_dir(pair.upstream.join("docs")).unwrap();
    commit(&pair.upstream, "docs/guide.md", "guide\n");
    std::fs::write(pair.base.join("docs"), "mine, untracked\n").unwrap();

    let report = sync_base_for_a_cut(&pair.base, "main", NOW);

    assert!(skipped(&report).contains("docs"), "{report:?}");
    let service = sync_base(&pair.base, "main", Fetch::Skip);
    assert!(
        matches!(service.outcome, SyncOutcome::Skipped(_)),
        "{service:?}"
    );
}

/// The fetch failing leaves nothing fetched to branch from.
#[test]
fn a_fetch_that_failed_gives_no_commit_to_cut_from() {
    let pair = pair();
    commit(&pair.upstream, "theirs.txt", "theirs\n");
    let gone = pair.upstream.with_file_name("gone.git");
    git_in(
        &pair.base,
        &["remote", "set-url", "origin", gone.to_str().unwrap()],
    );

    let report = sync_base_for_a_cut(&pair.base, "main", NOW);

    failed(&report);
}

/// Review #277, finding 1: a tracked file upstream turns into a symlink is
/// the checkout's own to replace, not a file of the user's.
#[test]
fn a_tracked_file_upstream_turns_into_a_symlink_is_not_in_the_way() {
    let pair = pair();
    git_in(&pair.upstream, &["rm", "-q", "README.md"]);
    std::os::unix::fs::symlink("elsewhere.md", pair.upstream.join("README.md")).unwrap();
    git_in(&pair.upstream, &["add", "README.md"]);
    git_in(
        &pair.upstream,
        &["commit", "-q", "-m", "README becomes a link"],
    );

    let report = sync_base_for_a_cut(&pair.base, "main", NOW);

    assert_eq!(cut_from(&report), rev(&pair.upstream, "main"));
    let service = sync_base(&pair.base, "main", Fetch::Skip);
    assert_eq!(service.outcome, SyncOutcome::FastForwarded { commits: 1 });
}

/// Review #277, finding 2: git will not turn a tracked directory into a
/// file while it holds a file of the user's, so neither does the cut's
/// reading, and it names the directory.
#[test]
fn an_untracked_file_in_a_directory_upstream_turns_into_a_file_keeps_the_cut_on_the_base() {
    let pair = pair();
    std::fs::create_dir(pair.upstream.join("notes")).unwrap();
    commit(&pair.upstream, "notes/a.md", "a\n");
    git_in(&pair.base, &["pull", "-q", "--ff-only"]);
    git_in(&pair.upstream, &["rm", "-q", "-r", "notes"]);
    commit(&pair.upstream, "notes", "now a file\n");
    std::fs::create_dir(pair.base.join("notes/deeper")).unwrap();
    std::fs::write(pair.base.join("notes/deeper/mine.txt"), "mine\n").unwrap();

    let report = sync_base_for_a_cut(&pair.base, "main", NOW);

    assert!(skipped(&report).contains("notes"), "{report:?}");
    let service = sync_base(&pair.base, "main", Fetch::Skip);
    assert!(
        matches!(service.outcome, SyncOutcome::Skipped(_)),
        "{service:?}"
    );
    assert_eq!(
        std::fs::read_to_string(pair.base.join("notes/deeper/mine.txt")).unwrap(),
        "mine\n"
    );
}

use super::*;
use crate::git_fixture::{git_command, git_in, init_repo_named};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const NOW: Fetch = Fetch::Within(Duration::from_secs(20));

/// An upstream repository and a clone of it standing on `main`: the source's
/// base checkout, and the remote it follows.
struct Pair {
    _dir: tempfile::TempDir,
    upstream: PathBuf,
    base: PathBuf,
}

fn pair() -> Pair {
    let dir = tempfile::tempdir().unwrap();
    let upstream = init_repo_named(dir.path(), "upstream");
    let base = dir.path().join("base");
    git_in(
        dir.path(),
        &["clone", "-q", upstream.to_str().unwrap(), "base"],
    );
    git_in(&base, &["config", "user.email", "test@build.ing"]);
    git_in(&base, &["config", "user.name", "Test"]);
    Pair {
        _dir: dir,
        upstream,
        base,
    }
}

fn commit(repo: &Path, file: &str, text: &str) {
    std::fs::write(repo.join(file), text).unwrap();
    git_in(repo, &["add", "."]);
    git_in(repo, &["commit", "-q", "-m", file]);
}

fn rev(repo: &Path, name: &str) -> String {
    let output = git_command(repo, &["rev-parse", name]).output().unwrap();
    assert!(output.status.success(), "rev-parse {name}");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn skipped(report: &SyncReport) -> &str {
    match &report.outcome {
        SyncOutcome::Skipped(reason) => reason,
        other => panic!("expected a skip, got {other:?}"),
    }
}

fn failed(report: &SyncReport) -> &Failure {
    match &report.outcome {
        SyncOutcome::Failed(failure) => failure,
        other => panic!("expected a failure, got {other:?}"),
    }
}

#[test]
fn a_clean_checkout_on_the_base_is_fast_forwarded_with_its_files() {
    let pair = pair();
    commit(&pair.upstream, "new.txt", "from upstream\n");
    commit(&pair.upstream, "more.txt", "and more\n");

    let report = sync_base(&pair.base, "main", NOW);

    assert_eq!(report.outcome, SyncOutcome::FastForwarded { commits: 2 });
    assert!(report.fetched);
    assert_eq!((report.ahead, report.behind), (0, 0));
    assert_eq!(rev(&pair.base, "main"), rev(&pair.upstream, "main"));
    assert_eq!(
        std::fs::read_to_string(pair.base.join("new.txt")).unwrap(),
        "from upstream\n"
    );
}

#[test]
fn a_base_level_with_its_remote_is_up_to_date() {
    let pair = pair();

    let report = sync_base(&pair.base, "main", NOW);

    assert_eq!(report.outcome, SyncOutcome::UpToDate);
    assert_eq!((report.ahead, report.behind), (0, 0));
}

#[test]
fn a_base_with_commits_the_remote_does_not_have_is_left_alone() {
    let pair = pair();
    commit(&pair.upstream, "theirs.txt", "theirs\n");
    commit(&pair.base, "mine.txt", "mine\n");
    let before = rev(&pair.base, "main");

    let report = sync_base(&pair.base, "main", NOW);

    assert!(skipped(&report).contains("1 commit"), "{report:?}");
    assert_eq!((report.ahead, report.behind), (1, 1));
    assert_eq!(rev(&pair.base, "main"), before);
}

#[test]
fn a_base_ahead_of_its_remote_and_not_behind_is_up_to_date() {
    let pair = pair();
    commit(&pair.base, "mine.txt", "mine\n");

    let report = sync_base(&pair.base, "main", NOW);

    assert_eq!(report.outcome, SyncOutcome::UpToDate);
    assert_eq!((report.ahead, report.behind), (1, 0));
}

#[test]
fn uncommitted_changes_in_the_base_checkout_are_never_touched() {
    let pair = pair();
    commit(&pair.upstream, "README.md", "upstream edit\n");
    std::fs::write(pair.base.join("README.md"), "my unsaved edit\n").unwrap();
    let before = rev(&pair.base, "main");

    let report = sync_base(&pair.base, "main", NOW);

    assert!(skipped(&report).contains("uncommitted"), "{report:?}");
    assert_eq!(report.behind, 1);
    assert_eq!(rev(&pair.base, "main"), before);
    assert_eq!(
        std::fs::read_to_string(pair.base.join("README.md")).unwrap(),
        "my unsaved edit\n"
    );
}

#[test]
fn staged_changes_count_as_uncommitted() {
    let pair = pair();
    commit(&pair.upstream, "theirs.txt", "theirs\n");
    std::fs::write(pair.base.join("staged.txt"), "staged\n").unwrap();
    git_in(&pair.base, &["add", "staged.txt"]);

    let report = sync_base(&pair.base, "main", NOW);

    assert!(skipped(&report).contains("uncommitted"), "{report:?}");
}

#[test]
fn an_untracked_file_does_not_hold_the_base_back() {
    let pair = pair();
    commit(&pair.upstream, "theirs.txt", "theirs\n");
    std::fs::write(pair.base.join("scratch.txt"), "notes\n").unwrap();

    let report = sync_base(&pair.base, "main", NOW);

    assert_eq!(report.outcome, SyncOutcome::FastForwarded { commits: 1 });
    assert_eq!(
        std::fs::read_to_string(pair.base.join("scratch.txt")).unwrap(),
        "notes\n"
    );
}

#[test]
fn an_untracked_file_the_fast_forward_would_overwrite_is_kept_and_named() {
    let pair = pair();
    commit(&pair.upstream, "clash.txt", "theirs\n");
    std::fs::write(pair.base.join("clash.txt"), "mine, untracked\n").unwrap();
    let before = rev(&pair.base, "main");

    let report = sync_base(&pair.base, "main", NOW);

    assert!(skipped(&report).contains("clash.txt"), "{report:?}");
    assert_eq!(rev(&pair.base, "main"), before);
    assert_eq!(
        std::fs::read_to_string(pair.base.join("clash.txt")).unwrap(),
        "mine, untracked\n"
    );
}

#[test]
fn a_merge_in_progress_is_left_alone() {
    let pair = pair();
    commit(&pair.upstream, "theirs.txt", "theirs\n");
    let head = rev(&pair.base, "HEAD");
    std::fs::write(pair.base.join(".git/MERGE_HEAD"), format!("{head}\n")).unwrap();

    let report = sync_base(&pair.base, "main", NOW);

    assert!(skipped(&report).contains("in progress"), "{report:?}");
    assert_eq!(rev(&pair.base, "main"), head);
}

#[test]
fn a_checkout_on_another_branch_has_its_base_ref_fast_forwarded_and_its_tree_untouched() {
    let pair = pair();
    commit(&pair.upstream, "theirs.txt", "theirs\n");
    git_in(&pair.base, &["switch", "-q", "-c", "feature"]);
    std::fs::write(pair.base.join("README.md"), "work in progress\n").unwrap();

    let report = sync_base(&pair.base, "main", NOW);

    assert_eq!(report.outcome, SyncOutcome::FastForwarded { commits: 1 });
    assert_eq!(rev(&pair.base, "main"), rev(&pair.upstream, "main"));
    let head = git_command(&pair.base, &["symbolic-ref", "HEAD"])
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&head.stdout).trim(),
        "refs/heads/feature"
    );
    assert!(!pair.base.join("theirs.txt").exists());
    assert_eq!(
        std::fs::read_to_string(pair.base.join("README.md")).unwrap(),
        "work in progress\n"
    );
}

#[test]
fn a_detached_checkout_has_its_base_ref_fast_forwarded() {
    let pair = pair();
    commit(&pair.upstream, "theirs.txt", "theirs\n");
    git_in(&pair.base, &["switch", "-q", "--detach"]);
    let detached_at = rev(&pair.base, "HEAD");

    let report = sync_base(&pair.base, "main", NOW);

    assert_eq!(report.outcome, SyncOutcome::FastForwarded { commits: 1 });
    assert_eq!(rev(&pair.base, "main"), rev(&pair.upstream, "main"));
    assert_eq!(rev(&pair.base, "HEAD"), detached_at);
}

#[test]
fn a_base_checked_out_in_another_worktree_is_left_alone() {
    let pair = pair();
    commit(&pair.upstream, "theirs.txt", "theirs\n");
    git_in(&pair.base, &["switch", "-q", "-c", "feature"]);
    let elsewhere = pair.base.parent().unwrap().join("elsewhere");
    git_in(
        &pair.base,
        &["worktree", "add", "-q", elsewhere.to_str().unwrap(), "main"],
    );
    let before = rev(&pair.base, "main");

    let report = sync_base(&pair.base, "main", NOW);

    assert!(skipped(&report).contains("elsewhere"), "{report:?}");
    assert_eq!(rev(&pair.base, "main"), before);
}

#[test]
fn a_remote_without_the_base_branch_is_reported() {
    let pair = pair();
    git_in(&pair.upstream, &["branch", "-q", "-m", "main", "trunk"]);
    let before = rev(&pair.base, "main");

    let report = sync_base(&pair.base, "main", NOW);

    assert_eq!(skipped(&report), "origin has no branch main.");
    assert_eq!(rev(&pair.base, "main"), before);
}

#[test]
fn a_checkout_with_no_remote_has_nothing_to_sync_with() {
    let dir = tempfile::tempdir().unwrap();
    let repo = init_repo_named(dir.path(), "alone");

    let report = sync_base(&repo, "main", NOW);

    assert_eq!(report.outcome, SyncOutcome::NoRemote);
}

#[test]
fn a_base_branch_git_would_read_as_an_option_is_refused() {
    let pair = pair();

    let report = sync_base(&pair.base, "--upload-pack=touch", NOW);

    assert!(!failed(&report).needs_you);
}

/// The url a remote is checked by is the one git will use, after
/// `url.<base>.insteadOf` rewrites it: a harmless-looking `origin` that git
/// would turn into a transport helper never reaches a fetch.
#[test]
fn a_remote_git_would_rewrite_into_a_helper_is_refused_before_git_fetches() {
    let pair = pair();
    let marker = pair.base.parent().unwrap().join("ran");
    let helper = format!("ext::sh -c touch% {}", marker.display());
    git_in(
        &pair.base,
        &[
            "remote",
            "set-url",
            "origin",
            "https://example.invalid/repo.git",
        ],
    );
    git_in(
        &pair.base,
        &[
            "config",
            &format!("url.{helper}.insteadOf"),
            "https://example.invalid/",
        ],
    );

    let report = sync_base(&pair.base, "main", NOW);

    assert!(
        failed(&report)
            .reason
            .contains("not a location Build will fetch from"),
        "{report:?}"
    );
    assert!(!report.fetched);
    assert!(!marker.exists(), "git ran the helper");
}

#[test]
fn a_remote_that_reads_as_an_option_is_refused_before_git_fetches() {
    let pair = pair();
    let marker = pair.base.parent().unwrap().join("ran");
    let upload_pack = pair.base.parent().unwrap().join("upload-pack.sh");
    std::fs::write(
        &upload_pack,
        format!("#!/bin/sh\ntouch {}\n", marker.display()),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&upload_pack, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    git_in(
        &pair.base,
        &[
            "config",
            "remote.origin.url",
            &format!("--upload-pack={}", upload_pack.display()),
        ],
    );

    let report = sync_base(&pair.base, "main", NOW);

    assert!(
        failed(&report)
            .reason
            .contains("not a location Build will fetch from"),
        "{report:?}"
    );
    assert!(!marker.exists(), "git ran the option");
}

/// A `git://` endpoint that takes the connection and never answers, the way
/// a remote waiting on a key touch or a dead network does.
fn stalling_remote() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(held) => std::mem::forget(held),
                Err(_) => break,
            }
        }
    });
    format!("git://127.0.0.1:{port}/never.git")
}

#[test]
fn a_fetch_that_never_answers_is_killed_at_its_deadline_and_needs_you() {
    let pair = pair();
    git_in(
        &pair.base,
        &["remote", "set-url", "origin", &stalling_remote()],
    );
    let before = rev(&pair.base, "main");

    let started = Instant::now();
    let report = sync_base(
        &pair.base,
        "main",
        Fetch::Within(Duration::from_millis(500)),
    );

    assert!(started.elapsed() < Duration::from_secs(5), "was not killed");
    let failure = failed(&report);
    assert!(failure.needs_you, "{failure:?}");
    assert!(failure.reason.contains("did not answer"), "{failure:?}");
    assert_eq!(rev(&pair.base, "main"), before);
}

/// An http remote that asks for a password on every request.
fn remote_wanting_a_password() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let mut request = [0u8; 4096];
            let _ = stream.read(&mut request);
            let _ = stream.write_all(
                b"HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Basic realm=\"x\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
        }
    });
    format!("http://127.0.0.1:{port}/private.git")
}

/// However the machine is set up to ask for a secret, a background fetch
/// asks nobody: an askpass program in the repository's config is never run,
/// and the fetch fails, saying it needs you.
#[test]
fn a_remote_that_wants_a_password_fails_without_asking_for_one() {
    let pair = pair();
    let marker = pair.base.parent().unwrap().join("asked");
    let askpass = pair.base.parent().unwrap().join("askpass.sh");
    std::fs::write(
        &askpass,
        format!("#!/bin/sh\ntouch {}\necho secret\n", marker.display()),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&askpass, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    git_in(
        &pair.base,
        &["remote", "set-url", "origin", &remote_wanting_a_password()],
    );
    git_in(
        &pair.base,
        &["config", "core.askPass", askpass.to_str().unwrap()],
    );
    git_in(&pair.base, &["config", "credential.helper", ""]);

    let report = sync_base(&pair.base, "main", NOW);

    assert!(failed(&report).needs_you, "{report:?}");
    assert!(!marker.exists(), "an askpass program was run");
}

#[test]
fn without_a_fetch_the_base_catches_up_to_what_was_last_fetched() {
    let pair = pair();
    commit(&pair.upstream, "theirs.txt", "theirs\n");

    let unfetched = sync_base(&pair.base, "main", Fetch::Skip);
    assert_eq!(unfetched.outcome, SyncOutcome::UpToDate);
    assert!(!unfetched.fetched);

    git_in(&pair.base, &["fetch", "-q", "origin"]);
    let caught_up = sync_base(&pair.base, "main", Fetch::Skip);
    assert_eq!(caught_up.outcome, SyncOutcome::FastForwarded { commits: 1 });
}

/// The compare-and-swap: a base that moves between the read and the write is
/// not overwritten, whatever it moved to.
#[test]
fn a_ref_that_moves_between_the_read_and_the_swap_is_not_overwritten() {
    let pair = pair();
    commit(&pair.upstream, "theirs.txt", "theirs\n");
    git_in(&pair.base, &["fetch", "-q", "origin"]);
    git_in(&pair.base, &["switch", "-q", "-c", "feature"]);
    let read = rev(&pair.base, "main");
    let target = rev(&pair.base, "origin/main");
    // Someone commits onto main in the gap.
    git_in(&pair.base, &["switch", "-q", "main"]);
    commit(&pair.base, "racer.txt", "racer\n");
    git_in(&pair.base, &["switch", "-q", "feature"]);
    let raced = rev(&pair.base, "main");

    let swapped = fast_forward_ref(&pair.base, "main", &read, &target);

    assert!(swapped.is_err());
    assert_eq!(rev(&pair.base, "main"), raced);
}

#[test]
fn credentials_in_a_url_are_taken_out_of_what_git_said() {
    assert_eq!(
        without_credentials(
            "fatal: unable to access 'https://zech:ghp_secret@github.com/x/y.git/': 403"
        ),
        "fatal: unable to access 'https://github.com/x/y.git/': 403"
    );
    assert_eq!(
        without_credentials("ssh://git@host/r and http://u:p@h:8080/"),
        "ssh://host/r and http://h:8080/"
    );
    assert_eq!(without_credentials("no url here"), "no url here");
}

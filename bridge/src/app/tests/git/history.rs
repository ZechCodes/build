use super::status::git_gui_state;
use super::*;

/// A repository whose history is `commits` deep, the initial commit included.
fn repo_with_history(commits: usize) -> (tempfile::TempDir, PathBuf) {
    let (dir, repo) = init_repo();
    for n in 2..=commits {
        let message = format!("commit {n}");
        git_in(&repo, &["commit", "-q", "--allow-empty", "-m", &message]);
    }
    (dir, repo)
}

/// What `git rev-parse` makes of a name — the test's own way to a hash, so a
/// fixture never asks the verb under test where a commit is.
fn rev_parse(repo: &Path, name: &str) -> String {
    let read = Command::new("git")
        .args(["rev-parse", name])
        .current_dir(repo)
        .output()
        .expect("git rev-parse runs");
    assert!(read.status.success(), "git rev-parse {name} failed");
    String::from_utf8(read.stdout).unwrap().trim().to_string()
}

/// Every commit the scope's history holds, oldest first.
fn history(state: &mut AppState, project_id: &str) -> Vec<String> {
    let log = state.handle(req(
        "git.log",
        json!({ "project_id": project_id, "limit": 200 }),
    ));
    assert_eq!(log["ok"], true, "{log:?}");
    let mut hashes: Vec<String> = log["result"]["commits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|commit| commit["hash"].as_str().unwrap().to_string())
        .collect();
    hashes.reverse();
    hashes
}

/// The subjects a log answered with, in the order it answered them.
fn subjects(log: &Value) -> Vec<String> {
    log["result"]["commits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|commit| commit["subject"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn git_log_since_a_cached_hash_answers_only_what_landed_after_it() {
    let (dir, repo) = repo_with_history(5);
    let mut state = git_gui_state(&dir, &repo);
    let project_id = state.project_at(0).id.clone();
    let hashes = history(&mut state, &project_id);

    let log = state.handle(req(
        "git.log",
        json!({ "project_id": project_id, "since": hashes[1] }),
    ));
    assert_eq!(log["ok"], true, "{log:?}");
    assert_eq!(
        subjects(&log),
        vec!["commit 5", "commit 4", "commit 3"],
        "{log:?}"
    );
    assert_eq!(log["result"]["reset"], false, "{log:?}");
    assert_eq!(log["result"]["newest"], hashes[4].as_str(), "{log:?}");
    assert_eq!(log["result"]["more"], false, "{log:?}");
}

#[test]
fn git_log_since_the_head_it_already_holds_answers_nothing_new() {
    let (dir, repo) = repo_with_history(5);
    let mut state = git_gui_state(&dir, &repo);
    let project_id = state.project_at(0).id.clone();
    let hashes = history(&mut state, &project_id);

    let log = state.handle(req(
        "git.log",
        json!({ "project_id": project_id, "since": hashes[4] }),
    ));
    assert_eq!(log["ok"], true, "{log:?}");
    assert!(
        log["result"]["commits"].as_array().unwrap().is_empty(),
        "{log:?}"
    );
    assert_eq!(log["result"]["reset"], false, "{log:?}");
    // The cursor still comes back, so a caught-up client keeps its place.
    assert_eq!(log["result"]["newest"], hashes[4].as_str(), "{log:?}");
    assert_eq!(log["result"]["more"], false, "{log:?}");
}

/// A rebase, a reset, or a hash from another checkout entirely: the cursor
/// names no ancestor of HEAD, so the answer is a fresh log and the flag that
/// tells the client to replace its own rather than prepend to it.
#[test]
fn git_log_since_a_hash_off_the_history_answers_the_latest_commits_and_says_reset() {
    let (dir, repo) = repo_with_history(25);
    let mut state = git_gui_state(&dir, &repo);
    let project_id = state.project_at(0).id.clone();
    let head = rev_parse(&repo, "HEAD");
    // A commit that exists but sits off the checked-out history.
    git_in(&repo, &["checkout", "-q", "-b", "aside"]);
    git_in(&repo, &["commit", "-q", "--allow-empty", "-m", "aside"]);
    let aside = rev_parse(&repo, "HEAD");
    git_in(&repo, &["checkout", "-q", "main"]);

    for orphan in ["deadbeefdeadbeefdeadbeefdeadbeefdeadbeef", &aside] {
        let log = state.handle(req(
            "git.log",
            json!({ "project_id": project_id, "since": orphan }),
        ));
        assert_eq!(log["ok"], true, "{log:?}");
        assert_eq!(log["result"]["reset"], true, "{orphan}: {log:?}");
        // The latest page, cut at the cursored default rather than the
        // browsing one.
        assert_eq!(
            log["result"]["commits"].as_array().unwrap().len(),
            crate::api::v1::git::LATEST_COMMITS as usize,
            "{orphan}: {log:?}"
        );
        assert_eq!(log["result"]["more"], true, "{orphan}: {log:?}");
        assert_eq!(log["result"]["newest"], head.as_str(), "{orphan}: {log:?}");
    }
}

/// `since` is an object-id prefix like `git.show`'s hash, never a revspec: a
/// branch named like hex must not be able to shadow one.
#[test]
fn git_log_refuses_a_since_that_is_not_an_object_id_prefix() {
    let (dir, repo) = repo_with_history(2);
    let mut state = git_gui_state(&dir, &repo);
    let project_id = state.project_at(0).id.clone();

    for bad in ["HEAD", "abc", "ABCDEF12", "main", "deadbeef^", ""] {
        let refused = state.handle(req(
            "git.log",
            json!({ "project_id": project_id, "since": bad }),
        ));
        assert_eq!(refused["ok"], false, "since {bad:?}: {refused:?}");
        assert_eq!(refused["error_code"], "invalid_params", "{refused:?}");
    }
}

/// A log nobody handed a cursor to is the page it always was, with the
/// cursor fields beside it so a client can start holding one.
#[test]
fn git_log_without_a_cursor_still_carries_the_head_it_read() {
    let (dir, repo) = repo_with_history(3);
    let mut state = git_gui_state(&dir, &repo);
    let project_id = state.project_at(0).id.clone();
    let hashes = history(&mut state, &project_id);

    let log = state.handle(req("git.log", json!({ "project_id": project_id })));
    assert_eq!(log["ok"], true, "{log:?}");
    assert_eq!(log["result"]["commits"].as_array().unwrap().len(), 3);
    assert_eq!(log["result"]["reset"], false, "{log:?}");
    assert_eq!(log["result"]["newest"], hashes[2].as_str(), "{log:?}");
}

/// An unborn HEAD has no cursor to hand back, and a `since` against one is
/// not a reset — there is nothing to replace.
#[test]
fn git_log_on_an_unborn_head_names_no_cursor() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    git_in(&repo, &["init", "-b", "main"]);
    let mut state = git_gui_state(&dir, &repo);
    let project_id = state.project_at(0).id.clone();

    let log = state.handle(req(
        "git.log",
        json!({ "project_id": project_id, "since": "deadbeef" }),
    ));
    assert_eq!(log["ok"], true, "{log:?}");
    assert!(
        log["result"]["commits"].as_array().unwrap().is_empty(),
        "{log:?}"
    );
    assert_eq!(log["result"]["reset"], false, "{log:?}");
    assert!(log["result"]["newest"].is_null(), "{log:?}");
}

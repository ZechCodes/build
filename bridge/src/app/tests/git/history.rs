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

/// A cursored page that cannot carry the whole gap is a reset too. The page
/// is the commits nearest HEAD, so prepending it to a log whose newest is
/// the cursor would leave a hole between them — one the client cannot see,
/// because the cursor it stores next is HEAD.
#[test]
fn git_log_since_a_hash_too_far_back_to_page_says_reset() {
    let (dir, repo) = repo_with_history(30);
    let mut state = git_gui_state(&dir, &repo);
    let project_id = state.project_at(0).id.clone();
    let hashes = history(&mut state, &project_id);

    // 29 commits landed since the cursor; the cursored default carries 20.
    let log = state.handle(req(
        "git.log",
        json!({ "project_id": project_id, "since": hashes[0] }),
    ));
    assert_eq!(log["ok"], true, "{log:?}");
    assert_eq!(
        log["result"]["commits"].as_array().unwrap().len(),
        crate::api::v1::git::LATEST_COMMITS as usize,
        "{log:?}"
    );
    assert_eq!(subjects(&log)[0], "commit 30", "{log:?}");
    assert_eq!(log["result"]["more"], true, "{log:?}");
    assert_eq!(log["result"]["reset"], true, "{log:?}");
    assert_eq!(log["result"]["newest"], hashes[29].as_str(), "{log:?}");

    // A limit wide enough for the gap reaches the cursor, so the answer is
    // one the client may prepend.
    let whole = state.handle(req(
        "git.log",
        json!({ "project_id": project_id, "since": hashes[0], "limit": 29 }),
    ));
    assert_eq!(whole["result"]["more"], false, "{whole:?}");
    assert_eq!(whole["result"]["reset"], false, "{whole:?}");
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

/// A cursor of the wrong type is refused, never ignored — the typed params
/// are read before the handler sees them, and this pins that they are.
///
/// What it costs if they ever stop being: a number dropped on the floor
/// hands the caller the browsing page with `reset: false` on it, an
/// uncursored read wearing a cursored read's answer, which is exactly what
/// a cache would apply as if it had asked for it.
#[test]
fn git_log_refuses_a_since_that_is_not_a_string() {
    let (dir, repo) = repo_with_history(3);
    let mut state = git_gui_state(&dir, &repo);
    let project_id = state.project_at(0).id.clone();

    for mistyped in [json!(123), json!(true), json!(["deadbeef"])] {
        let refused = state.handle(req(
            "git.log",
            json!({ "project_id": project_id, "since": mistyped }),
        ));
        assert_eq!(refused["ok"], false, "since {mistyped}: {refused:?}");
        assert_eq!(refused["error_code"], "invalid_params", "{refused:?}");
    }

    // Null is how a client says it holds no cursor at all, so it reads as
    // the uncursored page rather than as a refusal.
    let unnamed = state.handle(req(
        "git.log",
        json!({ "project_id": project_id, "since": Value::Null }),
    ));
    assert_eq!(unnamed["ok"], true, "{unnamed:?}");
    assert_eq!(unnamed["result"]["commits"].as_array().unwrap().len(), 3);
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

// ---- git.show, capped ----------------------------------------------------

/// A repository whose HEAD commit adds `lines` identical lines — a patch as
/// large as the test needs it, with an exact stat to check it against.
fn repo_with_a_large_commit(lines: usize) -> (tempfile::TempDir, PathBuf) {
    let (dir, repo) = init_repo();
    std::fs::write(repo.join("big.txt"), "0123456789abcdef\n".repeat(lines)).unwrap();
    git_in(&repo, &["add", "big.txt"]);
    git_in(&repo, &["commit", "-q", "-m", "a large commit"]);
    (dir, repo)
}

/// What `git.show` makes of the scope's HEAD commit.
fn show_head(state: &mut AppState, project_id: &str, max_bytes: Option<u64>) -> Value {
    let log = state.handle(req(
        "git.log",
        json!({ "project_id": project_id, "limit": 1 }),
    ));
    let hash = log["result"]["commits"][0]["hash"]
        .as_str()
        .unwrap()
        .to_string();
    let mut params = json!({ "project_id": project_id, "hash": hash });
    if let Some(max_bytes) = max_bytes {
        params["max_bytes"] = json!(max_bytes);
    }
    state.handle(req("git.show", params))
}

/// A client caching commits asks for a patch it can afford. Past the cap it
/// is told which files moved and how large the real patch is, and fetches
/// the patch itself only when a reviewer opens the commit.
#[test]
fn git_show_capped_at_max_bytes_answers_the_file_list_and_the_patchs_true_size() {
    let (dir, repo) = repo_with_a_large_commit(2_000);
    let mut state = git_gui_state(&dir, &repo);
    let project_id = state.project_at(0).id.clone();

    let shown = show_head(&mut state, &project_id, Some(1024));
    assert_eq!(shown["ok"], true, "{shown:?}");
    let result = &shown["result"];
    assert_eq!(result["truncated"], true, "{result:?}");
    let patch = result["patch"].as_str().unwrap();
    assert!(patch.contains("diff --git a/big.txt b/big.txt"), "{patch}");
    assert!(patch.contains("+++ b/big.txt"), "{patch}");
    // The header and the file list, not the top of the diff.
    assert!(!patch.contains("+0123456789abcdef"), "{patch}");
    assert!(patch.len() <= 1024, "{} bytes", patch.len());
    // The counts and the size are the whole patch's, so the client can say
    // what opening it would cost.
    assert_eq!(result["stat"]["insertions"], 2_000, "{result:?}");
    assert!(result["patch_bytes"].as_u64().unwrap() > 1024, "{result:?}");
}

/// The cap is clamped at both ends: a patch that fits under the floor is
/// never cut, and a caller that asks for more than the wire carries gets
/// `COMMIT_PATCH_MAX_BYTES`.
#[test]
fn git_show_clamps_the_cap_it_was_asked_for() {
    let (dir, repo) = repo_with_a_large_commit(30_000);
    let mut state = git_gui_state(&dir, &repo);
    let project_id = state.project_at(0).id.clone();

    // ~510 KiB of patch: under the 1 MiB default, over the clamped ceiling.
    let greedy = show_head(&mut state, &project_id, Some(8 * 1_048_576));
    assert_eq!(greedy["result"]["truncated"], true, "{greedy:?}");
    let uncapped = show_head(&mut state, &project_id, None);
    assert_eq!(uncapped["result"]["truncated"], false, "{uncapped:?}");
    assert_eq!(
        uncapped["result"]["patch_bytes"].as_u64().unwrap(),
        uncapped["result"]["patch"].as_str().unwrap().len() as u64,
        "{uncapped:?}"
    );

    // A floor of 1 KiB, so a cap of nothing still answers a small commit
    // whole rather than as a file list.
    let (small_dir, small_repo) = repo_with_a_large_commit(10);
    let mut small = git_gui_state(&small_dir, &small_repo);
    let small_project = small.project_at(0).id.clone();
    let tiny_cap = show_head(&mut small, &small_project, Some(1));
    assert_eq!(tiny_cap["result"]["truncated"], false, "{tiny_cap:?}");
    assert!(tiny_cap["result"]["patch"]
        .as_str()
        .unwrap()
        .contains("+0123456789abcdef"));
}

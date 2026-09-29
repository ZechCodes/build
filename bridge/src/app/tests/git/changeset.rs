use super::*;

// ==== one file's hunks out of a changeset ==================================
//
// A review surface holds a list of changed paths long before it holds what
// any of them say: a `git` push carries the list with no patch at all, and
// the client's cold pass asks for the same shape. `git.changeset_diff` is how
// the hunks behind ONE of those paths are read when the reader opens it —
// the same changeset the whole-patch verb answers, narrowed to what is on
// screen, so a checkout with a megabyte of diff costs the file being looked
// at and nothing else.

/// The `files` row for `path` in a diff-shaped answer.
fn row<'a>(diff: &'a Value, path: &str) -> &'a Value {
    diff["files"]
        .as_array()
        .unwrap_or_else(|| panic!("a diff answers a file list: {diff:?}"))
        .iter()
        .find(|file| file["path"] == json!(path))
        .unwrap_or_else(|| panic!("no {path} in {diff:?}"))
}

/// A project repository with two uncommitted files, one of them large enough
/// that carrying it for a reader who did not open it is the whole problem.
fn project_with_two_dirty_files(
    repo: &std::path::Path,
    dir: &std::path::Path,
) -> (Arc<Mutex<AppState>>, FrameHandler, String) {
    let (state, handler) = shared_qa_state_and_handler(repo, dir);
    let project_id = state.lock().unwrap().project_at(0).id.clone();
    std::fs::write(repo.join("small.txt"), "one small line\n").unwrap();
    std::fs::write(
        repo.join("large.txt"),
        "a line nobody asked to read\n".repeat(400),
    )
    .unwrap();
    (state, handler, project_id)
}

/// The per-file counts every diff row carries: they are what a stack with no
/// hunks loaded draws its `+`/`−` from, and what the review bar sums.
#[test]
fn a_diff_row_says_what_its_file_weighs() {
    let (dir, repo) = init_repo();
    let (_state, handler, project_id) = project_with_two_dirty_files(&repo, dir.path());

    let diff = call(
        &handler,
        "git.changeset_diff",
        json!({ "project_id": project_id, "paths": ["small.txt", "large.txt"] }),
    );

    assert_eq!(diff["ok"], true, "{diff:?}");
    assert_eq!(row(&diff["result"], "small.txt")["additions"], 1);
    assert_eq!(row(&diff["result"], "small.txt")["deletions"], 0);
    assert_eq!(row(&diff["result"], "large.txt")["additions"], 400);
}

/// The verb's whole point: ask for one path, receive that path's hunks and
/// no others.
#[test]
fn a_changeset_diff_answers_only_the_paths_asked_for() {
    let (dir, repo) = init_repo();
    let (_state, handler, project_id) = project_with_two_dirty_files(&repo, dir.path());

    let opened = call(
        &handler,
        "git.changeset_diff",
        json!({ "project_id": project_id, "paths": ["small.txt"] }),
    );

    assert_eq!(opened["ok"], true, "{opened:?}");
    let result = &opened["result"];
    assert_eq!(
        result["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|file| file["path"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["small.txt"],
        "{result:?}"
    );
    assert!(
        result["patch"].as_str().unwrap().contains("one small line"),
        "{result:?}"
    );
    assert!(
        !result["patch"].as_str().unwrap().contains("large.txt"),
        "the file the reader did not open is not in the answer: {result:?}"
    );
}

/// The key on a narrowed answer names the WHOLE changeset, because that is
/// what says whether the body still stands. A client holding it asks the
/// whole-patch verb and the narrowed one with the same key and is answered
/// consistently by both.
#[test]
fn a_narrowed_answer_is_keyed_by_the_whole_changeset() {
    let (dir, repo) = init_repo();
    let (state, handler, project_id) = project_with_two_dirty_files(&repo, dir.path());
    let checkout = super::filesystem::add_external_worktree(&repo, dir.path(), "loose", "feature");
    let worktree_id = state
        .lock()
        .unwrap()
        .scan_external_worktrees_now(&project_id)
        .expect("the project's checkouts are scanned")
        .first()
        .expect("the checkout was found")
        .id
        .clone();
    std::fs::write(checkout.join("opened.txt"), "what the reader opened\n").unwrap();
    let scope = json!({ "project_id": project_id, "worktree_id": worktree_id });

    let whole = call(&handler, "worktree.diff", scope.clone());
    let key = whole["result"]["diff_key"].as_str().unwrap().to_string();
    let narrowed = call(
        &handler,
        "git.changeset_diff",
        json!({ "project_id": project_id, "worktree_id": worktree_id, "paths": ["opened.txt"] }),
    );

    assert_eq!(narrowed["result"]["diff_key"], json!(key), "{narrowed:?}");
    // And the conditional read short-circuits on it, so a reader whose body
    // is still good pays one small answer rather than a patch.
    let unchanged = call(
        &handler,
        "git.changeset_diff",
        json!({
            "project_id": project_id,
            "worktree_id": worktree_id,
            "paths": ["opened.txt"],
            "if_diff_key": key,
        }),
    );
    assert_eq!(unchanged["result"]["unchanged"], true, "{unchanged:?}");
    assert!(unchanged["result"].get("patch").is_none(), "{unchanged:?}");
}

/// A run's changeset is its own — measured against its baseline, not the
/// working tree alone — and the narrowed read answers out of that same
/// changeset.
#[test]
fn a_run_scope_narrows_the_runs_own_changeset() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_qa_state_and_handler(&repo, dir.path());
    let run_id = {
        let mut state = state.lock().unwrap();
        let (_plan, run_id) = planned_run_in_review(&mut state, "narrow me");
        run_id
    };
    let root = state
        .lock()
        .unwrap()
        .runs
        .get(&run_id)
        .unwrap()
        .worktree
        .path
        .clone();
    std::fs::write(root.join("opened.txt"), "one opened line\n").unwrap();
    std::fs::write(root.join("unopened.txt"), "unread\n".repeat(200)).unwrap();

    let opened = call(
        &handler,
        "git.changeset_diff",
        json!({ "run_id": run_id, "paths": ["opened.txt"] }),
    );

    assert_eq!(opened["ok"], true, "{opened:?}");
    assert_eq!(row(&opened["result"], "opened.txt")["additions"], 1);
    assert!(
        !opened["result"]["patch"]
            .as_str()
            .unwrap()
            .contains("unread"),
        "{opened:?}"
    );
}

/// A path with nothing in this changeset answers an empty list rather than
/// an error: the file list a reader holds can be a push behind the tree.
#[test]
fn a_path_with_nothing_to_say_answers_an_empty_diff() {
    let (dir, repo) = init_repo();
    let (_state, handler, project_id) = project_with_two_dirty_files(&repo, dir.path());

    let nothing = call(
        &handler,
        "git.changeset_diff",
        json!({ "project_id": project_id, "paths": ["never-touched.txt"] }),
    );

    assert_eq!(nothing["ok"], true, "{nothing:?}");
    assert_eq!(nothing["result"]["files"], json!([]), "{nothing:?}");
    assert_eq!(nothing["result"]["patch"], json!(""), "{nothing:?}");
}

/// The caps `git.diff` is already read through, on the verb that answers the
/// same kind of question: no empty ask, and no ask past the batch a client is
/// written to send.
#[test]
fn a_changeset_diff_takes_one_to_fifty_paths() {
    let (dir, repo) = init_repo();
    let (_state, handler, project_id) = project_with_two_dirty_files(&repo, dir.path());

    let none = call(
        &handler,
        "git.changeset_diff",
        json!({ "project_id": project_id, "paths": [] }),
    );
    assert_eq!(none["ok"], false, "{none:?}");

    let too_many: Vec<String> = (0..51).map(|index| format!("file-{index}.txt")).collect();
    let past = call(
        &handler,
        "git.changeset_diff",
        json!({ "project_id": project_id, "paths": too_many }),
    );
    assert_eq!(past["ok"], false, "{past:?}");
    assert!(
        past["error"].as_str().unwrap().contains("50"),
        "the refusal says the cap: {past:?}"
    );
}

/// A path that would read outside the checkout is refused, exactly as
/// `git.diff` refuses it: a diff verb is not a way to read the machine.
#[test]
fn a_path_that_escapes_the_checkout_is_refused() {
    let (dir, repo) = init_repo();
    let (_state, handler, project_id) = project_with_two_dirty_files(&repo, dir.path());

    let escaped = call(
        &handler,
        "git.changeset_diff",
        json!({ "project_id": project_id, "paths": ["../outside.txt"] }),
    );

    assert_eq!(escaped["ok"], false, "{escaped:?}");
}

/// A workspace source's changeset is its UNPUBLISHED work — the delta
/// `git.unpushed` describes — so the narrowed read comes out of that same
/// delta and under that same key. It is the surface a phone lands on, and
/// the aggregate patch behind it was the megabyte the landing spent.
#[test]
fn a_workspace_source_narrows_its_unpublished_changeset() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_qa_state_and_handler(&repo, dir.path());
    let project_id = state.lock().unwrap().project_at(0).id.clone();
    state.lock().unwrap().workspaces.adopt_root(
        &project_id,
        "workspace-a".to_string(),
        "A".to_string(),
        repo.clone(),
        "source-1".to_string(),
        true,
    );
    std::fs::write(repo.join("opened.txt"), "what the reader opened\n").unwrap();
    std::fs::write(repo.join("unopened.txt"), "unread\n".repeat(200)).unwrap();

    let listed = call(
        &handler,
        "git.unpushed",
        json!({ "workspace_id": "workspace-a", "source_id": "source-1", "patch": false }),
    );
    let opened = call(
        &handler,
        "git.changeset_diff",
        json!({ "workspace_id": "workspace-a", "source_id": "source-1", "paths": ["opened.txt"] }),
    );

    assert_eq!(opened["ok"], true, "{opened:?}");
    assert_eq!(
        opened["result"]["diff_key"], listed["result"]["diff_key"],
        "the narrowed read answers under the key the list read gave: {opened:?} against {listed:?}"
    );
    assert!(
        opened["result"]["patch"]
            .as_str()
            .unwrap()
            .contains("what the reader opened"),
        "{opened:?}"
    );
    assert!(
        !opened["result"]["patch"]
            .as_str()
            .unwrap()
            .contains("unread"),
        "{opened:?}"
    );
    // And the per-file key matches the one the list carried for that path, or
    // a body fetched under it would never be recognised as still current.
    assert_eq!(
        row(&opened["result"], "opened.txt")["content_key"],
        row(&listed["result"], "opened.txt")["content_key"],
    );
}

/// Every read that can be asked for the shape without its hunks must survive
/// the facade with the hunks gone.
///
/// `patch: false` shipped without this: the typed result declared `patch` as
/// a required string, so the facade could not serialise the very answer the
/// flag asks for and refused it as `internal`. The client's cold pass asked
/// that way on every workspace, was refused every time, and nobody saw it —
/// a pass swallows a failed read and the surfaces read for themselves. So
/// every one of them is asked here, and the answer has to be an answer.
#[test]
fn every_diff_read_survives_having_its_patch_left_off() {
    let (dir, repo) = init_repo();
    let (state, handler, project_id) = project_with_two_dirty_files(&repo, dir.path());
    let checkout = super::filesystem::add_external_worktree(&repo, dir.path(), "loose", "feature");
    let worktree_id = state
        .lock()
        .unwrap()
        .scan_external_worktrees_now(&project_id)
        .expect("the project's checkouts are scanned")
        .first()
        .expect("the checkout was found")
        .id
        .clone();
    std::fs::write(checkout.join("in-the-checkout.txt"), "a line\n").unwrap();
    let run_id = {
        let mut state = state.lock().unwrap();
        let (_plan, run_id) = planned_run_in_review(&mut state, "shapeless");
        run_id
    };

    for (method, params) in [
        (
            "worktree.diff",
            json!({ "project_id": project_id, "worktree_id": worktree_id, "patch": false }),
        ),
        ("run.diff", json!({ "run_id": run_id, "patch": false })),
        (
            "git.unpushed",
            json!({ "project_id": project_id, "patch": false }),
        ),
    ] {
        let answer = call(&handler, method, params);
        assert_eq!(answer["ok"], true, "{method}: {answer:?}");
        assert!(
            answer["result"].get("patch").is_none(),
            "{method} left the hunks off: {answer:?}"
        );
        assert!(
            answer["result"]["files"].is_array(),
            "{method} still names its files: {answer:?}"
        );
    }
}

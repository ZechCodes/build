use super::*;

// ==== the branch as the wire-level work item ==============================

/// The feed, with every poll cache cleared first. All three are 10s TTL, so
/// a test that changes git state and re-polls would otherwise be answered
/// from the poll before it.
pub(in crate::app::tests) fn work_item_rows(state: &mut AppState) -> Vec<Value> {
    state.board.diff_mut().clear_run_stats();
    let project_ids = state.projects.ids().map(str::to_string).collect::<Vec<_>>();
    for project_id in project_ids {
        state.board.diff_mut().clear_primary_summary(&project_id);
        state.board.diff_mut().clear_external_scan(&project_id);
    }
    state.handle(req("board.list", json!({})))["result"]["items"]
        .as_array()
        .expect("the feed ships work items")
        .clone()
}

pub(in crate::app::tests) fn branch_row(state: &mut AppState, branch: &str) -> Value {
    work_item_rows(state)
        .into_iter()
        .find(|row| row["kind"] == "branch" && row["branch"] == branch)
        .unwrap_or_else(|| panic!("{branch} has a row on the feed"))
}

/// What Done on this row would warn about, in order.
pub(in crate::app::tests) fn warning_codes(row: &Value) -> Vec<String> {
    row["finish"]["warnings"]
        .as_array()
        .unwrap_or_else(|| panic!("every row carries a finish preflight: {row:?}"))
        .iter()
        .map(|warning| {
            warning["code"]
                .as_str()
                .unwrap_or_else(|| panic!("a warning is coded: {warning:?}"))
                .to_string()
        })
        .collect()
}

fn hours_ago(hours: i64) -> String {
    (time::OffsetDateTime::now_utc() - time::Duration::hours(hours))
        .format(&time::format_description::well_known::Rfc3339)
        .expect("UTC formats as RFC 3339")
}

/// A run and the primary checkout are two ways of storing the same kind of
/// thing. The feed shows one row shape for both, keyed by branch, and the
/// primary checkout is the `main` row. A worktree Build never cut or
/// adopted is a THIRD source `work_items` still folds in (`branch.get`
/// deep-links to it), but `board_list`'s feed leaves it out — it is not
/// work the user started in Build.
#[test]
#[allow(clippy::cognitive_complexity)] // ratchet: the_feed_folds_runs_worktrees_and_the_primary_checkout_into_branch_rows is at 25, threshold 15 — bring it under, then remove
fn the_feed_folds_runs_worktrees_and_the_primary_checkout_into_branch_rows() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let run_id = adopted_run(&mut state, &repo, dir.path(), "feature-adopted");
    add_external_worktree(&repo, dir.path(), "stray", "feature-stray");
    let project_id = state.project_at(0).id.clone();

    let board = state.handle(req("board.list", json!({})));
    // The active board carries runs and workspace-backed rows, with no legacy
    // issue collections.
    assert!(board["result"]["runs"].is_array(), "{board:?}");
    assert!(board["result"].get("plans").is_none(), "{board:?}");
    assert!(board["result"].get("issues").is_none(), "{board:?}");
    assert!(
        board["result"]["external_worktrees"].is_array(),
        "{board:?}"
    );

    let adopted = branch_row(&mut state, "feature-adopted");
    assert_eq!(adopted["kind"], "branch", "{adopted:?}");
    assert_eq!(adopted["run_id"], run_id, "{adopted:?}");
    assert_eq!(adopted["project_id"], project_id, "{adopted:?}");
    assert_eq!(adopted["title"], "feature-adopted", "{adopted:?}");
    assert_eq!(adopted["state"], "review", "{adopted:?}");
    assert!(adopted["issue_id"].is_null(), "{adopted:?}");
    assert!(
        adopted["worktree_path"]
            .as_str()
            .unwrap()
            .ends_with("feature-adopted"),
        "{adopted:?}"
    );
    assert_eq!(adopted["agents"].as_array().unwrap().len(), 1);
    assert_eq!(adopted["unread"], false, "{adopted:?}");
    assert_eq!(adopted["unread_count"], 0, "{adopted:?}");
    assert_eq!(adopted["working"], false, "{adopted:?}");
    assert!(adopted["working_time"].is_null(), "{adopted:?}");
    assert_eq!(adopted["muted"], false, "{adopted:?}");
    assert!(adopted["stat"]["insertions"].is_u64(), "{adopted:?}");

    // Discoverable on disk, resolvable by name, but not on the feed:
    // Build never cut or adopted it, so it is not the user's in-flight
    // work.
    assert!(
        state
            .scan_external_worktrees_now(&project_id)
            .unwrap()
            .into_iter()
            .any(|w| w.branch.as_deref() == Some("feature-stray")),
        "still discoverable for adoption"
    );
    let routed = state.handle(req(
        "branch.get",
        json!({ "project_id": project_id, "branch": "feature-stray" }),
    ));
    assert_eq!(
        routed["ok"], true,
        "branch.get still deep-links to it: {routed:?}"
    );
    assert!(
        work_item_rows(&mut state)
            .iter()
            .all(|row| row["branch"] != json!("feature-stray")),
        "not on the feed"
    );

    let main = branch_row(&mut state, "main");
    assert_eq!(main["kind"], "branch", "{main:?}");
    assert!(main["run_id"].is_null(), "{main:?}");
    assert_eq!(
        main["worktree_path"],
        std::fs::canonicalize(&repo).unwrap().display().to_string(),
        "the main row is the primary checkout: {main:?}"
    );

    // One row per branch: an adopted worktree is not also an external one.
    let rows = work_item_rows(&mut state);
    assert_eq!(
        rows.iter()
            .filter(|row| row["branch"] == "feature-adopted")
            .count(),
        1,
        "{rows:?}"
    );
}

/// Legacy issues remain directly readable but never re-enter the active board.
#[test]
fn retired_issues_are_absent_from_the_board_but_remain_readable() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let issue = state
        .plan_create(&json!({ "goal": "legacy issue", "dispatch": false }))
        .expect("legacy fixture is created below the retired RPC boundary");
    let issue_id = issue["plan_id"].as_str().unwrap().to_string();

    let rows = work_item_rows(&mut state);
    assert!(
        rows.iter().all(|row| row["kind"] != "issue"),
        "retired issues must not appear in active work: {rows:?}"
    );
    let board = state.handle(req("board.list", json!({})));
    assert!(board["result"].get("issues").is_none(), "{board:?}");
    assert!(board["result"].get("plans").is_none(), "{board:?}");

    let readable = state.handle(req("issue.get", json!({ "issue_id": issue_id })));
    assert_eq!(readable["ok"], true, "{readable:?}");
    assert_eq!(
        readable["result"]["goal"], "legacy issue",
        "the direct compatibility read keeps the stored record"
    );
}

/// The branch row's second line: how many files it touched, how far it is
/// from where it is published, and what it added and removed. One poll,
/// every number the inbox prints — nothing the SPA has to go and ask for.
#[test]
fn a_branch_row_carries_the_numbers_its_second_line_prints() {
    let (dir, repo, _origin) = init_repo_with_origin();
    let mut state = qa_state(&repo, dir.path());
    adopted_run(&mut state, &repo, dir.path(), "feature-counted");
    let worktree = dir.path().join("feature-counted");
    let before = branch_row(&mut state, "feature-counted")["stat"].clone();
    let activity_before_commit = branch_row(&mut state, "feature-counted")["last_activity"].clone();

    std::fs::write(worktree.join("one.txt"), "a\nb\n").unwrap();
    std::fs::write(worktree.join("two.txt"), "c\n").unwrap();
    git_in(&worktree, &["add", "."]);
    git_in(&worktree, &["commit", "-m", "two files"]);

    // No upstream yet: ahead/behind are measured against the base branch,
    // and `upstream: null` is what says so. That distinction is what Done
    // warns with — unmerged reads differently from unpushed.
    let unpublished = branch_row(&mut state, "feature-counted");
    let stat = &unpublished["stat"];
    let grew = |key: &str| stat[key].as_u64().unwrap() - before[key].as_u64().unwrap();
    assert_eq!(grew("files_changed"), 2, "{stat:?}");
    assert_eq!(grew("insertions"), 3, "{stat:?}");
    assert_eq!(grew("deletions"), 0, "{stat:?}");
    assert!(stat["upstream"].is_null(), "{stat:?}");
    assert_eq!(stat["comparison_ref"], "main", "{stat:?}");
    assert_eq!(stat["ahead"], 1, "{stat:?}");
    assert_eq!(stat["behind"], 0, "{stat:?}");

    git_in(&worktree, &["push", "-u", "origin", "feature-counted"]);
    let published = branch_row(&mut state, "feature-counted");
    let stat = &published["stat"];
    assert_eq!(stat["upstream"], "origin/feature-counted", "{stat:?}");
    assert_eq!(stat["comparison_ref"], "origin/feature-counted", "{stat:?}");
    assert_eq!(
        stat["ahead"], 0,
        "unpushed work is what ahead means now: {stat:?}"
    );
    assert_eq!(stat["behind"], 0, "{stat:?}");

    // And the row dates itself, so Recent can bucket it.
    let last_activity = published["last_activity"]
        .as_str()
        .unwrap_or_else(|| panic!("a branch row says when it last moved: {published:?}"));
    assert!(
        last_activity.as_bytes()[0].is_ascii_digit(),
        "an RFC 3339 instant: {last_activity}"
    );
    assert_eq!(
        published["last_activity"], activity_before_commit,
        "commits are not inbox activity"
    );
}

/// The diff cache is the file watcher: two computes that disagree are work
/// landing on disk, and that is what dates a branch nobody has committed
/// on. A recompute after an invalidation is not a filesystem event — there
/// was nothing to disagree with.
#[test]
fn a_moving_diffstat_is_what_dates_a_branch_between_commits() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let run_id = "run-watched".to_string();
    let stat = |files: u64| json!({ "files_changed": files, "insertions": files, "deletions": 0 });

    state.store_diff_entry(DiffCacheEntry::RunStat {
        run_id: run_id.clone(),
        stat: stat(1),
    });
    assert!(
        state.board.diff().run_files_changed_at(&run_id).is_none(),
        "the first compute has nothing to disagree with"
    );

    state.store_diff_entry(DiffCacheEntry::RunStat {
        run_id: run_id.clone(),
        stat: stat(1),
    });
    assert!(
        state.board.diff().run_files_changed_at(&run_id).is_none(),
        "an unchanged tree is not a change"
    );

    state.store_diff_entry(DiffCacheEntry::RunStat {
        run_id: run_id.clone(),
        stat: stat(2),
    });
    let changed_at = state
        .board
        .diff()
        .run_files_changed_at(&run_id)
        .map(str::to_string)
        .expect("files moved");
    assert!(changed_at > hours_ago(1), "stamped now: {changed_at}");

    // And it is the run's, so deleting the run takes it with them.
    state.invalidate_run_stat(&run_id);
    state.store_diff_entry(DiffCacheEntry::RunStat {
        run_id: run_id.clone(),
        stat: stat(9),
    });
    assert_eq!(
        state.board.diff().run_files_changed_at(&run_id),
        Some(changed_at.as_str()),
        "a recompute after an invalidation had nothing to compare against"
    );
}

/// Done on a branch deletes it, and the row says beforehand what deleting
/// it would cost — never that it cannot be done. The primary checkout is
/// the one exception: it is the repository, not a worktree to file away,
/// so there is nothing there to finish.
#[test]
fn a_branch_always_offers_done_and_says_what_it_would_cost() {
    let (dir, repo, _origin) = init_repo_with_origin();
    let mut state = qa_state(&repo, dir.path());
    adopted_run(&mut state, &repo, dir.path(), "feature-done");
    let worktree = dir.path().join("feature-done");

    // The agent's edits are sitting in the tree, and Done removes the tree.
    let fresh = branch_row(&mut state, "feature-done");
    assert_eq!(fresh["can_finish"], true, "{fresh:?}");
    assert_eq!(warning_codes(&fresh), vec!["uncommitted"], "{fresh:?}");

    // Committed, with no remote: the base branch is the only place the
    // work could survive Done, and it is not there.
    std::fs::write(worktree.join("work.txt"), "one\n").unwrap();
    git_in(&worktree, &["add", "-A"]);
    git_in(&worktree, &["commit", "-m", "work"]);
    let unmerged = branch_row(&mut state, "feature-done");
    assert_eq!(unmerged["can_finish"], true, "{unmerged:?}");
    assert_eq!(warning_codes(&unmerged), vec!["unmerged"], "{unmerged:?}");
    assert_eq!(
        unmerged["finish"]["warnings"][0]["count"], 1,
        "{unmerged:?}"
    );
    assert_eq!(
        unmerged["finish"]["warnings"][0]["ref"], "main",
        "{unmerged:?}"
    );

    git_in(&worktree, &["push", "-u", "origin", "feature-done"]);
    let pushed = branch_row(&mut state, "feature-done");
    assert!(
        warning_codes(&pushed).is_empty(),
        "the remote has all of it: {pushed:?}"
    );

    // An unsaved edit exists only here, and Done removes the checkout.
    std::fs::write(worktree.join("work.txt"), "one\ntwo\n").unwrap();
    let dirty = branch_row(&mut state, "feature-done");
    assert_eq!(warning_codes(&dirty), vec!["uncommitted"], "{dirty:?}");

    // Committed, and now the remote is the one behind.
    git_in(&worktree, &["commit", "-am", "more"]);
    let ahead = branch_row(&mut state, "feature-done");
    assert_eq!(warning_codes(&ahead), vec!["unpushed"], "{ahead:?}");
    assert_eq!(
        ahead["finish"]["warnings"][0]["ref"], "origin/feature-done",
        "{ahead:?}"
    );

    let main = branch_row(&mut state, "main");
    assert_eq!(
        main["can_finish"], false,
        "the primary checkout is the repository: {main:?}"
    );
    assert!(warning_codes(&main).is_empty(), "{main:?}");
}

/// `#/project/<id>/branch/<name>` resolves through one verb, to the run
/// underneath when there is one and to the bare checkout when there is not.
#[test]
fn branch_get_resolves_a_branch_to_what_is_underneath_it() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let run_id = adopted_run(&mut state, &repo, dir.path(), "feature-routed");
    add_external_worktree(&repo, dir.path(), "loose", "feature-loose");
    let project_id = state.project_at(0).id.clone();
    // Made behind Build's back, so it reaches the board the way anything
    // made outside Build does: on the next scan, not on the next read.
    state.scan_external_worktrees_now(&project_id).unwrap();

    let routed = state.handle(req(
        "branch.get",
        json!({ "project_id": project_id, "branch": "feature-routed" }),
    ));
    assert_eq!(routed["ok"], true, "{routed:?}");
    let routed = &routed["result"];
    assert_eq!(routed["kind"], "branch", "{routed:?}");
    assert_eq!(routed["run_id"], run_id, "{routed:?}");
    assert_eq!(routed["run"]["run_id"], run_id, "{routed:?}");
    assert!(
        routed["run"]["thread"]["items"].is_array(),
        "the underlying view carries the full conversation: {routed:?}"
    );

    let loose = state.handle(req(
        "branch.get",
        json!({ "project_id": project_id, "branch": "feature-loose" }),
    ));
    assert_eq!(loose["ok"], true, "{loose:?}");
    assert!(loose["result"]["run"].is_null(), "{loose:?}");
    assert!(loose["result"]["worktree_path"].is_string(), "{loose:?}");

    let missing = state.handle(req(
        "branch.get",
        json!({ "project_id": project_id, "branch": "never-existed" }),
    ));
    assert_eq!(missing["ok"], false, "{missing:?}");
}

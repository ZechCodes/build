use super::*;

// ==== the branch as the wire-level work item ==============================

/// The feed, with every poll cache cleared first. All three are 10s TTL, so
/// a test that changes git state and re-polls would otherwise be answered
/// from the poll before it.
pub(in crate::app::tests) fn work_item_rows(state: &mut AppState) -> Vec<Value> {
    state.run_stat_cache.clear();
    for index in 0..state.projects.len() {
        state.projects[index].primary_summary = None;
        state.projects[index].external_scan = None;
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
    let project_id = state.projects[0].id.clone();

    let board = state.handle(req("board.list", json!({})));
    // The transition keeps the old keys shipping alongside the new one.
    assert!(board["result"]["runs"].is_array(), "{board:?}");
    assert!(board["result"]["plans"].is_array(), "{board:?}");
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

/// Once implementation starts, the issue's work surfaces as the branch row
/// alone — and that row is what carries the issue id.
#[test]
fn an_issue_speaks_as_its_implementation_while_one_is_in_flight() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let waiting = state.handle(req("plan.create", json!({ "goal": "not started" })));
    let waiting_id = plan_id_of(&waiting);

    let rows = work_item_rows(&mut state);
    let issue = rows
        .iter()
        .find(|row| row["issue_id"] == json!(waiting_id.clone()))
        .unwrap_or_else(|| panic!("an unimplemented issue is its own row: {rows:?}"));
    assert_eq!(issue["kind"], "issue", "{issue:?}");
    assert_eq!(issue["title"], "not started", "{issue:?}");
    assert_eq!(issue["state"], "plan_review", "{issue:?}");
    assert!(issue["branch"].is_null(), "{issue:?}");
    // Done on an issue archives it and is always on offer; that nothing
    // was ever built for it is the warning it carries.
    assert_eq!(issue["can_finish"], true, "{issue:?}");
    assert_eq!(warning_codes(issue), vec!["unimplemented"], "{issue:?}");

    let (implemented_id, run_id) = planned_run_in_review(&mut state, "implement me");
    let rows = work_item_rows(&mut state);
    assert!(
        !rows
            .iter()
            .any(|row| row["kind"] == "issue" && row["issue_id"] == json!(implemented_id.clone())),
        "the issue row is suppressed while its implementation is live: {rows:?}"
    );
    let implementation = rows
        .iter()
        .find(|row| row["run_id"] == json!(run_id.clone()))
        .unwrap_or_else(|| panic!("the implementation is on the feed: {rows:?}"));
    assert_eq!(implementation["kind"], "branch", "{implementation:?}");
    assert_eq!(
        implementation["issue_id"],
        json!(implemented_id.clone()),
        "the branch row carries the issue it implements: {implementation:?}"
    );
    // The issue nothing was started for still warns about exactly that.
    let unimplemented = rows
        .iter()
        .find(|row| row["issue_id"] == json!(waiting_id.clone()))
        .expect("the untouched issue still has a row");
    assert_eq!(
        warning_codes(unimplemented),
        vec!["unimplemented"],
        "{unimplemented:?}"
    );

    // Abandoning the implementation hands the issue its own row back.
    let abandoned = state.handle(req("run.abandon", json!({ "run_id": run_id })));
    assert_eq!(abandoned["ok"], true, "{abandoned:?}");
    let rows = work_item_rows(&mut state);
    assert!(
        rows.iter()
            .any(|row| row["kind"] == "issue" && row["issue_id"] == json!(implemented_id)),
        "a finished implementation stops speaking for its issue: {rows:?}"
    );
}

/// An RFC 3339 stamp `hours` in the past, for tests that need a gap no
/// suite can wait out.
fn hours_ago(hours: i64) -> String {
    (time::OffsetDateTime::now_utc() - time::Duration::hours(hours))
        .format(&time::format_description::well_known::Rfc3339)
        .expect("UTC formats as RFC 3339")
}

fn issue_row(state: &mut AppState, issue_id: &str) -> Value {
    work_item_rows(state)
        .into_iter()
        .find(|row| row["kind"] == "issue" && row["issue_id"] == json!(issue_id))
        .unwrap_or_else(|| panic!("{issue_id} has a row on the feed"))
}

/// The inbox sorts by anchor, oldest first, so every row it can show has to
/// carry one — a branch, an issue, and the capture that has not become
/// either yet.
#[test]
fn every_feed_row_carries_the_anchor_it_sorts_by() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let issue_id = plan_id_of(&state.handle(req(
        "issue.create",
        json!({ "goal": "add a greeting", "dispatch": false }),
    )));
    adopted_run(&mut state, &repo, dir.path(), "feature-anchored");
    captured(&mut state, "something I said");

    let rows = work_item_rows(&mut state);
    for row in &rows {
        assert!(
            row["anchor"].as_str().is_some(),
            "every row sorts by an anchor: {row:?}"
        );
    }
    assert!(
        rows.iter().any(|row| row["kind"] == "issue")
            && rows.iter().any(|row| row["kind"] == "branch")
            && rows.iter().any(|row| row["kind"] == "capture"),
        "all three kinds are on this feed: {rows:?}"
    );

    // An issue enters the list where it was filed, and the detail surface
    // agrees with the row.
    let filed_at = state.entity_created_at[&issue_id].clone();
    let issue = rows
        .iter()
        .find(|row| row["issue_id"] == json!(issue_id.clone()))
        .expect("the issue is on the feed");
    assert_eq!(issue["anchor"], json!(filed_at.clone()), "{issue:?}");
    let detail = state.handle(req("issue.get", json!({ "issue_id": issue_id })));
    assert_eq!(detail["result"]["attention"]["anchor"], json!(filed_at));
}

/// Half a day of saying nothing and then saying something is picking the
/// work back up: it goes to the bottom of the inbox. Saying a second thing
/// straight after is the same sitting, and moves nothing.
#[test]
fn a_message_after_half_a_day_of_silence_moves_the_anchor() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let issue_id = plan_id_of(&state.handle(req(
        "issue.create",
        json!({ "goal": "add a greeting", "dispatch": false }),
    )));

    // Filed yesterday, and nothing said about it since.
    let filed_at = hours_ago(30);
    state
        .entity_created_at
        .insert(issue_id.clone(), filed_at.clone());
    let attention = state.attention.get_mut(&issue_id).expect("it is anchored");
    attention.anchor_at = Some(filed_at.clone());
    attention.last_user_message_at = Some(hours_ago(13));
    assert_eq!(issue_row(&mut state, &issue_id)["anchor"], json!(filed_at));

    let posted = state.handle(req(
        "thread.post",
        json!({ "entity_id": issue_id, "body": "still want this" }),
    ));
    assert_eq!(posted["ok"], true, "{posted:?}");
    let picked_up = state.anchor_of(&issue_id);
    assert!(
        picked_up > filed_at,
        "a message after twelve hours of silence moves it to now: {picked_up} vs {filed_at}"
    );
    assert_eq!(
        issue_row(&mut state, &issue_id)["anchor"],
        json!(picked_up.clone()),
        "the row sorts by the anchor that just moved"
    );

    // The rest of the conversation is one sitting.
    let again = state.handle(req(
        "thread.post",
        json!({ "entity_id": issue_id, "body": "and this too" }),
    ));
    assert_eq!(again["ok"], true, "{again:?}");
    assert_eq!(
        state.anchor_of(&issue_id),
        picked_up,
        "the inbox must not reshuffle while you type"
    );
}

/// The rule the whole ordering rests on: agents never move the list. A
/// planning session, an implementation and everything said on its
/// conversation leave the anchor exactly where the user left it — with the
/// silence clock long past the gap, so only a USER message could move it.
#[test]
fn an_agent_working_all_night_leaves_the_anchor_alone() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let issue_id = plan_id_of(&state.handle(req("plan.create", json!({ "goal": "implement me" }))));
    let anchored_at = hours_ago(30);
    let attention = state.attention.get_mut(&issue_id).expect("it is anchored");
    attention.anchor_at = Some(anchored_at.clone());
    attention.last_user_message_at = Some(anchored_at.clone());

    // The QA agent plans it, it is approved, and an implementation starts —
    // a night of work, all of it the agent's.
    let said_before = primary_thread(&state.plans[&issue_id].agents).items.len();
    state.handle(req("plan.approve", json!({ "plan_id": issue_id })));
    let dispatched = state.handle(req(
        "issue.implement_all",
        json!({ "issue_id": issue_id, "auto_advance": true }),
    ));
    assert_eq!(dispatched["ok"], true, "{dispatched:?}");
    assert!(
        primary_thread(&state.plans[&issue_id].agents).items.len() > said_before,
        "the agent has been talking"
    );

    assert_eq!(
        state.anchor_of(&issue_id),
        anchored_at,
        "nothing an agent does moves the inbox"
    );
}

/// What the user said and the work it became are ONE entry in the inbox:
/// routing hands the capture's place to the issue it becomes, rather than
/// filing the work as something new that arrived just now.
#[test]
fn routing_a_capture_hands_its_anchor_to_the_work() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.projects[0].id.clone();
    let (capture_id, _) = captured(&mut state, "fix the login redirect");
    let said_at = hours_ago(20);
    state
        .captures
        .get_mut(&capture_id)
        .expect("the capture is kept")
        .anchor_at = Some(said_at.clone());
    assert_eq!(
        capture_rows(&mut state)[0]["anchor"],
        json!(said_at.clone()),
        "the capture's row sorts by when it was said"
    );

    let routed = state.handle(req(
        "capture.reroute",
        json!({ "capture_id": capture_id, "project_id": project_id, "kind": "issue" }),
    ));
    assert_eq!(routed["ok"], true, "{routed:?}");
    let issue_id = routed["result"]["routing"]["target_id"]
        .as_str()
        .expect("the capture became an issue")
        .to_string();

    assert_eq!(
        state.anchor_of(&issue_id),
        said_at,
        "the work keeps the place the capture held"
    );
    assert_eq!(
        issue_row(&mut state, &issue_id)["anchor"],
        json!(said_at),
        "and the row that replaced the capture's says so"
    );

    // The other destination: a capture routed straight onto a branch hands
    // its place to the run that was cut for it.
    let (branch_capture, _) = captured(&mut state, "the login redirect again");
    let asked_at = hours_ago(40);
    state
        .captures
        .get_mut(&branch_capture)
        .expect("the capture is kept")
        .anchor_at = Some(asked_at.clone());
    let dispatched = state.handle(req(
        "capture.reroute",
        json!({ "capture_id": branch_capture, "project_id": project_id, "kind": "branch" }),
    ));
    assert_eq!(dispatched["ok"], true, "{dispatched:?}");
    let branch = dispatched["result"]["routing"]["target_id"]
        .as_str()
        .expect("the capture became a branch")
        .to_string();
    assert_eq!(
        branch_row(&mut state, &branch)["anchor"],
        json!(asked_at),
        "the branch holds the place the capture held"
    );
}

/// Anchors are durable, and every record written before they existed gets
/// the one it would have had on the next boot — seeded from when it was
/// created, never from when the daemon happened to restart.
#[test]
fn anchors_survive_a_restart_and_a_record_from_before_them_is_seeded() {
    let (dir, repo) = init_repo();
    let issue_id = {
        let mut state = qa_state(&repo, dir.path());
        let issue_id = plan_id_of(&state.handle(req(
            "issue.create",
            json!({ "goal": "add a greeting", "dispatch": false }),
        )));
        // A record from before anchors: the attention map has everything
        // else about it and nothing about where it sits.
        let attention = state.attention.get_mut(&issue_id).expect("it is anchored");
        attention.anchor_at = None;
        attention.last_user_message_at = None;
        attention.interact(&now_rfc3339());
        state.persist_attention();
        issue_id
    };

    let mut booted = qa_state(&repo, dir.path());
    let created_at = booted.entity_created_at[&issue_id].clone();
    assert_eq!(
        booted.anchor_of(&issue_id),
        created_at,
        "boot anchors it where it was created, not where the restart was"
    );
    assert_eq!(
        booted.attention[&issue_id].anchor_at.as_deref(),
        Some(created_at.as_str()),
        "and writes it down"
    );

    // Pick it up, restart again: the anchor the user moved is the anchor
    // the next boot finds.
    booted
        .attention
        .get_mut(&issue_id)
        .expect("anchored above")
        .last_user_message_at = Some(hours_ago(13));
    booted.handle(req(
        "thread.post",
        json!({ "entity_id": issue_id, "body": "still want this" }),
    ));
    let picked_up = booted.anchor_of(&issue_id);
    assert!(picked_up > created_at, "{picked_up} vs {created_at}");
    drop(booted);

    let rebooted = qa_state(&repo, dir.path());
    assert_eq!(
        rebooted.anchor_of(&issue_id),
        picked_up,
        "a restart never re-files what the user picked up"
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

/// An issue with a branch being built for it drops out of the inbox — so it
/// has to be able to say where it went. The branch names it; the issue
/// names the branch.
#[test]
fn an_issue_says_which_branch_is_implementing_it() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let issue_id = plan_id_of(&state.handle(req(
        "issue.create",
        json!({ "goal": "not started", "dispatch": false }),
    )));

    let idle = issue_row(&mut state, &issue_id);
    assert_eq!(idle["implementation_active"], false, "{idle:?}");
    assert!(idle["implementing_branch"].is_null(), "{idle:?}");

    let (implemented_id, run_id) = planned_run_in_review(&mut state, "implement me");
    let board = state.handle(req("board.list", json!({})));
    let issue = board["result"]["issues"]
        .as_array()
        .unwrap()
        .iter()
        .find(|issue| issue["issue_id"] == json!(implemented_id.clone()))
        .expect("the issue is still an issue")
        .clone();
    let branch = state.runs[&run_id].worktree.branch();
    assert_eq!(issue["implementation_active"], true, "{issue:?}");
    assert_eq!(issue["implementing_branch"], json!(branch), "{issue:?}");
    assert!(
        !work_item_rows(&mut state)
            .iter()
            .any(|row| row["kind"] == "issue" && row["issue_id"] == json!(implemented_id)),
        "and that is exactly why it is not in the inbox itself"
    );

    // The branch stops implementing it: the issue is its own row again, and
    // says nothing is being built for it.
    state.handle(req("run.abandon", json!({ "run_id": run_id })));
    let back = issue_row(&mut state, &implemented_id);
    assert_eq!(back["implementation_active"], false, "{back:?}");
    assert!(back["implementing_branch"].is_null(), "{back:?}");
}

/// Every event on an issue's conversation, as the wire ships them.
fn issue_events(state: &mut AppState, issue_id: &str) -> Vec<Value> {
    let issue = state.handle(req("issue.get", json!({ "issue_id": issue_id })));
    assert_eq!(issue["ok"], true, "{issue:?}");
    issue["result"]["thread"]["items"]
        .as_array()
        .expect("a conversation")
        .iter()
        .filter(|item| item["type"] == "event")
        .map(|item| item["data"].clone())
        .collect()
}

fn says_the_branch_was_abandoned(events: &[Value], branch: &str) -> bool {
    events.iter().any(|event| {
        event["event"] == "abandoned"
            && event["summary"]
                .as_str()
                .is_some_and(|summary| summary.contains(branch) && summary.contains("merged"))
    })
}

/// An issue whose branch is deleted with nothing merged comes back to the
/// inbox, and a row that reappears unexplained reads as the list losing
/// track of its own work. The conversation names the branch it lost.
#[test]
fn abandoning_a_branch_tells_its_issue_which_branch_it_lost() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (issue_id, run_id) = planned_run_in_review(&mut state, "implement me");
    let branch = state.runs[&run_id].worktree.branch();

    let abandoned = state.handle(req("run.abandon", json!({ "run_id": run_id })));
    assert_eq!(abandoned["ok"], true, "{abandoned:?}");

    let events = issue_events(&mut state, &issue_id);
    assert!(
        says_the_branch_was_abandoned(&events, &branch),
        "the issue must say which branch went and that nothing was merged: {events:?}"
    );
    let back = issue_row(&mut state, &issue_id);
    assert_eq!(back["implementation_active"], false, "{back:?}");
    assert_eq!(
        back["unread"], true,
        "an issue that is waiting for work again is asking for someone: {back:?}"
    );
}

/// Same story with nobody to tell it: the checkout is deleted outside
/// Build, the sweep notices on the next poll, and the issue still explains
/// itself.
#[test]
fn a_checkout_deleted_outside_build_still_explains_the_issue_it_returns() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (issue_id, run_id) = planned_run_in_review(&mut state, "implement me");
    let branch = state.runs[&run_id].worktree.branch();
    let worktree = state.runs[&run_id].worktree.path.clone();

    std::fs::remove_dir_all(&worktree).expect("the user deleted their worktree");
    // The sweep runs on the poll the inbox already makes.
    state.handle(req("board.list", json!({})));

    let events = issue_events(&mut state, &issue_id);
    assert!(
        says_the_branch_was_abandoned(&events, &branch),
        "{events:?}"
    );
    assert!(
        work_item_rows(&mut state)
            .iter()
            .any(|row| row["kind"] == "issue" && row["issue_id"] == json!(issue_id)),
        "and the issue is back in the inbox"
    );
}

/// The inbox reads oldest first, and the bridge hands it over in that
/// order: a fresh pickup appends to the bottom instead of shoving what has
/// been waiting longest down the list.
#[test]
fn the_feed_arrives_oldest_first() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let older = plan_id_of(&state.handle(req(
        "issue.create",
        json!({ "goal": "filed last week", "dispatch": false }),
    )));
    let newer = plan_id_of(&state.handle(req(
        "issue.create",
        json!({ "goal": "filed yesterday", "dispatch": false }),
    )));
    state
        .attention
        .get_mut(&older)
        .expect("anchored at creation")
        .anchor_at = Some(hours_ago(200));
    state
        .attention
        .get_mut(&newer)
        .expect("anchored at creation")
        .anchor_at = Some(hours_ago(20));

    let order: Vec<String> = work_item_rows(&mut state)
        .iter()
        .filter_map(|row| row["issue_id"].as_str().map(str::to_string))
        .collect();
    assert_eq!(
        order,
        vec![older.clone(), newer.clone()],
        "oldest at the top"
    );

    // Picking the older one back up sends it to the bottom.
    state
        .attention
        .get_mut(&older)
        .expect("anchored above")
        .last_user_message_at = Some(hours_ago(13));
    state.handle(req(
        "thread.post",
        json!({ "entity_id": older, "body": "picking this back up" }),
    ));
    let order: Vec<String> = work_item_rows(&mut state)
        .iter()
        .filter_map(|row| row["issue_id"].as_str().map(str::to_string))
        .collect();
    assert_eq!(order, vec![newer, older], "a fresh pickup appends");
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
        !state.run_files_changed_at.contains_key(&run_id),
        "the first compute has nothing to disagree with"
    );

    state.store_diff_entry(DiffCacheEntry::RunStat {
        run_id: run_id.clone(),
        stat: stat(1),
    });
    assert!(
        !state.run_files_changed_at.contains_key(&run_id),
        "an unchanged tree is not a change"
    );

    state.store_diff_entry(DiffCacheEntry::RunStat {
        run_id: run_id.clone(),
        stat: stat(2),
    });
    let changed_at = state
        .run_files_changed_at
        .get(&run_id)
        .cloned()
        .expect("files moved");
    assert!(changed_at > hours_ago(1), "stamped now: {changed_at}");

    // And it is the run's, so deleting the run takes it with them.
    state.invalidate_run_stat(&run_id);
    state.store_diff_entry(DiffCacheEntry::RunStat {
        run_id: run_id.clone(),
        stat: stat(9),
    });
    assert_eq!(
        state.run_files_changed_at.get(&run_id),
        Some(&changed_at),
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
    let project_id = state.projects[0].id.clone();
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

/// An issue ends with its branch only when the branch's work landed. A
/// merge finishes both; deleting the branch instead hands the issue back
/// to the inbox, with its conversation naming the branch it lost.
#[test]
#[allow(clippy::cognitive_complexity)] // ratchet: branch_finish_ends_the_issue_only_when_the_work_was_merged is at 16, threshold 15 — bring it under, then remove
fn branch_finish_ends_the_issue_only_when_the_work_was_merged() {
    let (dir, repo, _origin) = init_repo_with_origin();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.projects[0].id.clone();

    let (merged_issue, merged_run) = planned_run_in_review(&mut state, "merged done");
    let merged_branch = state.runs[&merged_run].worktree.branch();
    let finished = state.handle(req(
        "branch.finish",
        json!({ "project_id": project_id, "branch": merged_branch, "action": "merge" }),
    ));
    assert_eq!(finished["ok"], true, "{finished:?}");
    assert_eq!(finished["result"]["issue_id"], merged_issue, "{finished:?}");
    assert_eq!(finished["result"]["issue_archived"], true, "{finished:?}");
    assert_eq!(finished["result"]["issue_abandoned"], false, "{finished:?}");
    assert!(
        state.plans[&merged_issue].plan.archived_at.is_some(),
        "the work landed, so the issue is done with it"
    );

    // Deleted instead: the issue is waiting for work again, and says so.
    let (kept_issue, kept_run) = planned_run_in_review(&mut state, "deleted branch");
    let kept_branch = state.runs[&kept_run].worktree.branch();
    let deleted = state.handle(req(
        "branch.finish",
        json!({ "project_id": project_id, "branch": kept_branch, "action": "delete" }),
    ));
    assert_eq!(deleted["ok"], true, "{deleted:?}");
    assert_eq!(deleted["result"]["issue_archived"], false, "{deleted:?}");
    assert_eq!(deleted["result"]["issue_abandoned"], true, "{deleted:?}");
    assert!(
        state.plans[&kept_issue].plan.archived_at.is_none(),
        "nothing was merged, so the issue is not done"
    );
    let events = issue_events(&mut state, &kept_issue);
    assert!(
        says_the_branch_was_abandoned(&events, &kept_branch),
        "{events:?}"
    );
    // The issue that was hidden behind that branch is back in the inbox,
    // and the merged one — done — is not.
    let rows = work_item_rows(&mut state);
    assert!(
        rows.iter()
            .any(|row| row["issue_id"] == json!(kept_issue.clone())),
        "the issue is waiting for work again: {rows:?}"
    );
    assert!(
        !rows
            .iter()
            .any(|row| row["issue_id"] == json!(merged_issue.clone())),
        "nothing marked done is ever shown: {rows:?}"
    );

    // …and unlink is the control that leaves the issue out of it entirely.
    let (unlinked_issue, unlinked_run) = planned_run_in_review(&mut state, "unlinked done");
    let unlinked_branch = state.runs[&unlinked_run].worktree.branch();
    let unlinked = state.handle(req(
        "branch.finish",
        json!({
            "project_id": project_id,
            "branch": unlinked_branch,
            "action": "merge",
            "unlink": true,
        }),
    ));
    assert_eq!(unlinked["ok"], true, "{unlinked:?}");
    assert_eq!(unlinked["result"]["issue_archived"], false, "{unlinked:?}");
    assert!(state.plans[&unlinked_issue].plan.archived_at.is_none());
}

/// Done on a branch is the user's decision, not the bridge's. Work that
/// exists only on this machine is a warning the row carried all along —
/// the verb still deletes the branch, its checkout and its records.
#[test]
fn branch_finish_deletes_unpushed_work_it_warned_about() {
    let (dir, repo, _origin) = init_repo_with_origin();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.projects[0].id.clone();
    let (issue_id, run_id) = planned_run_in_review(&mut state, "never pushed");
    let branch = state.runs[&run_id].worktree.branch();
    let worktree = state.runs[&run_id].worktree.path.clone();

    // The preflight the confirm dialog reads, before anything is touched.
    let preflight = state.handle(req(
        "branch.get",
        json!({ "project_id": project_id, "branch": branch }),
    ));
    assert_eq!(preflight["ok"], true, "{preflight:?}");
    assert_eq!(preflight["result"]["can_finish"], true, "{preflight:?}");
    assert_eq!(
        warning_codes(&preflight["result"]),
        vec!["unmerged"],
        "{preflight:?}"
    );

    // No action at all: Done means delete.
    let finished = state.handle(req(
        "branch.finish",
        json!({ "project_id": project_id, "branch": branch }),
    ));
    assert_eq!(finished["ok"], true, "{finished:?}");
    assert_eq!(finished["result"]["worktree"]["action"], "delete");
    assert!(!worktree.exists(), "the checkout is gone: {finished:?}");
    assert!(
        !local_branch_exists(&std::fs::canonicalize(&repo).unwrap(), &branch).unwrap(),
        "the branch is gone with it"
    );
    assert!(
        !work_item_rows(&mut state)
            .iter()
            .any(|row| row["branch"] == json!(branch.clone())),
        "and its conversation is out of the inbox"
    );
    // The issue it was built for is still waiting for work.
    assert!(state.plans[&issue_id].plan.archived_at.is_none());
}

use super::*;

// ==== Plan/Run split: new-protocol coverage ================================

/// An issue filed with `dispatch: false` is a record and nothing else: the
/// user opens it, and the agent session starts when they say something.
#[test]
fn issue_create_without_dispatch_files_an_inert_issue() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());

    let res = state.handle(req(
        "issue.create",
        json!({ "goal": "add a greeting", "dispatch": false }),
    ));

    assert_eq!(res["ok"], true, "{res:?}");
    assert_eq!(res["result"]["state"], "created", "{res:?}");
    assert!(
        res["result"]["stages"].as_array().unwrap().is_empty(),
        "nothing has been planned yet: {res:?}"
    );
    let issue_id = plan_id_of(&res);
    let active = state.plans.get(&issue_id).expect("the record is filed");
    assert!(
        active.workspace.is_none(),
        "an inert issue has no session and so no workspace"
    );
    assert!(
        state.pending_agent_turns.is_empty(),
        "no session was dispatched"
    );
    // The goal is the conversation's first message, and no agent has read it.
    let first = &res["result"]["thread"]["items"][0];
    assert_eq!(first["data"]["role"], "user", "{res:?}");
    assert_eq!(first["data"]["body"], "add a greeting", "{res:?}");
    assert_eq!(first["data"]["seen_at"], Value::Null, "{res:?}");
    // …and it is on the feed as an issue row the user can open.
    let board = state.handle(req("board.list", json!({})));
    assert!(
        board["result"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["kind"] == "issue" && item["issue_id"] == json!(issue_id.clone())),
        "{board:?}"
    );
}

/// Every checkout git knows about for a repo, primary first.
fn registered_checkouts(repo: &std::path::Path) -> Vec<String> {
    let out = Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(repo)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| line.strip_prefix("worktree ").map(str::to_string))
        .collect()
}

/// An issue's agent works in the project's primary checkout and nowhere
/// else: no worktree is cut for planning, and the docs it writes live in a
/// scratch dir outside the repo that the store ingests from.
#[test]
fn an_issue_agent_runs_on_the_primary_checkout_and_cuts_no_worktree() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let issue_id = plan_id_of(&state.handle(req(
        "issue.create",
        json!({ "goal": "add a greeting", "dispatch": false }),
    )));

    let posted = state.handle(req(
        "thread.post",
        json!({ "entity_id": issue_id, "body": "Start with the endpoint." }),
    ));

    assert_eq!(posted["ok"], true, "{posted:?}");
    let primary = std::fs::canonicalize(&repo).unwrap();
    assert_eq!(
        registered_checkouts(&repo).len(),
        1,
        "planning cut a worktree: {:?}",
        registered_checkouts(&repo)
    );
    let workspace = state.plans[&issue_id]
        .workspace
        .as_ref()
        .expect("the planning session got a workspace");
    assert_eq!(workspace.checkout, primary);
    assert!(
        !workspace.docs_dir.starts_with(&primary),
        "the scratch docs dir is outside the repo: {}",
        workspace.docs_dir.display()
    );
    // The turn — and therefore the PTY it spawns — is addressed to the
    // primary checkout.
    assert_eq!(
        state.pending_agent_turns.len(),
        1,
        "one turn was dispatched"
    );
    assert_eq!(state.pending_agent_turns[0].root, primary);
    assert_eq!(
        state.entity_agent_root(&issue_id).unwrap(),
        primary,
        "every surface that asks where this issue's agent lives says the \
         primary checkout"
    );
}

/// An issue that has no stages yet still answers `issue.stages` — with the
/// empty list that is the truth. The surface reads issue.get and
/// issue.stages together on every poll, so an error here (the old "not a
/// multi-stage issue" refusal) left a fresh issue's page loading forever.
#[test]
fn an_issue_with_no_stages_answers_with_an_empty_stage_list() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let issue_id = plan_id_of(&state.handle(req(
        "issue.create",
        json!({ "goal": "add a greeting", "dispatch": false }),
    )));

    let stages = state.handle(req("issue.stages", json!({ "issue_id": issue_id })));

    assert_eq!(stages["ok"], true, "{stages:?}");
    assert_eq!(stages["result"]["issue_id"], issue_id);
    assert!(
        stages["result"]["stages"].as_array().unwrap().is_empty(),
        "{stages:?}"
    );
}

/// The first message is what starts the planning session — that is the whole
/// point of filing an issue inert.
#[test]
fn the_first_message_to_an_inert_issue_starts_its_planning_session() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let issue_id = plan_id_of(&state.handle(req(
        "issue.create",
        json!({ "goal": "add a greeting", "dispatch": false }),
    )));

    let posted = state.handle(req(
        "thread.post",
        json!({
            "entity_id": issue_id,
            "body": "Start with the endpoint."
        }),
    ));

    assert_eq!(posted["ok"], true, "{posted:?}");
    // The QA planning agent ran, so the issue lands where plan.create leaves
    // one — the dispatch this post triggered is the same dispatch.
    assert_eq!(posted["result"]["state"], "plan_review", "{posted:?}");
    let active = state.plans.get(&issue_id).expect("the issue is still here");
    let docs_dir = active
        .workspace
        .as_ref()
        .expect("the planning session got a workspace")
        .docs_dir
        .clone();
    assert!(docs_dir.is_dir(), "{docs_dir:?}");
    assert_eq!(
        state.pending_agent_turns.len(),
        1,
        "exactly one turn was dispatched"
    );
    let queued = &state.pending_agent_turns[0];
    assert_eq!(queued.owner, issue_id);
    let delivered =
        state.cold_prompt_with_catch_up(&queued.owner, &queued.agent_id, &queued.said().cold);
    assert!(
        delivered.contains("Start with the endpoint."),
        "the message that started the session is in the prompt it is handed: {delivered}"
    );

    // A second message steers the session it already has; it never mints a
    // second one.
    let again = state.handle(req(
        "thread.post",
        json!({
            "entity_id": issue_id,
            "body": "And a test."
        }),
    ));
    assert_eq!(again["ok"], true, "{again:?}");
    assert_eq!(
        state.plans[&issue_id]
            .workspace
            .as_ref()
            .map(|workspace| workspace.docs_dir.clone()),
        Some(docs_dir),
        "the same planning workspace"
    );
    assert_eq!(state.pending_agent_turns.len(), 1, "no second dispatch");
}

#[test]
fn plan_create_reaches_review_with_two_stages() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let res = state.handle(req("plan.create", json!({ "goal": "add a greeting" })));
    assert_eq!(res["ok"], true, "{res:?}");
    assert_eq!(res["result"]["state"], "plan_review");
    assert_eq!(res["result"]["docs_available"], true, "{res:?}");
    assert_eq!(res["result"]["stages"].as_array().unwrap().len(), 2);
    assert!(res["result"]["created_at"]
        .as_str()
        .is_some_and(|s| !s.is_empty()));
    let first_item = &res["result"]["thread"]["items"][0];
    assert_eq!(first_item["type"], "message", "{res:?}");
    assert_eq!(first_item["data"]["role"], "user", "{res:?}");
    assert_eq!(first_item["data"]["body"], "add a greeting", "{res:?}");
    let plan_id = plan_id_of(&res);

    // Each stage doc is readable from the canonical store, never a worktree.
    let doc = state.handle(req(
        "plan.stage_doc",
        json!({ "plan_id": plan_id, "stage_id": "first-half" }),
    ));
    assert_eq!(doc["ok"], true, "{doc:?}");
    assert!(doc["result"]["contents"]
        .as_str()
        .unwrap()
        .contains("add a greeting"));
}

#[test]
fn plan_send_notes_accepts_a_conversation_message_for_multi_stage_plans() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let plan = state.handle(req("plan.create", json!({ "goal": "make it staged" })));
    let plan_id = plan_id_of(&plan);

    let revised = state.handle(req(
        "plan.send_notes",
        json!({
            "plan_id": plan_id,
            "messages": [{ "body": "Keep the second stage reversible.", "anchor": null }]
        }),
    ));

    assert_eq!(revised["ok"], true, "{revised:?}");
    assert_eq!(revised["result"]["state"], "plan_review", "{revised:?}");
    assert!(revised["result"]["thread"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["type"] == "message"
            && item["data"]["role"] == "user"
            && item["data"]["body"] == "Keep the second stage reversible."));
}

#[test]
#[allow(clippy::cognitive_complexity)] // ratchet: full_multi_stage_lifecycle_plan_then_run is at 16, threshold 15 — bring it under, then remove
fn full_multi_stage_lifecycle_plan_then_run() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());

    let plan = state.handle(req(
        "plan.create",
        json!({
            "goal": "add a greeting",
            "provider": "codex",
            "model": "gpt-5.6-sol",
            "effort": "ultra"
        }),
    ));
    let plan_id = plan_id_of(&plan);
    assert_eq!(plan["result"]["provider"], "codex", "{plan:?}");
    let approved = state.handle(req("plan.approve", json!({ "plan_id": plan_id })));
    assert_eq!(approved["result"]["state"], "approved", "{approved:?}");
    state.handle(req(
        "plan.stage_approve",
        json!({ "plan_id": plan_id, "stage_id": "first-half" }),
    ));

    // Implement it: stage 1 auto-dispatches, QA drives it to the stage gate.
    let run = state.handle(req("run.create", json!({ "plan_id": plan_id })));
    assert_eq!(run["ok"], true, "{run:?}");
    assert_eq!(run["result"]["state"], "stage_gate", "{run:?}");
    assert_eq!(run["result"]["plan_id"], json!(plan_id));
    assert_eq!(run["result"]["provider"], "codex", "{run:?}");
    assert_eq!(run["result"]["model"], "gpt-5.6-sol", "{run:?}");
    assert_eq!(run["result"]["effort"], "ultra", "{run:?}");
    let run_id = run_id_of(&run);
    assert_eq!(run["result"]["stages"][0]["state"], "validated_passed");

    // Approve stage 2's doc, then dispatch it → the final validation opens review.
    state.handle(req(
        "plan.stage_approve",
        json!({ "plan_id": plan_id, "stage_id": "second-half" }),
    ));
    let s2 = state.handle(req(
        "run.stage_dispatch",
        json!({ "run_id": run_id, "stage_id": "second-half" }),
    ));
    assert_eq!(s2["result"]["state"], "review", "{s2:?}");

    let diff = state.handle(req("run.diff", json!({ "run_id": run_id })));
    let files: Vec<String> = diff["result"]["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["path"].as_str().unwrap().to_string())
        .collect();
    assert!(
        files.contains(&"result-first-half.txt".to_string()),
        "{files:?}"
    );
    assert!(
        files.contains(&"result-second-half.txt".to_string()),
        "{files:?}"
    );
    let edited_at = diff["result"]["file_edited_at"].as_object().unwrap();
    assert!(edited_at["result-first-half.txt"].as_u64().unwrap() > 0);
    assert!(edited_at["result-second-half.txt"].as_u64().unwrap() > 0);

    let merged = state.handle(req(
        "run.git_action",
        json!({ "run_id": run_id, "action": "merge" }),
    ));
    assert_eq!(merged["result"]["state"], "merged", "{merged:?}");
    assert!(repo.join("result-first-half.txt").exists());
    assert!(repo.join("result-second-half.txt").exists());
}

/// Every run implements a plan. The goal-only dispatch ("Quick task") is
/// gone: an ad-hoc coding session is now a `claude`/`codex` tab the human
/// drives (`term.create`), not a task-lifecycle run nobody planned.
#[test]
fn run_create_refuses_a_goal_without_a_plan() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let res = state.handle(req("run.create", json!({ "goal": "quick change" })));
    assert_eq!(res["ok"], false, "{res:?}");
    assert!(
        res["error"].as_str().unwrap().contains("plan_id"),
        "{res:?}"
    );
    assert!(state.runs.is_empty(), "nothing was dispatched");
}

#[test]
fn planned_run_builds_reviews_and_merges_with_a_cached_diffstat() {
    let (dir, repo, _origin) = init_repo_with_origin();
    let mut state = qa_state(&repo, dir.path());
    let (_, run_id) = planned_run_in_review(&mut state, "quick change");
    let worktree_path = state.runs[&run_id].worktree.path.clone();
    let branch = state.runs[&run_id].worktree.branch();
    git_in(&worktree_path, &["push", "-u", "origin", &branch]);
    std::fs::write(worktree_path.join("uncommitted.txt"), "one\ntwo\n").unwrap();
    let res = state.handle(req("run.get", json!({ "run_id": run_id })));
    assert_eq!(res["result"]["state"], "review", "{res:?}");

    let entry = |res: &Value| {
        res["result"]["runs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["run_id"] == json!(run_id.clone()))
            .unwrap()
            .clone()
    };
    let t = entry(&state.handle(req("board.list", json!({}))));
    assert!(t["stat"]["files_changed"].as_u64().unwrap() >= 1, "{t:?}");
    assert_eq!(
        t["stat"]["comparison_ref"],
        format!("origin/{branch}"),
        "{t:?}"
    );
    assert_eq!(t["stat"]["ahead"], 0, "pushed branch must be level: {t:?}");
    assert_eq!(t["stat"]["behind"], 0, "{t:?}");
    assert_eq!(t["stat"]["uncommitted"]["files_changed"], 1, "{t:?}");
    assert_eq!(t["stat"]["uncommitted"]["insertions"], 2, "{t:?}");
    assert_eq!(t["stat"]["uncommitted"]["deletions"], 0, "{t:?}");
    // served from cache on the next poll (identical).
    let t2 = entry(&state.handle(req("board.list", json!({}))));
    assert_eq!(t["stat"], t2["stat"]);

    state.handle(req(
        "run.git_action",
        json!({ "run_id": run_id, "action": "commit" }),
    ));
    assert!(
        !repo.join("result-first-half.txt").exists(),
        "commit keeps, no merge"
    );

    let merged = state.handle(req(
        "run.git_action",
        json!({ "run_id": run_id, "action": "merge" }),
    ));
    assert_eq!(merged["result"]["state"], "merged");
    assert!(repo.join("result-first-half.txt").exists());
    let t3 = entry(&state.handle(req("board.list", json!({}))));
    assert!(t3["stat"].is_null(), "merged run has no worktree: {t3:?}");
}

#[test]
fn failed_push_retains_durable_candidate_journal() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (_, run_id) = planned_run_in_review(&mut state, "journal failed push");

    let failed = state.handle(req(
        "run.git_action",
        json!({ "run_id": run_id, "action": "push" }),
    ));
    assert_eq!(failed["ok"], false, "{failed:?}");
    let record = Store::new(dir.path().join("store"))
        .expect("store opens")
        .load_all_runs()
        .unwrap()
        .into_iter()
        .find(|run| run.id == run_id)
        .unwrap();
    let attempt = record
        .publication_attempt
        .expect("intent is durable before uncertain side effect");
    assert_eq!(attempt.action, "push");
    assert_eq!(attempt.candidate_sha.len(), 40);
}

#[test]
fn run_stat_uses_the_checked_out_branch_upstream_after_a_rename() {
    let (dir, repo, _origin) = init_repo_with_origin();
    let mut state = qa_state(&repo, dir.path());
    let (_, run_id) = planned_run_in_review(&mut state, "renamed branch");
    let worktree_path = state.runs[&run_id].worktree.path.clone();
    let original_branch = state.runs[&run_id].worktree.branch();
    git_in(&worktree_path, &["push", "-u", "origin", &original_branch]);
    git_in(
        &worktree_path,
        &["branch", "-m", "build/actually-checked-out"],
    );
    git_in(
        &worktree_path,
        &["push", "-u", "origin", "build/actually-checked-out"],
    );

    let board = state.handle(req("board.list", json!({})));
    let run = board["result"]["runs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|run| run["run_id"] == run_id)
        .unwrap();
    assert_eq!(
        run["stat"]["comparison_ref"], "origin/build/actually-checked-out",
        "{run:?}"
    );
    assert_eq!(run["stat"]["ahead"], 0, "{run:?}");
    assert_eq!(run["stat"]["behind"], 0, "{run:?}");
}

#[test]
fn issue_facade_preserves_plan_identity_and_board_compatibility() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let created = state.handle(req("issue.create", json!({ "goal": "canonical issue" })));
    assert_eq!(created["ok"], true, "{created:?}");
    let issue_id = created["result"]["issue_id"].as_str().unwrap().to_string();
    assert_eq!(created["result"]["plan_id"], issue_id);

    let issue = state.handle(req("issue.get", json!({ "issue_id": issue_id })));
    let legacy = state.handle(req("plan.get", json!({ "plan_id": issue_id })));
    assert_eq!(issue["result"]["issue_id"], issue_id);
    assert_eq!(issue["result"]["plan_id"], issue_id);
    assert_eq!(legacy["result"]["issue_id"], issue_id);
    assert_eq!(issue["result"]["goal"], legacy["result"]["goal"]);

    let listed = state.handle(req("issue.list", json!({})));
    assert_eq!(listed["result"]["issues"].as_array().unwrap().len(), 1);
    let board = state.handle(req("board.list", json!({})));
    assert_eq!(board["result"]["issues"], board["result"]["plans"]);

    let stages = state.handle(req("issue.stages", json!({ "issue_id": issue_id })));
    assert_eq!(stages["result"]["issue_id"], issue_id);
    assert_eq!(stages["result"]["plan_id"], issue_id);
    assert_eq!(stages["result"]["stages"].as_array().unwrap().len(), 2);
    let store = Store::new(dir.path().join("store")).expect("store opens");
    assert!(store.issue_exists(&issue_id));
}

#[test]
fn disappearing_issue_worktree_invalidates_local_stage_completion_but_preserves_boundaries() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (issue_id, run_id) = planned_run_in_review(&mut state, "lost local worktree");
    let before = state.runs[&run_id].stages[0].clone();
    assert!(before.completion_sha.is_some());
    let worktree = state.runs[&run_id].worktree.path.clone();
    assert!(Command::new("git")
        .args([
            "worktree",
            "remove",
            "--force",
            "--",
            worktree.to_str().unwrap()
        ])
        .current_dir(&repo)
        .status()
        .unwrap()
        .success());

    state.handle(req("board.list", json!({})));
    let stages = state.handle(req("issue.stages", json!({ "issue_id": issue_id })));
    let first = &stages["result"]["stages"][0];
    assert_eq!(first["execution"], "incomplete", "{stages:?}");
    assert_eq!(first["start_sha"], before.start_sha.unwrap());
    assert_eq!(first["completion_sha"], before.completion_sha.unwrap());
    assert!(first["invalidation_reason"]
        .as_str()
        .unwrap()
        .contains("worktree"));
}

#[test]
fn abandoning_an_unpublished_issue_worktree_marks_completed_stages_incomplete() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (issue_id, run_id) = planned_run_in_review(&mut state, "abandon local lineage");
    let abandoned = state.handle(req("run.abandon", json!({ "run_id": run_id })));
    assert_eq!(abandoned["ok"], true, "{abandoned:?}");
    let stages = state.handle(req("issue.stages", json!({ "issue_id": issue_id })));
    assert!(
        stages["result"]["stages"]
            .as_array()
            .unwrap()
            .iter()
            .all(|stage| stage["execution"] == "incomplete"
                && stage["invalidation_reason"].as_str().is_some()),
        "{stages:?}"
    );
    let issue = state.handle(req("issue.get", json!({ "issue_id": issue_id })));
    let deleted = issue["result"]["thread"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["type"] == "event" && item["data"]["event"] == "worktree_deleted")
        .expect("worktree deletion is journaled");
    let links = deleted["data"]["links"].as_array().unwrap();
    assert!(links.iter().any(|link| link["kind"] == "implementation"));
    assert!(links.iter().any(|link| link["kind"] == "worktree"));
    assert!(links.iter().any(|link| link["kind"] == "issue_stage"));
}

#[test]
fn disappearing_worktree_keeps_pushed_stage_commits_complete() {
    let (dir, repo, _origin) = init_repo_with_origin();
    let mut state = qa_state(&repo, dir.path());
    let (issue_id, run_id) = planned_run_in_review(&mut state, "published worktree");
    let pushed = state.handle(req(
        "run.git_action",
        json!({ "run_id": run_id, "action": "push" }),
    ));
    assert_eq!(pushed["ok"], true, "{pushed:?}");
    let issue = state.handle(req("issue.get", json!({ "issue_id": issue_id })));
    assert!(
        issue["result"]["thread"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["type"] == "event"
                && item["data"]["event"] == "pushed"
                && item["data"]["links"]
                    .as_array()
                    .is_some_and(|links| links
                        .iter()
                        .any(|link| link["kind"] == "implementation"
                            && link["issue_id"] == issue_id
                            && link["implementation_id"] == run_id))),
        "{issue:?}"
    );
    let before_delete = state.handle(req("issue.stages", json!({ "issue_id": issue_id })));
    assert!(
        before_delete["result"]["stages"]
            .as_array()
            .unwrap()
            .iter()
            .all(|stage| stage["publication"] == "pushed"),
        "{before_delete:?}"
    );
    let worktree = state.runs[&run_id].worktree.path.clone();
    assert!(Command::new("git")
        .args([
            "worktree",
            "remove",
            "--force",
            "--",
            worktree.to_str().unwrap()
        ])
        .current_dir(&repo)
        .status()
        .unwrap()
        .success());

    state.handle(req("board.list", json!({})));
    let stages = state.handle(req("issue.stages", json!({ "issue_id": issue_id })));
    assert!(
        stages["result"]["stages"]
            .as_array()
            .unwrap()
            .iter()
            .all(|stage| stage["execution"] == "complete"
                && stage["publication"] == "pushed"
                && stage["invalidation_reason"].is_null()),
        "{stages:?}"
    );
    let stable = state.handle(req(
        "issue.stage_diff",
        json!({ "issue_id": issue_id, "stage_id": "first-half" }),
    ));
    assert_eq!(stable["ok"], true, "{stable:?}");
    assert_eq!(stable["result"]["status"], "available", "{stable:?}");
    assert!(stable["result"]["patch"]
        .as_str()
        .unwrap()
        .contains("result-first-half.txt"));
}

#[test]
fn issue_implement_all_runs_sequentially_and_exposes_stable_stage_diff() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let issue = state.handle(req("issue.create", json!({ "goal": "canonical all" })));
    let issue_id = issue["result"]["issue_id"].as_str().unwrap().to_string();
    for stage_id in ["first-half", "second-half"] {
        state.handle(req(
            "issue.stage_approve",
            json!({ "issue_id": issue_id, "stage_id": stage_id }),
        ));
    }
    state.handle(req("issue.approve", json!({ "issue_id": issue_id })));
    let implemented = state.handle(req("issue.implement_all", json!({ "issue_id": issue_id })));
    assert_eq!(implemented["ok"], true, "{implemented:?}");
    assert_eq!(
        implemented["result"]["current_implementation"]["state"],
        "review"
    );
    assert_eq!(
        implemented["result"]["implementation_lineage"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let lifecycle = implemented["result"]["thread"]["items"].as_array().unwrap();
    assert!(lifecycle
        .iter()
        .any(|item| item["data"]["event"] == "worktree_created"));
    assert!(
        !lifecycle
            .iter()
            .any(|item| item["data"]["event"] == "worktree_reused"),
        "the scheduler must not relabel its newly-created checkout as reused: {lifecycle:?}"
    );
    let issue_diff = state.handle(req("issue.diff", json!({ "issue_id": issue_id })));
    assert_eq!(issue_diff["ok"], true, "{issue_diff:?}");
    assert_eq!(issue_diff["result"]["issue_id"], issue_id);
    assert!(issue_diff["result"]["patch"]
        .as_str()
        .is_some_and(|patch| !patch.is_empty()));

    let diff = state.handle(req(
        "issue.stage_diff",
        json!({ "issue_id": issue_id, "stage_id": "first-half" }),
    ));
    assert_eq!(diff["result"]["issue_id"], issue_id);
    assert_eq!(diff["result"]["status"], "available");
    assert!(diff["result"]["patch"]
        .as_str()
        .unwrap()
        .contains("result-first-half.txt"));
    assert!(!diff["result"]["patch"]
        .as_str()
        .unwrap()
        .contains("result-second-half.txt"));
}

/// An approved Issue, an already-approved set of stages, and the id of the
/// checkout its implementation should run in.
fn issue_ready_to_implement(state: &mut AppState, goal: &str) -> String {
    let issue = state.handle(req("issue.create", json!({ "goal": goal })));
    let issue_id = issue["result"]["issue_id"].as_str().unwrap().to_string();
    for stage_id in ["first-half", "second-half"] {
        state.handle(req(
            "issue.stage_approve",
            json!({ "issue_id": issue_id, "stage_id": stage_id }),
        ));
    }
    state.handle(req("issue.approve", json!({ "issue_id": issue_id })));
    issue_id
}

/// Targeting (UX Architecture: "Issues can be assigned to a new or existing
/// worktree"): naming a checkout implements INTO it. The branch's own run
/// adopts the implementation — no `build/<slug>` is cut — the stage docs are
/// committed onto its current HEAD as the review baseline, and the work goes
/// to a FRESH agent, because implementation is always a handoff.
#[test]
fn issue_implements_into_an_existing_worktree_with_a_fresh_agent() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let run_id = adopted_run(&mut state, &repo, dir.path(), "feature-target");
    let primary_agent = primary_agent_id(&state, &run_id);
    let worktree_id = worktree_id_of_run(&state, &run_id);
    let worktree_path = state.runs[&run_id].worktree.path.clone();
    let issue_id = issue_ready_to_implement(&mut state, "target an existing branch");

    let implemented = state.handle(req(
        "issue.implement_all",
        json!({ "issue_id": issue_id, "worktree_id": worktree_id }),
    ));
    assert_eq!(implemented["ok"], true, "{implemented:?}");

    // One branch, one run: the checkout's own run carries the Issue now.
    assert_eq!(state.runs.len(), 1, "nothing new was cut");
    let active = &state.runs[&run_id];
    assert_eq!(
        active.run.plan_id.as_ref().map(|id| id.0.as_str()),
        Some(issue_id.as_str()),
        "the branch's run adopted the implementation"
    );
    assert_eq!(active.worktree.branch(), "feature-target");
    assert!(
        active.base_sha.is_some(),
        "the stage docs pin the review baseline"
    );

    // The implementing agent is new — the branch's existing conversation is
    // never the one handed the work.
    assert_eq!(active.agents.len(), 2, "{:?}", active.agents.agents());
    let implementing = active.agents.agents()[1].id.clone();
    assert_ne!(implementing, primary_agent);
    assert!(
        state
            .pending_agent_turns
            .iter()
            .any(|turn| turn.owner == run_id && turn.agent_id == implementing),
        "the build turn is addressed to the fresh agent"
    );

    // What the branch was carrying is below the baseline, in its own commit,
    // rather than swept into the docs commit.
    let log = std::process::Command::new("git")
        .args(["-C", worktree_path.to_str().unwrap(), "log", "--format=%s"])
        .output()
        .unwrap();
    let log = String::from_utf8(log.stdout).unwrap();
    assert!(
        log.contains("plan: target an existing branch"),
        "the docs commit is the baseline: {log}"
    );

    // The Issue's conversation says the checkout was reused, not created.
    let events: Vec<&str> = implemented["result"]["thread"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|item| item["data"]["event"].as_str())
        .collect();
    assert!(events.contains(&"worktree_reused"), "{events:?}");
    assert!(!events.contains(&"worktree_created"), "{events:?}");
    assert!(events.contains(&"implementation_started"), "{events:?}");
}

/// Two Issues writing one branch would make neither one's diff readable, so
/// a branch already implementing another Issue refuses the second.
#[test]
fn implementing_into_a_branch_already_implementing_another_issue_is_refused() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let run_id = adopted_run(&mut state, &repo, dir.path(), "feature-contended");
    let worktree_id = worktree_id_of_run(&state, &run_id);
    let first_issue = issue_ready_to_implement(&mut state, "first claim");
    let taken = state.handle(req(
        "issue.implement_all",
        json!({ "issue_id": first_issue, "worktree_id": worktree_id }),
    ));
    assert_eq!(taken["ok"], true, "{taken:?}");

    let second_issue = issue_ready_to_implement(&mut state, "second claim");
    let refused = state.handle(req(
        "issue.implement_all",
        json!({ "issue_id": second_issue, "worktree_id": worktree_id }),
    ));
    assert_eq!(refused["ok"], false, "{refused:?}");
    let message = refused["error"].as_str().unwrap();
    assert!(message.contains("feature-contended"), "{message}");
    assert!(message.contains(&first_issue), "{message}");

    // The refusal changed nothing: the branch still belongs to the first
    // Issue, and the second one started no implementation anywhere.
    assert_eq!(state.runs.len(), 1);
    assert_eq!(
        state.runs[&run_id].run.plan_id.as_ref().map(|id| &id.0),
        Some(&first_issue)
    );
    assert!(state
        .runs
        .values()
        .all(|run| run.run.plan_id.as_ref().map(|id| &id.0) != Some(&second_issue)));
}

/// The primary checkout is the repository itself. Handing an Issue to it
/// would commit the stage docs onto the branch the human is standing on, so
/// it is refused however the client asks.
#[test]
fn implementing_into_the_primary_checkout_is_refused() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.project_at(0).id.clone();
    let adopted = state.handle(req(
        "run.adopt",
        json!({ "project_id": project_id, "primary": true }),
    ));
    let run_id = run_id_of(&adopted);
    let worktree_id = worktree_id_of_run(&state, &run_id);
    let issue_id = issue_ready_to_implement(&mut state, "not on main");

    let refused = state.handle(req(
        "issue.implement_all",
        json!({ "issue_id": issue_id, "worktree_id": worktree_id }),
    ));
    assert_eq!(refused["ok"], false, "{refused:?}");
    assert!(
        refused["error"]
            .as_str()
            .unwrap()
            .contains("primary checkout"),
        "{refused:?}"
    );
    assert!(state.runs[&run_id].run.plan_id.is_none());
}

/// A checkout Build has never adopted is adoptable in the same act: naming
/// an external worktree implements into it, and the run minted for it is the
/// implementation's.
#[test]
fn implementing_into_an_unadopted_worktree_adopts_it_first() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    add_external_worktree(&repo, dir.path(), "feature-unadopted", "feature-unadopted");
    let project_id = state.project_at(0).id.clone();
    let worktree_id = state
        .scan_external_worktrees_now(&project_id)
        .unwrap()
        .into_iter()
        .find(|w| w.branch.as_deref() == Some("feature-unadopted"))
        .expect("the external worktree is discoverable")
        .id;
    let issue_id = issue_ready_to_implement(&mut state, "adopt and implement");

    let implemented = state.handle(req(
        "issue.implement_all",
        json!({ "issue_id": issue_id, "worktree_id": worktree_id }),
    ));
    assert_eq!(implemented["ok"], true, "{implemented:?}");
    assert_eq!(state.runs.len(), 1, "{:?}", state.runs.keys());
    let active = state.runs.values().next().unwrap();
    assert_eq!(active.worktree.branch(), "feature-unadopted");
    assert_eq!(
        active.run.plan_id.as_ref().map(|id| id.0.as_str()),
        Some(issue_id.as_str())
    );
    assert!(active.adopted, "the checkout was not cut by Build");
}

#[test]
fn implement_all_persists_intent_before_stage_one_approval_and_resumes() {
    let (dir, repo) = init_repo();
    let issue_id;
    {
        let mut state = qa_state(&repo, dir.path());
        let issue = state.handle(req("issue.create", json!({ "goal": "durable all" })));
        issue_id = issue["result"]["issue_id"].as_str().unwrap().to_string();
        state.handle(req("issue.approve", json!({ "issue_id": issue_id })));

        let waiting = state.handle(req("issue.implement_all", json!({ "issue_id": issue_id })));
        assert_eq!(waiting["ok"], true, "{waiting:?}");
        assert!(waiting["result"]["current_implementation"].is_null());
        assert_eq!(waiting["result"]["implementation_intent"], "all");
        assert_eq!(
            waiting["result"]["implementation_activity"],
            json!({ "waiting_approval": "first-half" })
        );
    }

    // The intent is durable even though no worktree/run existed yet.
    let mut state = qa_state(&repo, dir.path());
    let restored = state.handle(req("issue.get", json!({ "issue_id": issue_id })));
    assert_eq!(restored["result"]["implementation_intent"], "all");
    assert_eq!(
        restored["result"]["implementation_activity"],
        json!({ "waiting_approval": "first-half" })
    );

    let first = state.handle(req(
        "issue.stage_approve",
        json!({ "issue_id": issue_id, "stage_id": "first-half" }),
    ));
    assert_eq!(first["ok"], true, "{first:?}");
    let waiting = state.handle(req("issue.get", json!({ "issue_id": issue_id })));
    assert_eq!(
        waiting["result"]["current_implementation"]["state"],
        "stage_gate"
    );
    assert_eq!(
        waiting["result"]["implementation_activity"],
        json!({ "waiting_approval": "second-half" })
    );

    let second = state.handle(req(
        "issue.stage_approve",
        json!({ "issue_id": issue_id, "stage_id": "second-half" }),
    ));
    assert_eq!(second["ok"], true, "{second:?}");
    let completed = state.handle(req("issue.get", json!({ "issue_id": issue_id })));
    assert_eq!(
        completed["result"]["current_implementation"]["state"],
        "review"
    );
    assert_eq!(completed["result"]["implementation_intent"], "none");
    assert_eq!(completed["result"]["implementation_activity"], "idle");
}

#[test]
fn implement_stage_recreates_the_original_missing_issue_worktree() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let issue = state.handle(req("issue.create", json!({ "goal": "reuse branch" })));
    let issue_id = issue["result"]["issue_id"].as_str().unwrap().to_string();
    state.handle(req(
        "issue.stage_approve",
        json!({ "issue_id": issue_id, "stage_id": "first-half" }),
    ));
    state.handle(req("issue.approve", json!({ "issue_id": issue_id })));
    let run = state.handle(req("run.create", json!({ "plan_id": issue_id })));
    let run_id = run_id_of(&run);
    assert_eq!(run["result"]["state"], "stage_gate", "{run:?}");
    state.handle(req(
        "issue.stage_approve",
        json!({ "issue_id": issue_id, "stage_id": "second-half" }),
    ));
    let worktree = state.runs[&run_id].worktree.path.clone();
    assert!(Command::new("git")
        .args([
            "worktree",
            "remove",
            "--force",
            "--",
            worktree.to_str().unwrap()
        ])
        .current_dir(&repo)
        .status()
        .unwrap()
        .success());

    let implemented = state.handle(req(
        "issue.implement_stage",
        json!({ "issue_id": issue_id, "stage_id": "second-half" }),
    ));
    assert_eq!(implemented["ok"], true, "{implemented:?}");
    assert!(worktree.exists());
    assert_eq!(
        implemented["result"]["current_implementation"]["state"],
        "review"
    );
    assert!(implemented["result"]["thread"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["type"] == "event" && item["data"]["event"] == "worktree_recreated"));
}

#[test]
#[allow(clippy::cognitive_complexity)] // ratchet: missing_original_branch_starts_nonce_bound_recovery_instead_of_archiving is at 19, threshold 15 — bring it under, then remove
fn missing_original_branch_starts_nonce_bound_recovery_instead_of_archiving() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let issue = state.handle(req(
        "issue.create",
        json!({ "goal": "recover exact lineage" }),
    ));
    let issue_id = issue["result"]["issue_id"].as_str().unwrap().to_string();
    for stage_id in ["first-half", "second-half"] {
        state.handle(req(
            "issue.stage_approve",
            json!({ "issue_id": issue_id, "stage_id": stage_id }),
        ));
    }
    state.handle(req("issue.approve", json!({ "issue_id": issue_id })));
    let run = state.handle(req("run.create", json!({ "plan_id": issue_id })));
    let run_id = run_id_of(&run);
    let branch = state.runs[&run_id].worktree.branch();
    let worktree = state.runs[&run_id].worktree.path.clone();
    assert!(Command::new("git")
        .args([
            "worktree",
            "remove",
            "--force",
            "--",
            worktree.to_str().unwrap()
        ])
        .current_dir(&repo)
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["branch", "-D", "--", &branch])
        .current_dir(&repo)
        .status()
        .unwrap()
        .success());

    let requested = state.handle(req(
        "issue.implement_stage",
        json!({ "issue_id": issue_id, "stage_id": "second-half" }),
    ));
    assert_eq!(requested["ok"], false, "{requested:?}");
    assert!(requested["error"]
        .as_str()
        .unwrap()
        .contains("verified recovery"));
    let issue_view = state.handle(req("issue.get", json!({ "issue_id": issue_id })));
    let active = &state.runs[&run_id];
    assert_ne!(active.run.state, RunState::Archived);
    let recovery = active.recovery.as_ref().expect("durable recovery attempt");
    assert_eq!(recovery.state, crate::run::RecoveryState::Started);
    assert!(recovery.id.starts_with("recovery-"));
    assert_eq!(recovery.requested_stage_id, "second-half");
    let recovery_turn = state
        .pending_agent_turns
        .iter()
        .find(|turn| turn.owner == run_id && turn.phase == "recover")
        .expect("recovery turn is queued");
    assert!(
        recovery_turn.said().cold.contains(&recovery.id),
        "{}",
        recovery_turn.said().cold
    );
    assert!(
        recovery_turn
            .said()
            .cold
            .contains("Build conversation protocol"),
        "{}",
        recovery_turn.said().cold
    );
    assert!(recovery_turn.said().cold.contains("read_unread_messages"));
    assert!(recovery_turn
        .said()
        .cold
        .contains("Ordered Issue stage-plan catalog"));
    let catalog = recovery_turn
        .said()
        .cold
        .split("Ordered Issue stage-plan catalog (authoritative order):")
        .nth(1)
        .unwrap();
    assert!(catalog.find("first-half") < catalog.find("second-half"));
    assert!(recovery_turn.said().warm.contains("read_unread_messages"));
    // A warm recovery is a live process that lived this conversation, and
    // the protocol block it keeps tells it to read what it missed — so the
    // packet is the cold half's alone, and is composed at delivery.
    assert!(
        !recovery_turn.said().warm.contains("Catch-up packet"),
        "{}",
        recovery_turn.said().warm
    );
    assert!(
        !recovery_turn.said().cold.contains("Catch-up packet"),
        "{}",
        recovery_turn.said().cold
    );
    assert!(issue_view["result"]["thread"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["data"]["event"] == "recovery_started"
            && item["data"]["links"]
                .as_array()
                .unwrap()
                .iter()
                .any(|link| link["kind"] == "recovery")));
}

#[test]
fn recovery_report_nonce_mismatch_fails_and_invalidates_only_the_predecessor() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let issue = state.handle(req(
        "issue.create",
        json!({ "goal": "reject forged recovery" }),
    ));
    let issue_id = issue["result"]["issue_id"].as_str().unwrap().to_string();
    for stage_id in ["first-half", "second-half"] {
        state.handle(req(
            "issue.stage_approve",
            json!({ "issue_id": issue_id, "stage_id": stage_id }),
        ));
    }
    state.handle(req("issue.approve", json!({ "issue_id": issue_id })));
    let run = state.handle(req("run.create", json!({ "plan_id": issue_id })));
    let run_id = run_id_of(&run);
    let branch = state.runs[&run_id].worktree.branch();
    let worktree = state.runs[&run_id].worktree.path.clone();
    Command::new("git")
        .args([
            "worktree",
            "remove",
            "--force",
            "--",
            worktree.to_str().unwrap(),
        ])
        .current_dir(&repo)
        .status()
        .unwrap();
    Command::new("git")
        .args(["branch", "-D", "--", &branch])
        .current_dir(&repo)
        .status()
        .unwrap();
    state.handle(req(
        "issue.implement_stage",
        json!({ "issue_id": issue_id, "stage_id": "second-half" }),
    ));

    state.on_agent_done(
        &run_id,
        DoneReport {
            phase: DonePhase::Recover,
            status: DoneStatus::Completed,
            summary: "forged".into(),
            outputs: DoneOutputs {
                recovery: Some(crate::mcp::RecoveryReport {
                    recovery_id: "recovery-wrong".into(),
                    recovered: true,
                    branch,
                    head_sha: "0".repeat(40),
                    findings: "claim".into(),
                }),
                ..DoneOutputs::default()
            },
        },
    );
    let active = &state.runs[&run_id];
    assert_eq!(
        active.recovery.as_ref().unwrap().state,
        crate::run::RecoveryState::Failed
    );
    assert!(active.stages[0].invalidation_reason.is_some());
    assert!(active
        .stages
        .iter()
        .skip(1)
        .all(|stage| stage.invalidation_reason.is_none()));
    assert_ne!(active.run.state, RunState::Archived);
    let issue = state.handle(req("issue.get", json!({ "issue_id": issue_id })));
    assert!(issue["result"]["thread"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["data"]["event"] == "recovery_failed"));
    assert_eq!(
        issue["result"]["implementation_activity"]["blocked"]["stage_id"], "second-half",
        "a failed verified recovery must visibly block the requested next stage: {issue:?}"
    );
    assert!(
        issue["result"]["implementation_activity"]["blocked"]["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("verified recovery failed"))
    );
}

#[test]
fn restart_with_pending_verified_recovery_boots_and_requeues_the_same_primed_attempt() {
    let (dir, repo) = init_repo();
    let issue_id;
    let run_id;
    let recovery_id;
    {
        let mut state = qa_state(&repo, dir.path());
        let issue = state.handle(req(
            "issue.create",
            json!({ "goal": "restart pending recovery" }),
        ));
        issue_id = issue["result"]["issue_id"].as_str().unwrap().to_string();
        for stage_id in ["first-half", "second-half"] {
            state.handle(req(
                "issue.stage_approve",
                json!({ "issue_id": issue_id, "stage_id": stage_id }),
            ));
        }
        state.handle(req("issue.approve", json!({ "issue_id": issue_id })));
        let run = state.handle(req("run.create", json!({ "plan_id": issue_id })));
        run_id = run_id_of(&run);
        let branch = state.runs[&run_id].worktree.branch();
        let worktree = state.runs[&run_id].worktree.path.clone();
        git_in(
            &repo,
            &[
                "worktree",
                "remove",
                "--force",
                "--",
                worktree.to_str().unwrap(),
            ],
        );
        git_in(&repo, &["branch", "-D", "--", &branch]);
        let requested = state.handle(req(
            "issue.implement_stage",
            json!({ "issue_id": issue_id, "stage_id": "second-half" }),
        ));
        assert_eq!(requested["ok"], false, "{requested:?}");
        recovery_id = state.runs[&run_id].recovery.as_ref().unwrap().id.clone();
    }

    let context =
        HarnessContext::resolved(dir.path().join("test-mcp.sock"), dir.path().to_path_buf())
            .unwrap();
    let state = AppState::new_configured(repo, dir.path().join("wt"), "main", true, context)
        .with_task_store(dir.path().join("store"))
        .expect("a pending recovery must not abort daemon startup");
    let recovery = state.runs[&run_id]
        .recovery
        .as_ref()
        .expect("the durable attempt survives restart");
    assert_eq!(
        recovery.id, recovery_id,
        "restart must not mint a new nonce"
    );
    assert_eq!(recovery.state, crate::run::RecoveryState::Started);
    let turn = state
        .pending_agent_turns
        .iter()
        .find(|turn| turn.owner == run_id && turn.phase == "recover")
        .expect("restart requeues the recovery agent");
    assert!(
        turn.said().cold.contains("Build conversation protocol"),
        "{}",
        turn.said().cold
    );
    assert!(
        turn.said().cold.contains("read_unread_messages"),
        "{}",
        turn.said().cold
    );
    assert!(
        turn.said()
            .cold
            .contains("Ordered Issue stage-plan catalog"),
        "{}",
        turn.said().cold
    );
    let catalog = turn
        .said()
        .cold
        .split("Ordered Issue stage-plan catalog (authoritative order):")
        .nth(1)
        .unwrap();
    assert!(catalog.find("first-half") < catalog.find("second-half"));
    assert_eq!(
        state.plans[&issue_id].plan.implementation_activity,
        ImplementationActivity::Blocked {
            stage_id: "second-half".into(),
            reason: state.runs[&run_id].last_error.clone().unwrap(),
        }
    );
}

#[test]
fn matching_recovery_report_is_independently_verified_and_resumes_requested_stage() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let issue = state.handle(req(
        "issue.create",
        json!({ "goal": "verify recovered head" }),
    ));
    let issue_id = issue["result"]["issue_id"].as_str().unwrap().to_string();
    for stage_id in ["first-half", "second-half"] {
        state.handle(req(
            "issue.stage_approve",
            json!({ "issue_id": issue_id, "stage_id": stage_id }),
        ));
    }
    state.handle(req("issue.approve", json!({ "issue_id": issue_id })));
    let run = state.handle(req("run.create", json!({ "plan_id": issue_id })));
    let run_id = run_id_of(&run);
    let branch = state.runs[&run_id].worktree.branch();
    let worktree = state.runs[&run_id].worktree.path.clone();
    let head_sha = String::from_utf8(
        Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&worktree)
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    Command::new("git")
        .args([
            "worktree",
            "remove",
            "--force",
            "--",
            worktree.to_str().unwrap(),
        ])
        .current_dir(&repo)
        .status()
        .unwrap();
    Command::new("git")
        .args(["branch", "-D", "--", &branch])
        .current_dir(&repo)
        .status()
        .unwrap();
    state.handle(req(
        "issue.implement_stage",
        json!({ "issue_id": issue_id, "stage_id": "second-half" }),
    ));
    let recovery_id = state.runs[&run_id].recovery.as_ref().unwrap().id.clone();
    assert!(Command::new("git")
        .args(["branch", &branch, &head_sha])
        .current_dir(&repo)
        .status()
        .unwrap()
        .success());

    state.on_agent_done(
        &run_id,
        DoneReport {
            phase: DonePhase::Recover,
            status: DoneStatus::Completed,
            summary: "exact branch recovered".into(),
            outputs: DoneOutputs {
                recovery: Some(crate::mcp::RecoveryReport {
                    recovery_id,
                    recovered: true,
                    branch,
                    head_sha,
                    findings: "local reflog proved the exact tip".into(),
                }),
                ..DoneOutputs::default()
            },
        },
    );
    let active = &state.runs[&run_id];
    assert_eq!(
        active.recovery.as_ref().unwrap().state,
        crate::run::RecoveryState::Succeeded
    );
    assert!(active.worktree.path.exists());
    assert_eq!(active.run.state, RunState::Review);
    let issue = state.handle(req("issue.get", json!({ "issue_id": issue_id })));
    assert!(issue["result"]["thread"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["data"]["event"] == "recovery_succeeded"));
}

#[test]
fn implement_all_resumes_when_the_waiting_stage_plan_is_approved() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let issue = state.handle(req("issue.create", json!({ "goal": "resume all" })));
    let issue_id = issue["result"]["issue_id"].as_str().unwrap().to_string();
    state.handle(req(
        "issue.stage_approve",
        json!({ "issue_id": issue_id, "stage_id": "first-half" }),
    ));
    state.handle(req("issue.approve", json!({ "issue_id": issue_id })));
    let run = state.handle(req("run.create", json!({ "plan_id": issue_id })));
    let run_id = run_id_of(&run);
    assert_eq!(run["result"]["state"], "stage_gate", "{run:?}");

    let waiting = state.handle(req(
        "run.set_auto_advance",
        json!({ "run_id": run_id, "enabled": true }),
    ));
    assert_eq!(waiting["result"]["state"], "stage_gate", "{waiting:?}");
    assert_eq!(waiting["result"]["auto_advance"], true);

    let approved = state.handle(req(
        "issue.stage_approve",
        json!({ "issue_id": issue_id, "stage_id": "second-half" }),
    ));
    assert_eq!(approved["ok"], true, "{approved:?}");
    let implementation = state.handle(req("run.get", json!({ "run_id": run_id })));
    assert_eq!(
        implementation["result"]["state"], "review",
        "{implementation:?}"
    );
    assert!(implementation["result"]["stages"]
        .as_array()
        .unwrap()
        .iter()
        .all(|stage| stage["state"] == "validated_passed"));
}

#[test]
fn board_list_carries_plans_runs_and_ride_alongs() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    planned_run_in_review(&mut state, "a plan");
    let board = state.handle(req("board.list", json!({})));
    let r = &board["result"];
    assert_eq!(r["plans"].as_array().unwrap().len(), 1);
    assert_eq!(r["runs"].as_array().unwrap().len(), 1);
    assert!(r["external_worktrees"].is_array());
    assert!(r["primary_changes"].is_array());
}

#[test]
fn board_list_views_carry_state_changed_at_and_run_worktree_path() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (_, run_id) = planned_run_in_review(&mut state, "progress facts");
    state.handle(req("plan.create", json!({ "goal": "a plan" })));

    let board = state.handle(req("board.list", json!({})));
    let run_entry = board["result"]["runs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["run_id"] == json!(run_id.clone()))
        .unwrap()
        .clone();
    assert!(
        run_entry["state_changed_at"]
            .as_str()
            .is_some_and(|s| !s.is_empty()),
        "{run_entry:?}"
    );
    let worktree_path = run_entry["worktree_path"].as_str().unwrap();
    assert!(
        std::path::Path::new(worktree_path).exists(),
        "{run_entry:?}"
    );

    let plan_entry = &board["result"]["plans"].as_array().unwrap()[0];
    assert!(
        plan_entry["state_changed_at"]
            .as_str()
            .is_some_and(|s| !s.is_empty()),
        "{plan_entry:?}"
    );
}

use super::*;

#[test]
fn completed_plan_archives_idempotently_and_moves_off_the_board() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (plan_id, run_id) = planned_run_in_review(&mut state, "archive completed plan");
    let project_id = state.projects[0].id.clone();

    let before = state.handle(req("plan.get", json!({ "plan_id": plan_id })));
    assert_eq!(
        before["result"]["implementation_complete"], true,
        "{before:?}"
    );
    assert_eq!(before["result"]["can_archive"], true, "{before:?}");
    assert!(before["result"]["archived_at"].is_null());

    let archived = state.handle(req("plan.archive", json!({ "plan_id": plan_id })));
    assert_eq!(archived["ok"], true, "{archived:?}");
    let archived_at = archived["result"]["archived_at"]
        .as_str()
        .expect("archive timestamp")
        .to_string();
    assert_eq!(archived["result"]["can_archive"], false);

    let repeated = state.handle(req("plan.archive", json!({ "plan_id": plan_id })));
    assert_eq!(repeated["result"]["archived_at"], archived_at);
    let board = state.handle(req("board.list", json!({})));
    assert!(board["result"]["plans"]
        .as_array()
        .unwrap()
        .iter()
        .all(|plan| plan["plan_id"] != plan_id));
    assert!(board["result"]["runs"]
        .as_array()
        .unwrap()
        .iter()
        .any(|run| run["run_id"] == run_id));

    let listed = state.handle(req("plan.list", json!({})));
    assert!(listed["result"]["plans"]
        .as_array()
        .unwrap()
        .iter()
        .any(|plan| plan["plan_id"] == plan_id));
    let archive = state.handle(req("archive.list", json!({ "project_id": project_id })));
    assert_eq!(archive["result"]["plans"].as_array().unwrap().len(), 1);
    assert!(archive["result"]["worktrees"]
        .as_array()
        .unwrap()
        .is_empty());
}

/// Done on an issue files it away whatever was built for it: the bridge
/// warns that nothing was, and then does as it is told.
#[test]
fn plan_archive_files_an_unimplemented_plan_away_with_a_warning() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let incomplete = state.handle(req("plan.create", json!({ "goal": "not implemented" })));
    let incomplete_id = plan_id_of(&incomplete);
    assert_eq!(
        warning_codes(&incomplete["result"]),
        vec!["unimplemented"],
        "{incomplete:?}"
    );
    assert_eq!(incomplete["result"]["can_archive"], true, "{incomplete:?}");
    let archived = state.handle(req("plan.archive", json!({ "plan_id": incomplete_id })));
    assert_eq!(archived["ok"], true, "{archived:?}");
    assert!(
        archived["result"]["archived_at"].is_string(),
        "{archived:?}"
    );
    assert_eq!(
        archived["result"]["implementation_complete"], false,
        "archiving it did not make it implemented: {archived:?}"
    );

    let (legacy_plan_id, legacy_run_id) = planned_run_in_review(&mut state, "legacy completion");
    state.plans.get_mut(&legacy_plan_id).unwrap().stages.clear();
    state.runs.get_mut(&legacy_run_id).unwrap().stages.clear();
    let legacy = state.handle(req("plan.get", json!({ "plan_id": legacy_plan_id })));
    assert_eq!(
        legacy["result"]["implementation_complete"], true,
        "{legacy:?}"
    );
}

#[test]
fn multi_stage_completion_requires_every_plan_stage_to_have_passed_in_one_run() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (plan_id, run_id) = planned_run_in_review(&mut state, "all stages");
    state.runs.get_mut(&run_id).unwrap().stages.pop();

    let incomplete = state.handle(req("plan.get", json!({ "plan_id": plan_id })));
    assert_eq!(incomplete["result"]["implementation_complete"], false);
    assert_eq!(
        incomplete["result"]["can_archive"], true,
        "incomplete is a fact about the work, not a bar on filing it away"
    );
    assert!(
        warning_codes(&incomplete["result"]).is_empty(),
        "something was built for it, whatever state that work is in: {incomplete:?}"
    );
}

#[test]
fn legacy_validated_stages_without_pinned_boundaries_preserve_completion() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (issue_id, run_id) = planned_run_in_review(&mut state, "legacy completion");
    for stage in &mut state.runs.get_mut(&run_id).unwrap().stages {
        stage.completion_sha = None;
        stage.publication = StagePublication::LegacyUnknown;
    }
    let issue = state.handle(req("issue.get", json!({ "issue_id": issue_id })));
    assert_eq!(
        issue["result"]["implementation_complete"], true,
        "{issue:?}"
    );
    let stages = state.handle(req("issue.stages", json!({ "issue_id": issue_id })));
    assert!(stages["result"]["stages"]
        .as_array()
        .unwrap()
        .iter()
        .all(|stage| stage["execution"] == "legacy_unpinned"));
}

#[test]
fn archived_local_only_run_no_longer_counts_as_issue_completion() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (plan_id, run_id) = planned_run_in_review(&mut state, "completed then removed");
    let worktree = state.runs[&run_id].worktree.path.clone();
    std::fs::remove_dir_all(worktree).unwrap();

    let board = state.handle(req("board.list", json!({})));
    assert!(board["result"]["runs"]
        .as_array()
        .unwrap()
        .iter()
        .all(|run| run["run_id"] != run_id));
    assert_eq!(state.runs[&run_id].run.state, RunState::Archived);
    let plan = state.handle(req("plan.get", json!({ "plan_id": plan_id })));
    assert_eq!(plan["result"]["implementation_complete"], false, "{plan:?}");
    assert_eq!(plan["result"]["can_archive"], true, "{plan:?}");
}

#[test]
fn archived_plan_metadata_survives_restart_with_docs_and_runs() {
    let (dir, repo) = init_repo();
    let plan_id;
    let run_id;
    let canonical_doc;
    {
        let mut state = qa_state(&repo, dir.path());
        (plan_id, run_id) = planned_run_in_review(&mut state, "durable archive");
        let before = state.handle(req(
            "plan.stage_doc",
            json!({ "plan_id": plan_id, "stage_id": "first-half" }),
        ));
        canonical_doc = before["result"]["contents"]
            .as_str()
            .expect("canonical stage doc")
            .to_string();
        let archived = state.handle(req("plan.archive", json!({ "plan_id": plan_id })));
        assert_eq!(archived["ok"], true, "{archived:?}");
        let repeated = state.handle(req("plan.archive", json!({ "plan_id": plan_id })));
        assert_eq!(
            repeated["result"]["archived_at"],
            archived["result"]["archived_at"]
        );
        let after = state.handle(req(
            "plan.stage_doc",
            json!({ "plan_id": plan_id, "stage_id": "first-half" }),
        ));
        assert_eq!(after["result"]["contents"], canonical_doc);
    }

    let mut reloaded = qa_state(&repo, dir.path());
    let plan = reloaded.handle(req("plan.get", json!({ "plan_id": plan_id })));
    assert!(plan["result"]["archived_at"].is_string(), "{plan:?}");
    assert_eq!(plan["result"]["implementation_complete"], true);
    assert_eq!(
        reloaded.handle(req("run.get", json!({ "run_id": run_id })))["ok"],
        true
    );
    let doc = reloaded.handle(req(
        "plan.stage_doc",
        json!({ "plan_id": plan_id, "stage_id": "first-half" }),
    ));
    assert_eq!(doc["ok"], true, "{doc:?}");
    assert_eq!(doc["result"]["contents"], canonical_doc);
}

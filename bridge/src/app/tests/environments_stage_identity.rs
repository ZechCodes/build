//! Stage object-database fallback resolves identity before choosing a directory.
use super::*;
use crate::app::projects::test_support::register_sibling;
use crate::orchestrator::ActiveRun;

fn stage_owner(app: &mut AppState, repo: &Path, uuid_keyed: bool) -> (String, String) {
    let migrated = app.default_project().unwrap();
    let project = if uuid_keyed { register_sibling(app, &migrated) } else { migrated };
    let key = app.project(&project).unwrap().project_key.clone();
    let sha = git2::Repository::open(repo).unwrap().head().unwrap().target().unwrap().to_string();
    let mut stage = crate::run::StageProgress::dispatched("stage-one");
    stage.start_sha = Some(sha.clone());
    stage.completion_sha = Some(sha);
    let mut active = ActiveRun::workspace_conversation(
        crate::run::RunId::new("stage-owner"), "stage fallback".into(), repo.to_path_buf(), ModelChoice::default());
    active.run.plan_id = Some(crate::plan::PlanId::new("stage-plan"));
    active.stages = vec![stage];
    app.runs.insert("stage-owner".into(), active);
    app.projects.bind_entity("stage-owner".into(), project.clone());
    app.projects.retain_entity_path("stage-owner".into(), key);
    (project, "stage-owner".into())
}

#[test]
fn stage_fallback_never_opens_an_unresolved_project_key() {
    for uuid_keyed in [false, true] {
        let (dir, repo) = init_repo();
        let mut app = qa_state_before_admission(&repo, dir.path());
        let (_, owner) = stage_owner(&mut app, &repo, uuid_keyed);
        app.runs.get_mut(&owner).unwrap().worktree.path = dir.path().join("gone-checkout");
        app.projects.unbind_live_entity(&owner);
        app.clear_projects_for_test();
        let params = json!({"run_id":owner,"stage_id":"stage-one"});
        assert!(app.plan_run_stage_diff(&params, None).is_err(),
            "an unresolved key cannot select a fallback object database");
    }
}

#[test]
fn snapshot_stage_fallback_never_opens_an_unresolved_project_key() {
    for uuid_keyed in [false, true] {
        let (dir, repo) = init_repo();
        let mut app = qa_state_before_admission(&repo, dir.path());
        let (_, owner) = stage_owner(&mut app, &repo, uuid_keyed);
        app.runs.get_mut(&owner).unwrap().worktree.path = dir.path().join("gone-checkout");
        app.projects.unbind_live_entity(&owner);
        app.clear_projects_for_test();
        let context = crate::app::observation_reads::scopes::ScopeReadContext::capture(&app);
        let result = context.prepare("task.stage_diff", &json!({"task_id":"stage-plan","stage_id":"stage-one"}))
            .and_then(|job| job.run());
        let error = result.unwrap_err();
        assert!(error.message().contains("project repository is unavailable"), "{}", error.message());
    }
}

#[test]
fn stage_reads_keep_existing_checkouts_and_resolved_project_homes() {
    for uuid_keyed in [false, true] {
        let (dir, repo) = init_repo();
        let mut app = qa_state_before_admission(&repo, dir.path());
        let (_, owner) = stage_owner(&mut app, &repo, uuid_keyed);
        app.projects.unbind_live_entity(&owner);
        let params = json!({"run_id":owner,"stage_id":"stage-one"});
        app.runs.get_mut(&owner).unwrap().worktree.path = dir.path().join("gone-checkout");
        assert!(app.plan_run_stage_diff(&params, None).is_ok(), "a resolvable retained key uses its project's home");
        app.take_deferred();
        let context = crate::app::observation_reads::scopes::ScopeReadContext::capture(&app);
        let answer = context.prepare("task.stage_diff", &json!({"task_id":"stage-plan","stage_id":"stage-one"})).unwrap().run().unwrap();
        assert_eq!(answer["status"], "available");
        app.runs.get_mut(&owner).unwrap().worktree.path = repo;
        app.clear_projects_for_test();
        assert!(app.plan_run_stage_diff(&params, None).is_ok(), "a surviving recorded checkout remains readable");
        let context = crate::app::observation_reads::scopes::ScopeReadContext::capture(&app);
        let answer = context.prepare("task.stage_diff", &json!({"task_id":"stage-plan","stage_id":"stage-one"})).unwrap().run().unwrap();
        assert_eq!(answer["status"], "available");
    }
}

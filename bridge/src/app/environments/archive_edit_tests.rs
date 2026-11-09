//! M3.2 project work is guarded; environment reference maintenance stays allowed.
use super::edit_tests::{fixture, ok};
use super::runtime_tests::rpc;
use crate::api::ApiError;
use crate::app::runtime::deferred::{DeferredJob, DeferredNext};
use crate::app::{AppState, Frame};
use serde_json::{json, Value};

fn request(method: &str, params: Value) -> Frame {
    Frame {
        session_id: "archive-edits".into(),
        message_id: "1".into(),
        frame_type: "data".into(),
        sender: "client".into(),
        created_at: "test".into(),
        payload: json!({"id":"1", "method":method, "params":params}),
    }
}

#[test]
fn archived_project_rename_is_refused_on_every_path() {
    let root = tempfile::tempdir().unwrap();
    let mut app = fixture(root.path());
    ok(&mut app, "project.archive", json!({"project_id":"proj-1"}));
    let before = std::fs::read(root.path().join("config.json")).unwrap();
    let params = json!({"project_id":"proj-1","name":"Must not change"});
    assert_eq!(
        rpc(&mut app, "project.rename", params.clone())["error_code"],
        "project_archived"
    );
    assert_eq!(
        app.dispatch_api("project.rename", &params)
            .unwrap_err()
            .code(),
        "project_archived"
    );
    // The typed entry and narrow mutation entry must refuse before reserving
    // any deferred work, as well as when reached through routed admission.
    assert_eq!(
        crate::api::v1::dispatch(&mut app, "project.rename", &params)
            .unwrap()
            .unwrap_err()
            .code(),
        "project_archived"
    );
    let error = app.project_rename(&params).unwrap_err();
    assert_eq!(ApiError::classify(error).code(), "project_archived");
    assert!(app.deferred_work.is_none());
    assert_eq!(app.projects.get("proj-1").unwrap().name, "First");
    assert_eq!(
        std::fs::read(root.path().join("config.json")).unwrap(),
        before
    );
    let state = app.shared();
    let handler = AppState::handler(state);
    let reply = handler.call(
        crate::carrier::SessionSender::detached("archive-edits"),
        request("project.rename", params),
    );
    assert_eq!(reply["error_code"], "project_archived", "{reply}");
}

#[test]
fn environment_reference_maintenance_is_allowed_for_archived_projects() {
    let root = tempfile::tempdir().unwrap();
    let mut app = fixture(root.path());
    for id in ["proj-1", "proj-2"] {
        ok(&mut app, "project.archive", json!({"project_id":id}));
    }
    let archived = app
        .projects
        .iter()
        .map(|p| (p.id.clone(), p.archived_at))
        .collect::<Vec<_>>();
    let repo = crate::git_fixture::init_repo_named(root.path(), "added");
    let row = ok(
        &mut app,
        "environment.add_source",
        json!({"environment_id":"env-1","path":repo}),
    );
    let source = row["sources"].as_array().unwrap().last().unwrap()["id"].clone();
    for (method, params) in [
        (
            "environment.update_source",
            json!({"environment_id":"env-1","source_id":source,"name":"Reference source","sync_base":false}),
        ),
        (
            "environment.rename",
            json!({"environment_id":"env-1","name":"Reference label"}),
        ),
        (
            "environment.sync_source",
            json!({"environment_id":"env-1","source_id":source,"source_path":repo}),
        ),
        (
            "environment.set_isolation",
            json!({"environment_id":"env-1","isolation":"worktree"}),
        ),
    ] {
        ok(&mut app, method, params);
        assert_eq!(
            app.projects
                .iter()
                .map(|p| (p.id.clone(), p.archived_at))
                .collect::<Vec<_>>(),
            archived
        );
        for project in app.projects.iter() {
            assert!(project
                .sources
                .iter()
                .any(|s| json!(s.id) == source && s.name == "Reference source"));
        }
        assert!(app.workspaces.list(None).is_empty());
    }
    ok(
        &mut app,
        "environment.remove_source",
        json!({"environment_id":"env-1","source_id":source}),
    );
    assert!(app
        .projects
        .iter()
        .all(|p| p.archived_at.is_some() && !p.sources.iter().any(|s| json!(s.id) == source)));
}

fn next(app: &mut AppState, params: &Value, job: DeferredJob) -> DeferredJob {
    match app.apply_deferred_stage("environment.add_source", params, job.run()) {
        DeferredNext::Again(job) => job,
        DeferredNext::Answered(answer) => panic!("expected another off-lock stage: {answer:?}"),
    }
}

#[test]
fn environment_source_add_preserves_archive_landing_during_each_off_lock_stage() {
    for stage in 0..3 {
        let root = tempfile::tempdir().unwrap();
        let mut app = fixture(root.path());
        let origin = crate::git_fixture::init_repo_named(root.path(), "origin");
        let params = json!({"environment_id":"env-1","remote":origin});
        let (answer, job) = app.dispatch_deferring("environment.add_source", &params);
        assert!(answer.is_ok());
        let mut job = job.unwrap();
        for _ in 0..stage {
            job = next(&mut app, &params, job);
        }
        let done = job.run();
        // Normal archive admission waits for filesystem jobs. Model an archive
        // already landing from another producer while this result is in flight.
        app.projects.at_mut(0).archived_at = Some(7);
        app.persist();
        let error = app
            .apply_deferred("environment.add_source", &params, done)
            .unwrap_err();
        assert!(
            error.contains("stale unversioned mutation"),
            "{stage}: {error}"
        );
        assert_ne!(ApiError::classify(error).code(), "project_archived");
        assert_eq!(app.projects.get("proj-1").unwrap().archived_at, Some(7));
        assert_eq!(app.projects.environment("env-1").unwrap().sources.len(), 2);
        // Reference maintenance is permitted on retry against the new inputs.
        ok(&mut app, "environment.add_source", params);
        assert_eq!(app.projects.get("proj-1").unwrap().archived_at, Some(7));
        assert_eq!(app.projects.get("proj-1").unwrap().sources.len(), 3);
        assert!(app.workspaces.list(None).is_empty());
    }
}

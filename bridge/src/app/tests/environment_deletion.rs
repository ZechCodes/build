//! Stage 2 deletion keeps sibling projects isolated, including shared homes.
use super::project_agent::workspace;
use super::tracker::{filed, tracked};
use super::*;

fn call(app: &mut AppState, method: &str, params: Value) -> Value {
    let reply = app.handle(req(method, params));
    assert_eq!(reply["ok"], true, "{reply}");
    reply["result"].clone()
}

#[test]
fn environment_delete_refused_while_projects_exist() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(root.path());
    let sibling = app.add_environment_sibling_for_test(&project);
    let active = app.handle(req("environment.delete", json!({"environment_id":"env-1"})));
    assert_eq!(active["error"], "Delete its 2 projects first");
    for count in [2, 1] {
        for index in 0..app.projects.iter().count() {
            app.projects.at_mut(index).archived_at = Some(1);
        }
        let refused = app.handle(req("environment.delete", json!({"environment_id":"env-1"})));
        assert_eq!(refused["error_code"], "conflict", "{refused}");
        assert_eq!(
            refused["error"],
            format!("Delete its {count} projects first")
        );
        if count == 2 {
            call(
                &mut app,
                "project.delete",
                json!({"project_id":sibling,"confirm":true,"tasks":"delete"}),
            );
        }
    }
    call(
        &mut app,
        "project.delete",
        json!({"project_id":project,"confirm":true,"tasks":"delete"}),
    );
    assert_eq!(
        call(
            &mut app,
            "environment.delete",
            json!({"environment_id":"env-1"})
        ),
        json!({})
    );
    assert!(app.projects.environment("env-1").is_none());
}

#[test]
fn project_delete_removes_its_tasks_only() {
    for delete_sibling in [false, true] {
        check_project_task_deletion(delete_sibling);
    }
}

fn check_project_task_deletion(delete_sibling: bool) {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(root.path());
    let sibling = app.add_environment_sibling_for_test(&project);
    let (project, sibling) = if delete_sibling {
        (sibling, project)
    } else {
        (project, sibling)
    };
    let first = filed(&mut app, &project, "Delete task");
    let second = filed(&mut app, &sibling, "Keep task");
    let waiting = call(
        &mut app,
        "tasks.create",
        json!({"project_id":sibling,"title":"Keep intent",
        "waiting_on":[second["id"]],"assignee":{"kind":"new_project_agent","agent_name":"Waiting"}}),
    );
    let deleted_waiting = call(
        &mut app,
        "tasks.create",
        json!({"project_id":project,"title":"Delete intent",
        "waiting_on":[first["id"]],"assignee":{"kind":"new_project_agent","agent_name":"Waiting"}}),
    );
    let sibling_checkout = workspace(&mut app, &sibling, "Keep checkout");
    let kept_review = call(
        &mut app,
        "tasks.review.open",
        json!({"workspace_id":sibling_checkout,
        "request_id":"keep-review","title":"Keep review","description":"Review"}),
    );
    let kept_review_id = kept_review["task"]["id"].as_str().unwrap();
    let kept_review_before = serde_json::to_value(
        app.store
            .as_ref()
            .unwrap()
            .load_review(kept_review_id)
            .unwrap(),
    )
    .unwrap();
    let checkout = workspace(&mut app, &project, "Review checkout");
    let review = call(
        &mut app,
        "tasks.review.open",
        json!({"workspace_id":checkout,
        "request_id":"delete-review","title":"Delete review","description":"Review"}),
    );
    let refused = app.handle(req(
        "project.delete",
        json!({"project_id":project,"confirm":true,"tasks":"delete"}),
    ));
    assert_eq!(refused["error_code"], "conflict", "{refused}");
    assert!(app
        .store
        .as_ref()
        .unwrap()
        .load_tracker_task(first["id"].as_str().unwrap())
        .unwrap()
        .is_some());
    // Finished manifests are the deletion admission boundary; no provider runs here.
    let mut finished = app.workspaces.get(&checkout).unwrap().clone();
    finished.status = crate::workspace::WorkspaceStatus::Finished;
    crate::workspace::persist_review_workspace(&finished).unwrap();
    app.workspaces.reload().unwrap();
    call(
        &mut app,
        "project.delete",
        json!({"project_id":project,"confirm":true,"tasks":"delete"}),
    );
    let store = app.store.as_ref().unwrap();
    for task in [&first, &deleted_waiting["task"], &review["task"]] {
        let id = task["id"].as_str().unwrap();
        assert!(store.load_tracker_task(id).unwrap().is_none());
        assert!(store.load_review(id).unwrap().is_none());
        assert!(store.current_delivery_intent(id).unwrap().is_none());
    }
    assert!(store
        .load_tracker_task(second["id"].as_str().unwrap())
        .unwrap()
        .is_some());
    assert!(store
        .current_delivery_intent(waiting["task"]["id"].as_str().unwrap())
        .unwrap()
        .is_some());
    assert!(app.project(&sibling).is_some());
    assert_eq!(
        serde_json::to_value(store.load_review(kept_review_id).unwrap()).unwrap(),
        kept_review_before
    );
}

#[test]
fn archived_project_accepts_delete() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(root.path());
    let task = filed(&mut app, &project, "Archived task");
    call(&mut app, "project.archive", json!({"project_id":project}));
    call(
        &mut app,
        "project.delete",
        json!({"project_id":project,"confirm":true,"tasks":"delete"}),
    );
    assert!(app.project(&project).is_none());
    assert!(app
        .store
        .as_ref()
        .unwrap()
        .load_tracker_task(task["id"].as_str().unwrap())
        .unwrap()
        .is_none());
}

#[test]
fn archived_project_accepts_restore_and_delete() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(root.path());
    let task = filed(&mut app, &project, "Restored task");
    call(&mut app, "project.archive", json!({"project_id":project}));
    call(&mut app, "project.restore", json!({"project_id":project}));
    assert!(app.project(&project).unwrap().archived_at.is_none());
    call(&mut app, "project.archive", json!({"project_id":project}));
    call(
        &mut app,
        "project.delete",
        json!({"project_id":project,"confirm":true,"tasks":"delete"}),
    );
    assert!(app
        .store
        .as_ref()
        .unwrap()
        .load_tracker_task(task["id"].as_str().unwrap())
        .unwrap()
        .is_none());
}

#[test]
fn parked_home_return_binds_only_exact_project_keys() {
    let root = tempfile::tempdir().unwrap();
    let away = root.path().join("away");
    let config_path = root.path().join("config.json");
    std::fs::write(
        &config_path,
        json!({"projects":[
            {"id":"proj-1","path":away,"name":"Legacy"}
        ]})
        .to_string(),
    )
    .unwrap();
    // Migrate while the legacy home is absent, then load its sibling first.
    let app = super::project_agent::rooted(root.path())
        .with_config(&config_path)
        .unwrap();
    assert_eq!(app.projects.parked().len(), 1);
    let mut config: Value = serde_json::from_slice(&std::fs::read(&config_path).unwrap()).unwrap();
    config["projects"].as_array_mut().unwrap().insert(
        0,
        json!({
            "id":"proj-2","environment_id":"env-1","project_key":"project:sibling",
            "home_path":away,"path":away,"name":"Sibling","added_sources":[]
        }),
    );
    std::fs::write(&config_path, config.to_string()).unwrap();
    let mut app = super::project_agent::rooted(root.path())
        .with_config(&config_path)
        .unwrap();
    assert_eq!(app.projects.parked().len(), 2);
    let refused = app.handle(req("environment.delete", json!({"environment_id":"env-1"})));
    assert_eq!(refused["error"], "Delete its 2 projects first");
    app.projects
        .retain_entity_path("legacy-owner".into(), away.display().to_string());
    app.projects
        .retain_entity_path("sibling-owner".into(), "project:sibling".into());
    std::fs::create_dir(&away).unwrap();
    assert_eq!(app.revive_parked_projects(), ["proj-2", "proj-1"]);
    assert_eq!(app.projects.project_id_of("legacy-owner"), Some("proj-1"));
    assert_eq!(app.projects.project_id_of("sibling-owner"), Some("proj-2"));
    assert_eq!(
        app.projects.get("proj-1").unwrap().project_key,
        away.display().to_string()
    );
}

#[test]
fn environment_delete_persists_and_preserves_sources() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(root.path());
    let config = root.path().join("config.json");
    app.config_path = Some(config.clone());
    let source = app.project(&project).unwrap().home_path.clone();
    call(
        &mut app,
        "project.delete",
        json!({"project_id":project,"confirm":true}),
    );
    // A config failure must leave the registration available for retry.
    app.config_path = Some(root.path().to_path_buf());
    let refused = app.handle(req("environment.delete", json!({"environment_id":"env-1"})));
    assert_eq!(refused["ok"], false, "{refused}");
    assert!(app.projects.environment("env-1").is_some());
    app.config_path = Some(config.clone());
    call(
        &mut app,
        "environment.delete",
        json!({"environment_id":"env-1"}),
    );
    assert!(source.exists());
    let reloaded = super::project_agent::rooted(root.path())
        .with_config(config)
        .unwrap();
    assert!(reloaded.projects.environment("env-1").is_none());
}

#[test]
fn legacy_project_delete_preserves_task_history() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(root.path());
    let task = filed(&mut app, &project, "Retained history");
    call(
        &mut app,
        "project.delete",
        json!({"project_id":project,"confirm":true}),
    );
    assert!(app
        .store
        .as_ref()
        .unwrap()
        .load_tracker_task(task["id"].as_str().unwrap())
        .unwrap()
        .is_some());
}

#[test]
fn link_workspace_of_sibling_project_refused() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(root.path());
    let sibling = app.add_environment_sibling_for_test(&project);
    let task = filed(&mut app, &project, "Scoped link");
    let foreign = workspace(&mut app, &sibling, "Sibling checkout");
    let refusal = link_task(
        &mut app,
        json!({"task_id":task["id"],"workspace_id":foreign}),
    );
    assert_eq!(refusal["ok"], false, "{refusal}");
    let saved = app
        .store
        .as_ref()
        .unwrap()
        .load_tracker_task(task["id"].as_str().unwrap())
        .unwrap()
        .unwrap();
    assert!(saved.links.workspace_ids.is_empty());
}

#[test]
fn environment_delete_revocation_failure_preserves_config() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(root.path());
    let config = root.path().join("config.json");
    app.config_path = Some(config.clone());
    call(
        &mut app,
        "project.delete",
        json!({"project_id":project,"confirm":true}),
    );
    let before = std::fs::read(&config).unwrap();
    app.environment_secret_revocation_error =
        Some("injected environment secret revocation failure".into());
    let refused = app.handle(req("environment.delete", json!({"environment_id":"env-1"})));
    assert_eq!(refused["ok"], false, "{refused}");
    assert_eq!(
        refused["error"],
        "injected environment secret revocation failure"
    );
    assert_eq!(std::fs::read(&config).unwrap(), before);
    assert!(app.projects.environment("env-1").is_some());
    app.environment_secret_revocation_error = None;
    call(
        &mut app,
        "environment.delete",
        json!({"environment_id":"env-1"}),
    );
    assert!(app.projects.environment("env-1").is_none());
}

//! Off-lock preparations must satisfy the current registry when they settle.
use super::*;

fn state_at(root: &std::path::Path) -> AppState {
    let config = root.join("config.json");
    std::fs::write(
        &config,
        json!({"projects_dir":root.join("projects")}).to_string(),
    )
    .unwrap();
    let context =
        crate::harness::HarnessContext::resolved(root.join("mcp.sock"), root.join("state"))
            .unwrap();
    AppState::new_unrooted_configured(root.join("workspaces"), "main", true, context)
        .with_config(config)
        .unwrap()
}

#[test]
fn interleaved_same_remote_clones_refuse_second_settlement() {
    let root = tempfile::tempdir().unwrap();
    let upstream = init_repo_named(root.path(), "upstream");
    let mut app = state_at(root.path());
    let mut prepared = Vec::new();
    for name in ["first", "second"] {
        let home = root.path().join(name);
        std::fs::create_dir(&home).unwrap();
        let project = app.add_project(home, "main".into());
        let params = json!({"project_id":project,"remote":upstream,"name":"fresh"});
        let (admitted, work) = app.dispatch_deferring("project.add_source", &params);
        assert!(admitted.is_ok(), "{admitted:?}");
        prepared.push((params, work.unwrap().run()));
    }
    let mut prepared = prepared.into_iter();
    let (params, done) = prepared.next().unwrap();
    let first = app.apply_deferred("project.add_source", &params, done);
    assert!(first.is_ok(), "{first:?}");
    let (params, done) = prepared.next().unwrap();
    let second = app.apply_deferred("project.add_source", &params, done);
    assert!(
        second.is_err(),
        "second clone published the same remote: {second:?}"
    );
    assert!(second.unwrap_err().to_string().contains("remote"));
    assert_eq!(app.sources_for("proj-1").unwrap().len(), 2);
    assert_eq!(app.sources_for("proj-2").unwrap().len(), 1);
    assert!(root
        .path()
        .join("projects/first-sources/fresh/.git")
        .is_dir());
}

#[test]
fn interleaved_new_projects_refuse_containing_registered_source() {
    let root = tempfile::tempdir().unwrap();
    let parent = root.path().join("parent");
    let child = parent.join("child");
    std::fs::create_dir_all(&child).unwrap();
    let mut app = state_at(root.path());
    let mut prepared = Vec::new();
    for (name, directory) in [("parent", parent), ("child", child)] {
        let params = json!({"name":name,"sources":[{"path":directory}]});
        let (admitted, work) = app.dispatch_deferring("project.create", &params);
        assert!(admitted.is_ok(), "{admitted:?}");
        prepared.push((params, work.unwrap().run()));
    }
    let mut prepared = prepared.into_iter();
    let (params, done) = prepared.next().unwrap();
    assert!(app.apply_deferred("project.create", &params, done).is_ok());
    let before = std::fs::read(root.path().join("config.json")).unwrap();
    let (params, done) = prepared.next().unwrap();
    let answer = app.apply_deferred("project.create", &params, done);
    assert!(answer.is_err(), "nested project was published: {answer:?}");
    assert_eq!(
        std::fs::read(root.path().join("config.json")).unwrap(),
        before
    );
    assert_eq!(app.projects.len(), 1);
}

#[test]
fn legacy_project_registration_refuses_containing_registered_source() {
    use crate::lifecycle::{OpenRepo, WorktreeMutation};
    let root = tempfile::tempdir().unwrap();
    let parent = root.path().join("parent");
    let child = parent.join("child");
    std::fs::create_dir_all(&child).unwrap();
    let mut app = state_at(root.path());
    let prepared = [parent, child].map(|path| {
        OpenRepo {
            path,
            requested_base: None,
        }
        .perform()
        .unwrap()
        .output
    });
    let [parent, child] = prepared;
    app.register_opened_repository(parent).unwrap();
    let answer = app.register_opened_repository(child);
    assert!(
        answer.is_err(),
        "legacy door published nested source: {answer:?}"
    );
    assert_eq!(app.projects.len(), 1);
}

#[test]
fn new_project_clone_refuses_remote_already_in_local_batch_source() {
    for clone_first in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let upstream = init_repo_named(root.path(), "upstream");
        let local = init_repo_named(root.path(), "local");
        git2::Repository::open(&local)
            .unwrap()
            .remote("origin", upstream.to_str().unwrap())
            .unwrap();
        let mut app = state_at(root.path());
        let before = std::fs::read(root.path().join("config.json")).unwrap();
        let mut sources = vec![
            json!({"path":local,"name":"local"}),
            json!({"remote":upstream,"name":"cloned"}),
        ];
        if clone_first {
            sources.reverse();
        }
        let answer = app.handle(req(
            "project.create",
            json!({"name":"attempt","sources":sources}),
        ));
        assert_eq!(answer["ok"], false, "clone_first={clone_first}: {answer}");
        assert!(
            answer["error"].as_str().unwrap().contains("remote"),
            "{answer}"
        );
        assert_eq!(app.projects.len(), 0);
        assert_eq!(
            std::fs::read(root.path().join("config.json")).unwrap(),
            before
        );
        assert!(
            local.join(".git").is_dir(),
            "the existing local source is never cleanup-owned"
        );
        assert!(!root.path().join("projects/attempt-sources/cloned").exists());
    }
}

fn assert_parked_home_refuses_source_creation(clone_request: bool) {
    let root = tempfile::tempdir().unwrap();
    let upstream = init_repo_named(root.path(), "upstream");
    let home = root.path().join("projects/attempt-sources/fresh");
    let missing = root.path().join("missing-secondary");
    let config = root.path().join("config.json");
    std::fs::write(
        &config,
        json!({
            "projects_dir":root.path().join("projects"),
            "environments":[{"id":"env-1","name":"parked","next_source":3,"sources":[
                {"id":"source-2","path":missing,"name":"secondary","mount":"secondary"}
            ]}],
            "projects":[{"id":"proj-1","environment_id":"env-1","project_key":"project:parked",
                "path":home,"home_path":home,"added_sources":[]}]
        })
        .to_string(),
    )
    .unwrap();
    let context = crate::harness::HarnessContext::resolved(
        root.path().join("mcp.sock"),
        root.path().join("state"),
    )
    .unwrap();
    let mut app =
        AppState::new_unrooted_configured(root.path().join("workspaces"), "main", true, context)
            .with_config(&config)
            .unwrap();
    assert_eq!(app.projects.parked().len(), 1);
    assert!(
        app.projects
            .registered_sources()
            .iter()
            .all(|source| source.path != home),
        "the retained home is intentionally absent from authoritative source references"
    );
    let params = if clone_request {
        json!({"name":"attempt","sources":[{"name":"fresh","remote":upstream}]})
    } else {
        json!({"name":"fresh","parent":home.parent().unwrap()})
    };
    let (answer, work) = app.dispatch_deferring("project.create", &params);
    assert!(answer.is_err(), "creation may revive an existing project's home while its failing batch owns cleanup: {answer:?}");
    assert!(
        work.is_none(),
        "no clone or repository initialization may start in a retained home"
    );
    assert!(!home.exists());
    assert_eq!(app.projects.parked().len(), 1);
    assert_eq!(app.projects.parked()[0]["home_path"], json!(home));
    assert!(app.projects.get("proj-1").is_none());
}

#[test]
fn clone_destination_refuses_retained_parked_home_before_preparation() {
    assert_parked_home_refuses_source_creation(true);
}

#[test]
fn legacy_creation_refuses_retained_parked_home_before_preparation() {
    assert_parked_home_refuses_source_creation(false);
}

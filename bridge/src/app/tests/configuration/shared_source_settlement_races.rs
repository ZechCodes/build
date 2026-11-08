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

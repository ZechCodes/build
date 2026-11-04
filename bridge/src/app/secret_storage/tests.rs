use super::*;
use crate::app::projects::ProjectSource;
use crate::harness::HarnessContext;
use serde_json::json;
use std::path::PathBuf;

fn app(root: &Path) -> AppState {
    let context =
        HarnessContext::resolved(root.join("synthetic-mcp.sock"), root.to_path_buf()).unwrap();
    let mut app = AppState::new_unrooted_configured(root.join("worktrees"), "main", true, context)
        .with_task_store(root.join("tasks"))
        .unwrap();
    app.projects_dir = root.join("projects");
    app
}

fn protected_app(root: &Path) -> AppState {
    app(root)
        .with_secret_vault(root, "synthetic-device")
        .unwrap()
}

#[test]
fn secrets_startup_rejects_registered_source_and_directory_root_overlaps() {
    for kind in ["source", "projects", "worktrees"] {
        let root = tempfile::tempdir().unwrap();
        let mut app = app(root.path());
        match kind {
            "source" => {
                app.add_project(root.path().to_path_buf(), "main".into());
            }
            "projects" => app.projects_dir = root.path().to_path_buf(),
            _ => app.worktrees_root = root.path().to_path_buf(),
        }
        assert!(
            app.with_secret_vault(root.path(), "synthetic-device")
                .is_err(),
            "{kind}"
        );
        assert!(!root.path().join("secrets").exists());
    }
}

#[test]
fn secrets_startup_rejects_parked_primary_and_secondary_sources() {
    for secondary in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let mut app = app(root.path());
        let blocked = root.path().join("secrets/not-created");
        let primary = if secondary {
            root.path().join("safe-missing")
        } else {
            blocked.clone()
        };
        app.projects.park(json!({
            "id":"proj-7", "path":primary,
            "sources":[{"id":"source-1","path":primary},{"id":"source-2","path":blocked}]
        }));
        assert!(app
            .with_secret_vault(root.path(), "synthetic-device")
            .is_err());
        assert!(!root.path().join("secrets").exists());
    }
}

#[test]
fn secrets_project_create_and_multisource_admission_refuse_vault_before_deferring() {
    let root = tempfile::tempdir().unwrap();
    let mut app = protected_app(root.path());
    let result = app
        .project_create(&json!({"name":"synthetic-source","parent":root.path().join("secrets")}));
    assert!(result.is_err());
    assert!(app.deferred_work.is_none());
    assert!(!root.path().join("secrets/synthetic-source").exists());
    let result =
        app.project_create(&json!({"name":"synthetic-project","sources":[{"path":root.path()}]}));
    assert!(result.is_err());
    assert!(app.deferred_work.is_none());
}

#[test]
fn secrets_source_add_and_move_refuse_vault_and_preserve_registered_sources() {
    let root = tempfile::tempdir().unwrap();
    let primary = root.path().join("synthetic-primary");
    let secondary = root.path().join("synthetic-secondary");
    for path in [&primary, &secondary] {
        std::fs::create_dir(path).unwrap();
    }
    let mut app = protected_app(root.path());
    let id = app.add_project(primary.clone(), "main".into());
    let sources: Vec<_> = [&primary, &secondary]
        .into_iter()
        .enumerate()
        .map(|(index, path)| {
            ProjectSource::added(
                format!("source-{}", index + 1),
                format!("synthetic-{index}"),
                format!("synthetic-{index}"),
                path.clone(),
                false,
                "main".into(),
            )
        })
        .collect();
    app.projects.set_sources(&id, sources);
    let before: Vec<PathBuf> = app
        .projects
        .get(&id)
        .unwrap()
        .sources
        .iter()
        .map(|source| source.path.clone())
        .collect();
    assert!(app
        .project_add_source(&json!({"project_id":id,"path":root.path()}))
        .is_err());
    assert!(app
        .project_update_source(
            &json!({"project_id":id,"source_id":"source-2","path":root.path().join("secrets")})
        )
        .is_err());
    assert!(app.deferred_work.is_none());
    let after: Vec<PathBuf> = app
        .projects
        .get(&id)
        .unwrap()
        .sources
        .iter()
        .map(|source| source.path.clone())
        .collect();
    assert_eq!(before, after);
}

#[test]
fn secrets_safe_project_admission_remains_available() {
    let root = tempfile::tempdir().unwrap();
    let mut app = protected_app(root.path());
    let safe = root.path().join("synthetic-safe");
    std::fs::create_dir(&safe).unwrap();
    assert!(app
        .project_create(&json!({"name":"synthetic-project","sources":[{"path":safe}]}))
        .is_ok());
    assert!(app.deferred_work.is_some());
}

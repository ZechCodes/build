use super::*;
use crate::app::projects::ProjectSource;
use crate::harness::HarnessContext;
use crate::workspace::{WorkspaceSource, WorkspaceStatus, MANIFEST_FILE};
use std::fs;

struct Fixture {
    _dir: tempfile::TempDir,
    app: AppState,
    source: PathBuf,
    state_root: PathBuf,
    copies: PathBuf,
    project: String,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("synthetic-existing-source");
    let state_root = source.join("synthetic-device-state");
    let vault = state_root.join("secrets");
    fs::create_dir_all(&vault).unwrap();
    fs::write(vault.join("vault.key"), b"synthetic vault fixture only").unwrap();
    let copies = dir.path().join("external-worktrees");
    fs::create_dir(&copies).unwrap();
    let context =
        HarnessContext::resolved(state_root.join("synthetic-mcp.sock"), state_root.clone())
            .unwrap();
    let mut app = AppState::new_unrooted_configured(&copies, "main", true, context)
        .with_projects_dir_default(state_root.join("projects"));
    let project = app.add_project(source.clone(), "main".into());
    let app = app
        .with_secret_vault(&state_root, "synthetic-device")
        .unwrap();
    assert!(app.secret_service().is_none());
    Fixture {
        _dir: dir,
        app,
        source,
        state_root,
        copies,
        project,
    }
}

fn saved_source(fixture: &Fixture) -> WorkspaceSource {
    WorkspaceSource {
        id: "source-1".into(),
        name: "synthetic".into(),
        mount: "synthetic".into(),
        path: fixture.source.clone(),
        is_git: false,
        base_branch: "main".into(),
    }
}

fn run_pending(app: &mut AppState) -> Result<Value, String> {
    let Some(DeferredWork::Git(work)) = app.deferred_work.take() else {
        panic!("pending filesystem work");
    };
    let result = work.call.run(&work.params);
    app.workspaces.reload().unwrap();
    result
}

fn no_copied_key(root: &Path) -> bool {
    let Ok(entries) = fs::read_dir(root) else {
        return true;
    };
    entries.map(Result::unwrap).all(|entry| {
        entry.file_name() != "vault.key"
            && (!entry.file_type().unwrap().is_dir() || no_copied_key(&entry.path()))
    })
}

fn safe_project(fixture: &mut Fixture) -> (String, PathBuf) {
    let source = fixture._dir.path().join("synthetic-safe-source");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("kept.txt"), b"synthetic safe content").unwrap();
    let id = fixture.app.add_project(source.clone(), "main".into());
    (id, source)
}

#[test]
fn secrets_workspace_create_refuses_existing_ancestor_source_before_reservation() {
    let mut fixture = fixture();
    let result = fixture.app.workspace_create(
        &json!({"project_id":fixture.project,"name":"unsafe","isolation":"worktree"}),
    );
    assert!(result.is_err());
    assert!(fixture.app.deferred_work.is_none());
    assert!(!fixture
        .copies
        .join("workspaces")
        .join(&fixture.project)
        .exists());
    assert!(no_copied_key(&fixture.copies));
    assert_eq!(
        fs::read(fixture.state_root.join("secrets/vault.key")).unwrap(),
        b"synthetic vault fixture only"
    );
}

#[test]
fn secrets_workspace_safe_sibling_still_materializes() {
    let mut fixture = fixture();
    let (project, _) = safe_project(&mut fixture);
    fixture
        .app
        .workspace_create(&json!({"project_id":project,"name":"safe","isolation":"worktree"}))
        .unwrap();
    let result = run_pending(&mut fixture.app).unwrap();
    let copied = Path::new(result["directories"][0]["path"].as_str().unwrap());
    assert_eq!(
        fs::read(copied.join("kept.txt")).unwrap(),
        b"synthetic safe content"
    );
    assert!(no_copied_key(&fixture.copies));
}

#[test]
fn secrets_workspace_local_directory_admission_refuses_source_id_and_raw_vault_path() {
    let mut fixture = fixture();
    let (project, _) = safe_project(&mut fixture);
    fixture
        .app
        .workspace_create(&json!({"project_id":project,"name":"safe","isolation":"worktree"}))
        .unwrap();
    let result = run_pending(&mut fixture.app).unwrap();
    let workspace_id = result["workspace_id"].as_str().unwrap();
    let manifest = Path::new(result["root"].as_str().unwrap()).join(MANIFEST_FILE);
    let mut sources = fixture.app.projects.get(&project).unwrap().sources.clone();
    sources.push(ProjectSource::added(
        "source-vault".into(),
        "synthetic-vault".into(),
        "synthetic-vault".into(),
        fixture.source.clone(),
        false,
        "main".into(),
    ));
    fixture.app.projects.set_sources(&project, sources);
    fixture.app.projects.remove(&fixture.project);
    let before = fs::read(&manifest).unwrap();
    for selector in [
        json!({"source_id":"source-vault"}),
        json!({"path":fixture.state_root.join("secrets")}),
    ] {
        let mut params = selector;
        params["workspace_id"] = json!(workspace_id);
        assert!(fixture.app.workspace_add_directory(&params).is_err());
        assert!(fixture.app.deferred_work.is_none());
        assert_eq!(fs::read(&manifest).unwrap(), before);
    }
    assert!(no_copied_key(&fixture.copies));
}

#[test]
fn secrets_saved_workspace_retry_and_deferred_claim_refuse_before_manifest_changes() {
    let mut fixture = fixture();
    let source = saved_source(&fixture);
    let workspace = fixture
        .app
        .workspaces
        .begin_with_isolation(
            &fixture.project,
            "retry",
            std::slice::from_ref(&source),
            Isolation::Worktree,
        )
        .unwrap();
    fixture
        .app
        .workspaces
        .fail_if_still_provisioning(&workspace.id);
    let manifest = workspace.root.join(MANIFEST_FILE);
    let before = fs::read(&manifest).unwrap();
    assert!(fixture
        .app
        .workspace_retry(&json!({"workspace_id":workspace.id}))
        .is_err());
    assert!(fixture.app.deferred_work.is_none());
    assert_eq!(
        fixture.app.workspaces.get(&workspace.id).unwrap().status,
        WorkspaceStatus::Failed
    );
    assert_eq!(fs::read(&manifest).unwrap(), before);
    let reserved = fixture
        .app
        .workspaces
        .reserve_delivery_workspace(&fixture.project, "reserved", &[source], Isolation::Worktree)
        .unwrap();
    assert!(fixture
        .app
        .prepare_deferred_workspace_snapshot(&reserved)
        .is_err());
    assert!(!reserved.root.exists());
    assert!(no_copied_key(&fixture.copies));
}

#[test]
fn secrets_offlock_workspace_copy_rechecks_redirected_source() {
    let mut fixture = fixture();
    let (project, safe) = safe_project(&mut fixture);
    fixture
        .app
        .workspace_create(&json!({"project_id":project,"name":"redirected","isolation":"worktree"}))
        .unwrap();
    fs::rename(&safe, safe.with_extension("original")).unwrap();
    std::os::unix::fs::symlink(&fixture.source, &safe).unwrap();
    assert!(run_pending(&mut fixture.app).is_err());
    assert!(no_copied_key(&fixture.copies));
    assert_eq!(
        fs::read(fixture.state_root.join("secrets/vault.key")).unwrap(),
        b"synthetic vault fixture only"
    );
}

#[test]
fn secrets_offlock_added_directory_rechecks_redirected_local_path() {
    let mut fixture = fixture();
    let (project, _) = safe_project(&mut fixture);
    fixture
        .app
        .workspace_create(&json!({"project_id":project,"name":"safe","isolation":"worktree"}))
        .unwrap();
    let workspace = run_pending(&mut fixture.app).unwrap();
    let extra = fixture._dir.path().join("synthetic-extra-source");
    fs::create_dir(&extra).unwrap();
    fs::write(extra.join("kept.txt"), b"synthetic extra content").unwrap();
    fixture
        .app
        .workspace_add_directory(&json!({"workspace_id":workspace["workspace_id"],"path":extra}))
        .unwrap();
    fs::rename(&extra, extra.with_extension("original")).unwrap();
    std::os::unix::fs::symlink(&fixture.source, &extra).unwrap();
    assert!(run_pending(&mut fixture.app).is_err());
    assert!(no_copied_key(&fixture.copies));
    assert_eq!(
        fs::read(fixture.state_root.join("secrets/vault.key")).unwrap(),
        b"synthetic vault fixture only"
    );
}

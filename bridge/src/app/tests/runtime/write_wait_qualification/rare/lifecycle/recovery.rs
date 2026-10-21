use super::*;

pub(super) async fn execute() -> Vec<VerifiedStep> {
    let mut steps = retry().await;
    steps.extend(reclaim().await);
    steps
}

async fn retry() -> Vec<VerifiedStep> {
    let directory = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(directory.path()).unwrap();
    let repo = init_repo_named(&root, "source");
    let mut app = configured(&repo, &root);
    let project = app.project_at(0).id.clone();
    let sources: Vec<_> = app
        .project_at(0)
        .sources
        .iter()
        .map(|source| crate::workspace::WorkspaceSource {
            id: source.id.clone(),
            name: source.name.clone(),
            mount: source.mount.clone(),
            path: source.path.clone(),
            is_git: source.is_git,
            base_branch: source.base_branch.clone(),
        })
        .collect();
    let workspace = app
        .workspaces
        .begin_with_isolation(
            &project,
            "retry",
            &sources,
            crate::isolation::Isolation::Worktree,
        )
        .unwrap();
    let failed = app.workspaces.provision(
        &workspace.id,
        &sources,
        crate::isolation::Isolation::Worktree,
        |_, _, _| Err("qualification fixture interrupted provisioning".to_string()),
    );
    assert!(failed.is_err());
    app.workspaces = crate::workspace::WorkspaceRegistry::recover(app.workspaces.root()).unwrap();
    assert_eq!(
        app.workspaces.get(&workspace.id).unwrap().status,
        crate::workspace::WorkspaceStatus::Failed
    );
    assert!(!workspace.directories[0].path.exists());
    let mut fixture = RareFixture::new("lifecycle-retry", app);
    let proof = fixture
        .call("workspace.retry", json!({"workspace_id": workspace.id}))
        .await;
    let row = workspace_row(&fixture, &workspace.id);
    assert_eq!(row["status"], "ready");
    assert_eq!(row["directories"][0]["status"], "ready");
    let checkout = PathBuf::from(row["directories"][0]["path"].as_str().unwrap());
    assert_eq!(head(&checkout), head(&repo));
    assert_eq!(
        std::fs::read_to_string(checkout.join("README.md")).unwrap(),
        "# project\n"
    );
    assert_eq!(
        disk_json(&workspace.root.join(crate::workspace::MANIFEST_FILE))["status"],
        "ready"
    );
    fixture.verify(proof, json!({"retried_workspace": workspace.id, "checkout": checkout, "head": head(&checkout)})).await;
    fixture.finish().await
}

async fn reclaim() -> Vec<VerifiedStep> {
    use crate::app::tests::workspace_reclaim::{
        finish, linked_workspace, root_and_checkout, timeline_kinds,
    };
    let (_directory, state, _project, workspace, task) = linked_workspace();
    finish(&state, &task);
    let (root, checkout) = root_and_checkout(&state, &workspace);
    let source = state
        .lock()
        .unwrap()
        .workspaces
        .get(&workspace)
        .unwrap()
        .directories[0]
        .source_path
        .clone();
    let registration = crate::isolation::checkout_name(&checkout).unwrap();
    let mut fixture = RareFixture::shared("lifecycle-reclaim", state);
    let proof = fixture
        .call("workspace.reclaim", json!({"workspace_id": workspace}))
        .await;
    assert_eq!(proof.reply["result"]["deleted"], true);
    assert_eq!(proof.reply["result"]["branches"][0]["outcome"], "deleted");
    assert!(!root.exists());
    assert!(git2::Repository::open(&source)
        .unwrap()
        .find_worktree(&registration)
        .is_err());
    assert!(fixture
        .state()
        .lock()
        .unwrap()
        .workspaces
        .get(&workspace)
        .is_none());
    let entries = timeline_kinds(fixture.state(), &task);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["kind"], "workspace_reclaimed");
    assert_eq!(entries[0]["actor"], json!({"kind": "user"}));
    assert_eq!(
        read(&fixture, "tasks.get", json!({"task_id": task}))["task"]["state"],
        "open"
    );
    fixture.verify(proof, json!({"reclaimed_workspace": workspace, "task_event": entries[0], "registration_removed": registration})).await;
    fixture.finish().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failed_workspace_retry_has_independent_effect_proof() {
    let steps = retry().await;
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].method, "workspace.retry");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn linked_workspace_reclaim_has_independent_effect_proof() {
    let steps = reclaim().await;
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].method, "workspace.reclaim");
}

use super::*;

pub(super) async fn create(context: &mut ProjectFixture) -> String {
    let proof = context
        .fixture
        .call_mcp(
            &context.caller.agent,
            &context.caller.token,
            BridgeAction::CreateWorkspace {
                name: "mcp-owned".into(),
                isolation: Some("worktree".into()),
            },
        )
        .await;
    let workspace = proof.reply["result"]["workspace_id"]
        .as_str()
        .unwrap()
        .to_string();
    let row = workspace_row(context, &workspace);
    assert_eq!(row["project_id"], context.project);
    assert_eq!(row["status"], "ready");
    assert_eq!(row["created_by_agent"], true);
    let root = PathBuf::from(row["root"].as_str().unwrap());
    let checkout = PathBuf::from(row["directories"][0]["path"].as_str().unwrap());
    assert_eq!(head(&checkout), head(&context.repo));
    assert_eq!(
        disk_json(&root.join(crate::workspace::MANIFEST_FILE))["created_by_agent"],
        true
    );
    context.fixture.verify(proof, json!({"caller":context.caller.agent,"workspace":workspace,"created_by_agent":true,"root":root,"head":head(&checkout)})).await;
    workspace
}

pub(super) async fn directories(context: &mut ProjectFixture, workspace: &str) {
    let assets = context.root.join("extra-directory");
    std::fs::create_dir(&assets).unwrap();
    std::fs::write(assets.join("sentinel.txt"), "directory copy bytes\n").unwrap();
    let proof = context
        .fixture
        .call_mcp(
            &context.caller.agent,
            &context.caller.token,
            BridgeAction::AddWorkspaceDirectory {
                workspace_id: workspace.into(),
                source_id: None,
                path: Some(assets.display().to_string()),
                remote: None,
                name: Some("assets".into()),
            },
        )
        .await;
    let row = workspace_row(context, workspace);
    assert_eq!(row["directories"].as_array().unwrap().len(), 2);
    let directory = row["directories"][1]["id"].as_str().unwrap().to_string();
    let copy = PathBuf::from(row["directories"][1]["path"].as_str().unwrap());
    assert!(copy.starts_with(row["root"].as_str().unwrap()));
    assert_eq!(
        std::fs::read_to_string(copy.join("sentinel.txt")).unwrap(),
        "directory copy bytes\n"
    );
    assert_eq!(project_row(context)["sources"].as_array().unwrap().len(), 1);
    context.fixture.verify(proof, json!({"caller":context.caller.agent,"workspace":workspace,"copied_directory":directory,"copy":copy})).await;

    let proof = context
        .fixture
        .call_mcp(
            &context.caller.agent,
            &context.caller.token,
            BridgeAction::RemoveWorkspaceDirectory {
                workspace_id: workspace.into(),
                directory_id: directory.clone(),
            },
        )
        .await;
    assert!(!copy.exists());
    let row = workspace_row(context, workspace);
    assert_eq!(row["directories"].as_array().unwrap().len(), 1);
    assert!(Path::new(row["directories"][0]["path"].as_str().unwrap()).is_dir());
    assert_eq!(
        std::fs::read_to_string(assets.join("sentinel.txt")).unwrap(),
        "directory copy bytes\n"
    );
    context.fixture.verify(proof,json!({"caller":context.caller.agent,"removed_directory":directory,"source_preserved":assets})).await;
}

pub(super) async fn delete(context: &mut ProjectFixture, workspace: &str) {
    let row = workspace_row(context, workspace);
    let root = PathBuf::from(row["root"].as_str().unwrap());
    let checkout = PathBuf::from(row["directories"][0]["path"].as_str().unwrap());
    let registration = crate::isolation::checkout_name(&checkout).unwrap();
    let entity = row["entity_id"].as_str().unwrap().to_string();
    let proof = context
        .fixture
        .call_mcp(
            &context.caller.agent,
            &context.caller.token,
            BridgeAction::DeleteWorkspace {
                workspace_id: workspace.into(),
            },
        )
        .await;
    assert_eq!(proof.reply["result"]["deleted"], true);
    assert!(!root.exists());
    assert!(context.repo.join(".git").is_dir());
    assert!(git2::Repository::open(&context.repo)
        .unwrap()
        .find_worktree(&registration)
        .is_err());
    let listed = read(
        &context.fixture,
        "workspace.list",
        json!({"project_id":context.project}),
    );
    assert!(!listed["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["workspace_id"] == workspace));
    let app = context.fixture.state().lock().unwrap();
    assert!(!app.runs.contains_key(&entity));
    assert!(!app
        .store
        .as_ref()
        .unwrap()
        .load_all_runs()
        .unwrap()
        .iter()
        .any(|run| run.id == entity));
    assert!(app.runs.contains_key(&context.caller.owner));
    drop(app);
    context.fixture.verify(proof,json!({"caller":context.caller.agent,"deleted_workspace":workspace,"removed_root":root,"registration_removed":registration,"caller_owner_retained":context.caller.owner})).await;
}

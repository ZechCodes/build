use super::*;

pub(super) async fn execute() -> Vec<VerifiedStep> {
    use crate::app::tests::workspace_reclaim::{
        finish, linked_workspace, root_and_checkout, timeline_kinds,
    };
    let (_directory, state, project, workspace, task) = linked_workspace();
    finish(&state, &task);
    let requester = caller(&mut state.lock().unwrap(), &project);
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
    let mut fixture = RareFixture::shared("mcp-lifecycle-reclaim", state);
    let proof = fixture
        .call_mcp(
            &requester.agent,
            &requester.token,
            BridgeAction::ReclaimWorkspace {
                workspace_id: workspace.clone(),
            },
        )
        .await;
    assert_eq!(proof.reply["result"]["deleted"], true);
    assert_eq!(proof.reply["result"]["branches"][0]["outcome"], "deleted");
    assert!(!root.exists());
    assert!(source.join(".git").is_dir());
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
    assert_eq!(entries[0]["actor"]["agent_id"], requester.agent);
    assert_eq!(
        read(&fixture, "tasks.get", json!({"task_id":task}))["task"]["state"],
        "open"
    );
    fixture.verify(proof,json!({"caller":requester.agent,"reclaimed_workspace":workspace,"task_event":entries[0],"registration_removed":registration})).await;
    fixture.finish().await
}

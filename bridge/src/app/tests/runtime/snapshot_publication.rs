use super::*;

#[test]
fn successful_settings_reply_publishes_the_value_read_by_other_clients() {
    let directory = tempfile::tempdir().unwrap();
    let app = AppState::new_unrooted(
        directory.path().join("worktrees"),
        "main",
        true,
        directory.path().join("unused.sock").to_str().unwrap(),
    )
    .with_config(directory.path().join("config.json"))
    .unwrap();
    let state = app.shared();
    let handler = AppState::handler(Arc::clone(&state));
    let reader = SessionSender::detached("snapshot-reader");
    let writer = SessionSender::detached("snapshot-writer");
    let before = handler.call(reader.clone(), req("settings.get", json!({})));
    assert_eq!(before["ok"], true, "{before}");
    assert_eq!(before["result"]["watch_agent_filed_tasks"], false);

    let reply = handler.call(
        writer,
        req("settings.set", json!({"watch_agent_filed_tasks": true})),
    );
    assert_eq!(reply["ok"], true, "{reply}");
    let after = handler.call(reader, req("settings.get", json!({})));
    assert_eq!(after["result"]["watch_agent_filed_tasks"], true, "{after}");
    assert_eq!(
        state
            .lock()
            .unwrap()
            .read_view
            .load()
            .get("settings.get", "")
            .unwrap()["watch_agent_filed_tasks"],
        true,
        "the acknowledged value belongs to the immutable published root"
    );
}

#[test]
fn project_compaction_publication_updates_existing_agent_observations() {
    let (directory, repository) = init_repo();
    let mut app = qa_state(&repository, directory.path());
    let owner = adopted_run(&mut app, &repository, directory.path(), "snapshot-policy");
    let project = app.projects.project_id_of(&owner).unwrap().to_owned();
    let agent = app.runs[&owner].agents.iter().next().unwrap();
    let provider = agent.choice.provider.wire_id().to_owned();
    let state = app.shared();
    let handler = AppState::handler(Arc::clone(&state));
    {
        let mut app = state.lock().unwrap();
        app.patch_compaction_thresholds(
            crate::compaction::CompactionScope::Project(project),
            &json!({provider: 151_001}),
        )
        .unwrap();
    }
    let after = handler.call(
        SessionSender::detached("snapshot-policy-reader"),
        req("agent.list", json!({"entity_id": owner})),
    );
    assert_eq!(after["ok"], true, "{after}");
    assert_eq!(
        after["result"]["agents"][0]["compaction"]["configured"]["tokens"], 151_001,
        "{after}"
    );
    let app = state.lock().unwrap();
    let published = app.read_view.load();
    let roster = published.get("agent.list", &owner).unwrap();
    assert_eq!(
        roster["agents"][0]["compaction"]["configured"]["tokens"],
        151_001
    );
}

#[tokio::test]
async fn subscribed_workspace_write_is_visible_to_snapshot_and_next_push() {
    let (directory, repository) = init_repo();
    let (state, handler, sender, mut incoming, key) =
        push::greeted_push_session(&repository, directory.path());
    let project = state.lock().unwrap().project_at(0).id.clone();
    let created = handler.call(
        sender.clone(),
        req(
            "workspace.create",
            json!({
                "project_id": project, "name": "before publication", "isolation": "worktree"
            }),
        ),
    );
    assert_eq!(created["ok"], true, "{created}");
    let workspace = created["result"]["workspace_id"].as_str().unwrap();
    let snapshots = state.lock().unwrap().snapshot_service();
    push::watch_everything(&handler, &sender);
    push::settled_pushes(&mut incoming, &key).await;

    let reply = handler.call(
        sender,
        req(
            "workspace.rename",
            json!({"workspace_id": workspace, "name": "after publication"}),
        ),
    );
    assert_eq!(reply["ok"], true, "{reply}");
    // This calls the immutable reader captured before the write, so it cannot
    // refresh from the ordinary AppState handler after the acknowledgement.
    let read = snapshots.call(req("workspace.get", json!({"workspace_id": workspace})));
    assert_eq!(read["ok"], true, "{read}");
    assert_eq!(read["result"]["name"], "after publication", "{read}");

    let pushed = push::settled_pushes(&mut incoming, &key).await;
    let row = pushed
        .iter()
        .filter(|frame| frame["type"] == "changes")
        .flat_map(|frame| frame["items"].as_array().into_iter().flatten())
        .filter(|item| item["entity_id"] == crate::changes::BOARD_ITEM_ID)
        .flat_map(|item| item["state"]["workspaces"].as_array().into_iter().flatten())
        .find(|row| row["workspace_id"] == workspace)
        .unwrap_or_else(|| panic!("no published workspace in the completed flush: {pushed:?}"));
    assert_eq!(row["name"], read["result"]["name"]);
    assert_eq!(row["project_id"], read["result"]["project_id"]);
}

#[test]
fn first_snapshot_keeps_seeded_workspace_rows_and_performs_no_path_lookups() {
    // Real managed workspaces at two sizes complement the existing #672
    // synthetic 16/64-owner counting tests. Reads must do zero owner searches.
    for count in [4, 16] {
        let (directory, repository) = init_repo();
        let mut app = qa_state(&repository, directory.path());
        let project = app.project_at(0).id.clone();
        let mut expected = Vec::new();
        for index in 0..count {
            let workspace = crate::app::tests::project_agent::workspace(
                &mut app,
                &project,
                &format!("first snapshot {index}"),
            );
            let conversation = app.handle(req(
                "workspace.ensure_conversation",
                json!({"workspace_id": workspace}),
            ));
            assert_eq!(conversation["ok"], true, "{conversation}");
            expected.push((
                workspace,
                conversation["result"]["entity_id"]
                    .as_str()
                    .unwrap()
                    .to_owned(),
            ));
        }
        assert!(!app.snapshot_projection_initialized());
        let state = app.shared();
        let snapshots = state.lock().unwrap().snapshot_service();
        let ((board, workspaces), comparisons) =
            crate::app::workspaces::count_path_comparisons(|| {
                (
                    snapshots.call(req("board.list", json!({}))),
                    snapshots.call(req("workspace.list", json!({"project_id": project}))),
                )
            });
        assert_eq!(board["ok"], true, "{board}");
        assert_eq!(workspaces["ok"], true, "{workspaces}");
        let listed = workspaces["result"]["workspaces"].as_array().unwrap();
        assert_eq!(listed.len(), count);
        let items = board["result"]["items"].as_array().unwrap();
        for (workspace, owner) in expected {
            assert!(
                listed.iter().any(|row| row["workspace_id"] == workspace),
                "{workspaces}"
            );
            assert!(items.iter().any(|row| row["run_id"] == owner), "{board}");
        }
        assert_eq!(
            comparisons, 0,
            "first read searched paths for {count} owners"
        );
    }
}

struct BranchSnapshotFixture {
    _directory: tempfile::TempDir,
    handler: crate::carrier::FrameHandler,
    snapshots: crate::app::read_view::SnapshotService,
    owner: String,
    workspace: String,
    path: PathBuf,
    recorded_branch: String,
}

impl BranchSnapshotFixture {
    fn new() -> Self {
        let (directory, repository) = init_repo();
        let mut app = qa_state(&repository, directory.path());
        let owner = adopted_run(&mut app, &repository, directory.path(), "snapshot-original");
        let path = app.runs[&owner].worktree.path.clone();
        let recorded_branch = app.runs[&owner].worktree.recorded_branch.clone();
        git_in(&path, &["branch", "snapshot-next"]);
        git_in(&path, &["tag", "snapshot-anchor"]);
        let state = app.shared();
        let (workspace, snapshots) = {
            let app = state.lock().unwrap();
            (
                app.surviving_workspace_snapshot_of_run(&owner).unwrap(),
                app.snapshot_service(),
            )
        };
        let fixture = Self {
            _directory: directory,
            handler: AppState::handler(state),
            snapshots,
            owner,
            workspace,
            path,
            recorded_branch,
        };
        fixture.assert_snapshot_branch(&fixture.recorded_branch);
        fixture
    }

    fn checkout(&self, full_ref: &str) -> Value {
        let reply = self.handler.call(
            SessionSender::detached("snapshot-branch-writer"),
            req(
                "git.checkout_ref",
                json!({"run_id": self.owner, "full_ref": full_ref}),
            ),
        );
        assert_eq!(reply["ok"], true, "{reply}");
        reply
    }

    fn assert_snapshot_branch(&self, expected: &str) {
        let board = self.snapshots.call(req("board.list", json!({})));
        assert_eq!(board["ok"], true, "{board}");
        let row = board["result"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["run_id"] == self.owner)
            .unwrap_or_else(|| panic!("no adopted run in cached board: {board}"));
        assert_eq!(row["branch"], expected, "{board}");
        let workspace = self.snapshots.call(req(
            "workspace.get",
            json!({"workspace_id": self.workspace}),
        ));
        assert_eq!(workspace["ok"], true, "{workspace}");
        assert_eq!(
            workspace["result"]["run"]["branch"], expected,
            "{workspace}"
        );
    }
}

#[test]
fn successful_checkout_reply_publishes_branch_to_board_and_workspace_snapshots() {
    let fixture = BranchSnapshotFixture::new();
    let reply = fixture.checkout("refs/heads/snapshot-next");
    let repository = git2::Repository::open(&fixture.path).unwrap();
    let head = repository.head().unwrap();
    assert!(head.is_branch());
    assert_eq!(head.shorthand(), Some("snapshot-next"));
    assert_eq!(reply["result"]["branch"], "snapshot-next", "{reply}");
    fixture.assert_snapshot_branch("snapshot-next");
}

#[test]
fn detached_checkout_publishes_the_recorded_branch_fallback() {
    let fixture = BranchSnapshotFixture::new();
    fixture.checkout("refs/heads/snapshot-next");
    let reply = fixture.checkout("refs/tags/snapshot-anchor");
    let repository = git2::Repository::open(&fixture.path).unwrap();
    assert!(repository.head_detached().unwrap());
    assert_eq!(reply["result"]["branch"], "HEAD", "{reply}");
    // Worktree::branch has always answered its recorded branch for detached
    // HEAD; retaining the last observed attached branch would change that.
    fixture.assert_snapshot_branch(&fixture.recorded_branch);
}

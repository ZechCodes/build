//! Saved, stable inputs are created before timed admission starts.
use super::*;

pub(super) const COMMENT_BODY: &str = "busy qualification stored read witness";

pub(in super::super) struct Fixture {
    pub project: String,
    pub workspace: String,
    pub workspace_agent: String,
    pub project_agent: String,
    pub task: String,
    pub dependent: String,
    pub comment: String,
    pub snapshot: String,
    pub directory: String,
    pub router: String,
    pub router_capture: String,
    pub blob_bytes: Vec<u8>,
}

impl Fixture {
    pub(in super::super) fn seed(
        app: &mut AppState,
        workspace: &str,
        workspace_agent: &str,
        task: &str,
        dependent: &str,
    ) -> Self {
        let project = app.project_at(0).id.clone();
        let project_owner = app
            .project_snapshot_conversation_run(&project)
            .expect("qualification seeds a real project roster");
        let project_agent = app
            .project_pin(&project_owner)
            .expect("qualification project owner retains a pin")
            .to_owned();
        let linked = app.handle(req(
            "tasks.waiting_on.add",
            json!({"task_id":dependent,"waiting_on":[task]}),
        ));
        assert_eq!(linked["ok"], true, "{linked}");
        let comment = app.handle(req(
            "tasks.comment",
            json!({"task_id":task,"body":COMMENT_BODY}),
        ));
        assert_eq!(comment["ok"], true, "{comment}");
        let comment = comment["result"]["comment"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let saved = app.handle(req(
            "tasks.review.snapshot",
            json!({"task_id":task,"workspace_id":workspace,"expected_version":0}),
        ));
        assert_eq!(saved["ok"], true, "real review snapshot: {saved}");
        let snapshot = saved["result"]["review"]["snapshots"]
            .as_array()
            .unwrap()
            .last()
            .unwrap();
        let directory = &snapshot["directories"][0];
        assert_eq!(
            directory["status"], "git",
            "saved review must serve an actual blob: {directory}"
        );
        let bytes = std::fs::read(app.project_at(0).repo_path.join("README.md")).unwrap();
        assert!(bytes.len() >= 4);
        let (capture, router) = crate::app::tests::routing::router::captured(
            app,
            "busy qualification read-only router",
        );
        app.test_install_mcp_token(router.clone(), format!("busy-token-{router}"));
        // The ordinary Busy setup clears queued launches before app.shared().
        // Retain this passive real router identity; never start a fake harness.
        Self {
            project,
            workspace: workspace.into(),
            workspace_agent: workspace_agent.into(),
            project_agent,
            task: task.into(),
            dependent: dependent.into(),
            comment,
            snapshot: snapshot["id"].as_str().unwrap().into(),
            directory: directory["id"].as_str().unwrap().into(),
            router,
            router_capture: capture,
            blob_bytes: bytes,
        }
    }

    pub(in super::super) fn ids(&self, owner: &str, agent: &str) -> Ids {
        Ids {
            owner: owner.into(),
            agent: agent.into(),
            workspace: self.workspace.clone(),
            task: self.task.clone(),
            comment: self.comment.clone(),
            snapshot: self.snapshot.clone(),
            directory: self.directory.clone(),
            path: "README.md".into(),
        }
    }

    pub(in super::super) fn frame(&self, agent: &str, action: &BridgeAction) -> Value {
        let endpoint = if action.allowed_on(McpSurface::Router) {
            &self.router
        } else {
            agent
        };
        json!({"task_id":endpoint,"session_token":format!("busy-token-{endpoint}"),"request":action})
    }

    pub(in super::super) fn assert_reply(&self, action: &BridgeAction, reply: &Value) {
        assert_eq!(reply["ok"], true, "{}: {reply}", action.tool_name());
        let value = &reply["result"];
        match action {
            BridgeAction::TrackerGetTask { task_id } => assert_eq!(value["task"]["id"], *task_id),
            BridgeAction::TrackerListTasks { .. } => {
                assert!(contains(&value["tasks"], "id", &self.task))
            }
            BridgeAction::TrackerTaskDependents { .. } => {
                assert!(contains(&value["blocks"], "task_id", &self.dependent))
            }
            BridgeAction::ListHarnesses => {
                assert!(!value["harnesses"].as_array().unwrap().is_empty())
            }
            BridgeAction::ListWorkspaceAgents { .. } => {
                assert_eq!(value["workspace_id"], self.workspace);
                assert!(contains(&value["agents"], "id", &self.workspace_agent));
            }
            BridgeAction::ListProjectAgents => {
                assert_eq!(value["project_id"], self.project);
                assert!(contains(&value["agents"], "id", &self.project_agent));
                assert!(value["agents"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|row| row["id"] == self.project_agent
                        && row["project_role"] == "coordinator"));
            }
            BridgeAction::ListWorkspaces => assert!(contains(
                &value["workspaces"],
                "workspace_id",
                &self.workspace
            )),
            BridgeAction::ListProjects => {
                assert!(contains(&value["projects"], "project_id", &self.project))
            }
            BridgeAction::ListWork => {
                assert!(contains(&value["work"], "task_id", &self.task));
                assert!(!value["work"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|row| row["kind"] == "capture"));
            }
            BridgeAction::SearchConversation { .. } => assert!(value["hits"]
                .as_array()
                .unwrap()
                .iter()
                .any(|hit| hit["excerpt"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("Busy request 0:"))),
            BridgeAction::ReadConversation { .. } => assert!(value["transcript"]
                .as_str()
                .unwrap()
                .contains("Busy request 0:")),
            BridgeAction::TrackerReadComment { .. } => {
                assert_eq!(value["comment_id"], self.comment);
                assert_eq!(value["body"], COMMENT_BODY);
            }
            BridgeAction::TrackerGetReview { .. } => assert!(contains(
                &value["review"]["snapshots"],
                "id",
                &self.snapshot
            )),
            BridgeAction::TrackerReadReview { .. } => {
                let content = value["content_b64"].as_str().unwrap();
                assert_eq!(
                    crate::encoding::b64decode(content).unwrap(),
                    self.blob_bytes
                );
            }
            _ => {}
        }
    }
}

fn contains(rows: &Value, field: &str, expected: &str) -> bool {
    rows.as_array()
        .unwrap()
        .iter()
        .any(|row| row[field] == expected)
}

pub(in super::super) fn seed_project_owner(app: &mut AppState, owner: &str) {
    let project = app.projects.project_id_of(owner).unwrap().to_owned();
    let original = app.runs[owner].agents.primary().unwrap().id.clone();
    let before_count = app.runs[owner]
        .agents
        .primary()
        .unwrap()
        .thread
        .total_item_count();
    assert_eq!(before_count as usize, HISTORY_ITEMS);
    let scratch = crate::app::projects::scratch_dir(&app.state_root, &app.project_at(0).repo_path);
    std::fs::create_dir_all(&scratch).unwrap();
    let mut active = app.take_run(owner).unwrap();
    active.worktree.path = scratch;
    app.finish_run_mutation(owner.to_owned(), active).unwrap();
    let reply = app.handle(req(
        "project.ensure_conversation",
        json!({"project_id":project}),
    ));
    assert_eq!(reply["ok"], true, "{reply}");
    assert_eq!(reply["result"]["run_id"], owner);
    assert_eq!(app.project_pin(owner), Some(original.as_str()));
    assert_eq!(
        app.runs[owner]
            .agents
            .primary()
            .unwrap()
            .thread
            .total_item_count(),
        before_count
    );
    assert_eq!(app.runs[owner].agents.iter().count(), 1);
}

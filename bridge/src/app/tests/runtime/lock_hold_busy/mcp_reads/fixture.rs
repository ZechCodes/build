//! Seed saved task/comment/review witnesses before any timed work starts.
use super::*;

const COMMENT: &str = "focused #741 saved comment witness";

pub(super) struct Fixture {
    workspace: String,
    task: String,
    dependent: String,
    comment: String,
    snapshot: String,
    directory: String,
    router: String,
    blob: Vec<u8>,
}

impl Fixture {
    pub(super) fn example() -> Self {
        Self {
            workspace: "read-workspace".into(),
            task: "read-task".into(),
            dependent: "read-dependent".into(),
            comment: "read-comment".into(),
            snapshot: "read-snapshot".into(),
            directory: "read-directory".into(),
            router: "read-router".into(),
            blob: Vec::new(),
        }
    }

    pub(super) fn seed(app: &mut AppState, workspace: &str, task: &str, dependent: &str) -> Self {
        let linked = app.handle(req(
            "tasks.waiting_on.add",
            json!({"task_id":dependent,"waiting_on":[task]}),
        ));
        assert_eq!(linked["ok"], true, "{linked}");
        let comment = app.handle(req("tasks.comment", json!({"task_id":task,"body":COMMENT})));
        assert_eq!(comment["ok"], true, "{comment}");
        let saved = app.handle(req(
            "tasks.review.snapshot",
            json!({"task_id":task,"workspace_id":workspace,"expected_version":0}),
        ));
        assert_eq!(saved["ok"], true, "{saved}");
        let snapshot = saved["result"]["review"]["snapshots"]
            .as_array()
            .unwrap()
            .last()
            .unwrap();
        let directory = &snapshot["directories"][0];
        assert_eq!(directory["status"], "git", "{directory}");
        let (_, router) =
            crate::app::tests::routing::router::captured(app, "focused busy read router");
        app.test_install_mcp_token(router.clone(), format!("busy-token-{router}"));
        Self {
            workspace: workspace.into(),
            task: task.into(),
            dependent: dependent.into(),
            comment: comment["result"]["comment"]["id"].as_str().unwrap().into(),
            snapshot: snapshot["id"].as_str().unwrap().into(),
            directory: directory["id"].as_str().unwrap().into(),
            router,
            blob: std::fs::read(app.project_at(0).repo_path.join("README.md")).unwrap(),
        }
    }

    pub(super) fn cases(&self) -> Vec<BridgeAction> {
        vec![
            BridgeAction::TrackerTaskDependents {
                task_id: self.task.clone(),
                limit: Some(1),
                cursor: None,
            },
            BridgeAction::SearchConversation {
                query: crate::thread::ConversationQuery {
                    text: Some("Busy request 0:".into()),
                    ..Default::default()
                },
            },
            BridgeAction::TrackerReadReview {
                task_id: self.task.clone(),
                snapshot_id: self.snapshot.clone(),
                directory_id: self.directory.clone(),
                mode: crate::reviews::read::ReviewReadMode::Blob,
                path: Some("README.md".into()),
                paths: Vec::new(),
                patch: false,
                range: Some(crate::body_page::FileRange {
                    offset: 0,
                    bytes: 4,
                    raw: None,
                }),
            },
            BridgeAction::TrackerReadComment {
                comment_id: self.comment.clone(),
            },
            BridgeAction::ListWork,
            BridgeAction::ListWorkspaces,
        ]
    }

    pub(super) fn task_list(&self) -> BridgeAction {
        BridgeAction::TrackerListTasks {
            state: None,
            status: None,
            label: None,
        }
    }

    pub(super) fn frame(&self, agent: &str, action: &BridgeAction) -> Value {
        let endpoint = if action.allowed_on(crate::mcp::McpSurface::Router) {
            &self.router
        } else {
            agent
        };
        json!({"task_id":endpoint,"session_token":format!("busy-token-{endpoint}"),"request":action})
    }

    pub(super) fn assert_reply(&self, action: &BridgeAction, reply: &Value) {
        assert_eq!(reply["ok"], true, "{}: {reply}", action.tool_name());
        let value = &reply["result"];
        match action {
            BridgeAction::TrackerTaskDependents { .. } => {
                assert!(contains(&value["blocks"], "task_id", &self.dependent))
            }
            BridgeAction::SearchConversation { .. } => assert!(value["hits"]
                .as_array()
                .unwrap()
                .iter()
                .any(|hit| hit["excerpt"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("Busy request 0:"))),
            BridgeAction::TrackerReadReview { .. } => assert_eq!(
                crate::encoding::b64decode(value["content_b64"].as_str().unwrap()).unwrap(),
                self.blob
            ),
            BridgeAction::TrackerReadComment { .. } => {
                assert_eq!(value["comment_id"], self.comment);
                assert_eq!(value["body"], COMMENT);
            }
            BridgeAction::ListWork => assert!(contains(&value["work"], "task_id", &self.task)),
            BridgeAction::ListWorkspaces => assert!(contains(
                &value["workspaces"],
                "workspace_id",
                &self.workspace
            )),
            BridgeAction::TrackerListTasks { .. } => {
                assert!(!value["tasks"].as_array().unwrap().is_empty())
            }
            _ => panic!("unexpected focused read: {}", action.tool_name()),
        }
    }
}

fn contains(rows: &Value, field: &str, expected: &str) -> bool {
    rows.as_array()
        .unwrap()
        .iter()
        .any(|row| row[field] == expected)
}

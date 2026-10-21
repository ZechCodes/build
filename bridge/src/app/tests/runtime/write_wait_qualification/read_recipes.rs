//! Native typed requests and saved-value oracles shared by busy and qualification.
//! McpCallers own fresh sockets, issued endpoints, IDs and timing.
use crate::mcp::BridgeAction;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app::tests::runtime) enum McpCaller {
    Coding,
    Router,
}

pub(in crate::app::tests::runtime) struct McpReadInputs {
    pub owner: String,
    pub agent: String,
    pub workspace: String,
    pub task: String,
    pub comment: String,
    pub snapshot: String,
    pub directory: String,
    pub path: String,
    pub search_text: String,
}

/// The method identity used by the actual qualification catalogue and dispatch.
pub(super) fn mcp_read_method(action: &BridgeAction) -> String {
    format!("mcp.control:{}", action.tool_name())
}

pub(in crate::app::tests::runtime) fn mcp_cases(
    ids: &McpReadInputs,
) -> Vec<(McpCaller, BridgeAction)> {
    use McpCaller::{Coding, Router};
    vec![
        (
            Coding,
            BridgeAction::TrackerGetTask {
                task_id: ids.task.clone(),
            },
        ),
        (
            Coding,
            BridgeAction::TrackerListTasks {
                state: None,
                status: None,
                label: None,
            },
        ),
        (
            Coding,
            BridgeAction::TrackerTaskDependents {
                task_id: ids.task.clone(),
                limit: Some(1),
                cursor: None,
            },
        ),
        (Coding, BridgeAction::ListHarnesses),
        (
            Coding,
            BridgeAction::ListWorkspaceAgents {
                workspace_id: ids.workspace.clone(),
            },
        ),
        (Coding, BridgeAction::ListProjectAgents),
        (Coding, BridgeAction::ListWorkspaces),
        (Router, BridgeAction::ListProjects),
        (Router, BridgeAction::ListWork),
        (
            Coding,
            BridgeAction::SearchConversation {
                query: crate::thread::ConversationQuery {
                    text: Some(ids.search_text.clone()),
                    ..Default::default()
                },
            },
        ),
        (
            Router,
            BridgeAction::ReadConversation {
                entity_id: ids.owner.clone(),
                agent_id: Some(ids.agent.clone()),
                limit: 300,
            },
        ),
        (
            Coding,
            BridgeAction::TrackerReadComment {
                comment_id: ids.comment.clone(),
            },
        ),
        (
            Coding,
            BridgeAction::TrackerGetReview {
                task_id: ids.task.clone(),
            },
        ),
        (
            Coding,
            BridgeAction::TrackerReadReview {
                task_id: ids.task.clone(),
                snapshot_id: ids.snapshot.clone(),
                directory_id: ids.directory.clone(),
                mode: crate::reviews::read::ReviewReadMode::Blob,
                path: Some(ids.path.clone()),
                paths: Vec::new(),
                patch: false,
                range: Some(crate::body_page::FileRange {
                    offset: 0,
                    bytes: 4,
                    raw: None,
                }),
            },
        ),
    ]
}

/// These are native seed witnesses, not values returned by this adapter.
pub(in crate::app::tests::runtime) struct McpExpected {
    pub project: String,
    pub workspace: String,
    pub workspace_agent: String,
    pub project_agent: String,
    pub task: String,
    pub dependent: String,
    pub comment: String,
    pub comment_body: String,
    pub snapshot: String,
    pub blob: Vec<u8>,
    pub search_text: String,
    pub harness: String,
}

fn contains(rows: &serde_json::Value, key: &str, expected: &str) -> bool {
    rows.as_array()
        .expect("typed real response array")
        .iter()
        .any(|row| row[key] == expected)
}

/// Run after exact original-ID final socket response, before incrementing the
/// method/form ready or legacy completion counter. No generic `{ok:true}` case.
pub(in crate::app::tests::runtime) fn assert_mcp_reply(
    action: &BridgeAction,
    reply: &serde_json::Value,
    e: &McpExpected,
) {
    assert_eq!(reply["ok"], true, "{}: {reply}", action.tool_name());
    let value = &reply["result"];
    assert!(
        !value.is_null(),
        "{} returned a placeholder",
        action.tool_name()
    );
    assert_ne!(
        value["pending"],
        true,
        "{} returned pending",
        action.tool_name()
    );
    match action {
        BridgeAction::TrackerGetTask { task_id } => {
            assert_eq!(value["task"]["id"], *task_id);
            assert_eq!(value["task"]["project_id"], e.project);
            assert!(value["timeline"].is_array());
        }
        BridgeAction::TrackerListTasks { .. } => assert!(contains(&value["tasks"], "id", &e.task)),
        BridgeAction::TrackerTaskDependents { .. } => {
            assert!(contains(&value["blocks"], "task_id", &e.dependent))
        }
        BridgeAction::ListHarnesses => assert!(contains(&value["harnesses"], "id", &e.harness)),
        BridgeAction::ListWorkspaceAgents { .. } => {
            assert_eq!(value["workspace_id"], e.workspace);
            assert!(contains(&value["agents"], "id", &e.workspace_agent));
        }
        BridgeAction::ListProjectAgents => {
            assert_eq!(value["project_id"], e.project);
            assert!(value["agents"]
                .as_array()
                .unwrap()
                .iter()
                .any(|row| row["id"] == e.project_agent && row["project_role"] == "coordinator"));
        }
        BridgeAction::ListWorkspaces => {
            assert!(contains(&value["workspaces"], "workspace_id", &e.workspace))
        }
        BridgeAction::ListProjects => {
            assert!(contains(&value["projects"], "project_id", &e.project))
        }
        BridgeAction::ListWork => {
            assert!(contains(&value["work"], "task_id", &e.task));
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
                .contains(&e.search_text))),
        BridgeAction::ReadConversation { .. } => assert!(value["transcript"]
            .as_str()
            .unwrap()
            .contains(&e.search_text)),
        BridgeAction::TrackerReadComment { .. } => {
            assert_eq!(value["comment_id"], e.comment);
            assert_eq!(value["body"], e.comment_body);
        }
        BridgeAction::TrackerGetReview { .. } => {
            assert!(contains(&value["review"]["snapshots"], "id", &e.snapshot))
        }
        BridgeAction::TrackerReadReview { .. } => assert_eq!(
            crate::encoding::b64decode(value["content_b64"].as_str().unwrap()).unwrap(),
            e.blob
        ),
        _ => panic!("{} has no reviewed common read oracle", action.tool_name()),
    }
}

mod rpc;
pub(in crate::app::tests::runtime) use rpc::{
    assert_rpc_reply, cases as rpc_cases, ReadOutcome, RpcCase,
};
mod seed;
pub(in crate::app::tests::runtime) use seed::{SeedContext, SeededReadInputs};

mod tests;

mod full_seed;
mod rpc_full;
mod verify;
pub(super) use full_seed::{PreparedReadInputs, ReadyReadInputs};
pub(super) use rpc_full::{Case as FullRpcCase, Outcome as FullReadOutcome};
pub(super) use verify::{collect_native_rpc_reads, verify_native_rpc_reads};

/// Prepare the same private finite PTY executable used by complete read setup.
pub(super) fn prepare_finite_shell(app: &mut crate::app::AppState, root: &std::path::Path) {
    full_seed::prepare_finite_shell(app, root);
}

mod mcp_forms;
pub(super) use mcp_forms::{mcp_method_latency_policy, mcp_recipe_form, LoadedReadPolicy};
pub(super) use rpc_full::Form as FullReadForm;

//! Lifecycle tools executed as authenticated complete MCP socket frames.

use super::{RareFixture, VerifiedStep};
use crate::app::tests::*;
use crate::mcp::{BridgeAction, McpSurface};
use std::collections::BTreeSet;

mod administration;
mod messaging;
mod reclaim;
mod workspaces;

struct Caller {
    owner: String,
    agent: String,
    token: String,
}

struct ProjectFixture {
    _directory: tempfile::TempDir,
    root: PathBuf,
    repo: PathBuf,
    project: String,
    caller: Caller,
    fixture: RareFixture,
}

impl ProjectFixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(directory.path()).unwrap();
        let repo = init_repo_named(&root, "source");
        let mut app = qa_state_before_admission(&repo, &root)
            .with_config(root.join("config.json"))
            .unwrap();
        app.set_projects_dir(root.join("projects"));
        let project = app.project_at(0).id.clone();
        let caller = caller(&mut app, &project);
        Self {
            _directory: directory,
            root,
            repo,
            project,
            caller,
            fixture: RareFixture::new("mcp-lifecycle-project", app),
        }
    }
}

fn caller(app: &mut AppState, project: &str) -> Caller {
    let (owner, agent) = crate::app::tests::project_agent::project_agent(app, project);
    assert_eq!(app.agent_mcp_surface(&owner, &agent), McpSurface::Project);
    let token = format!("qualification-mcp-{}", uuid::Uuid::new_v4());
    app.test_install_mcp_token(agent.clone(), token.clone());
    assert!(matches!(app.addressed_session(agent.clone()),
        Some(crate::app::mcp::AddressedSession::Coding(generation))
            if generation.entity_id == owner && generation.agent_id == agent));
    Caller {
        owner,
        agent,
        token,
    }
}

fn read(fixture: &RareFixture, method: &str, params: Value) -> Value {
    let reply = fixture.state().lock().unwrap().handle(req(method, params));
    assert_eq!(reply["ok"], true, "{method}: {reply}");
    reply["result"].clone()
}

fn project_row(context: &ProjectFixture) -> Value {
    let list = read(&context.fixture, "project.list", json!({}));
    list["projects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["project_id"] == context.project)
        .unwrap()
        .clone()
}

fn workspace_row(context: &ProjectFixture, workspace: &str) -> Value {
    read(
        &context.fixture,
        "workspace.get",
        json!({"workspace_id": workspace}),
    )
}

fn disk_json(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

fn head(path: &Path) -> String {
    git2::Repository::open(path)
        .unwrap()
        .revparse_single("HEAD")
        .unwrap()
        .peel_to_commit()
        .unwrap()
        .id()
        .to_string()
}

const ACTIONS: &[&str] = &[
    "mcp.control:create_workspace",
    "mcp.control:add_workspace_agent",
    "mcp.control:add_project_agent",
    "mcp.control:remove_workspace_agent",
    "mcp.control:message_workspace_agent",
    "mcp.control:delete_workspace",
    "mcp.control:add_project_source",
    "mcp.control:remove_project_source",
    "mcp.control:reclaim_workspace",
    "mcp.control:add_workspace_directory",
    "mcp.control:remove_workspace_directory",
];

pub(super) async fn execute() -> Vec<VerifiedStep> {
    let mut context = ProjectFixture::new();
    let workspace = workspaces::create(&mut context).await;
    administration::sources(&mut context).await;
    workspaces::directories(&mut context, &workspace).await;
    messaging::project_worker(&mut context).await;
    messaging::workspace_worker(&mut context, &workspace).await;
    workspaces::delete(&mut context, &workspace).await;
    let mut steps = context.fixture.finish().await;
    steps.extend(reclaim::execute().await);
    steps
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn authenticated_mcp_lifecycle_tools_have_actual_success_and_effect_proofs() {
    let steps = execute().await;
    let observed: BTreeSet<_> = steps.iter().map(|step| step.method.as_str()).collect();
    assert_eq!(observed, ACTIONS.iter().copied().collect());
}

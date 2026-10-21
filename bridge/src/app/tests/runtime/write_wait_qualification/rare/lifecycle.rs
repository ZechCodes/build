//! Actual encrypted lifecycle requests and independent durable-effect proofs.

use super::{RareFixture, VerifiedStep};
use crate::app::tests::*;
use std::collections::BTreeSet;

mod dispatch;
mod projects;
mod recovery;
mod workspaces;

const METHODS: &[&str] = &[
    "branch.dispatch",
    "branch.finish",
    "project.add_source",
    "project.create",
    "project.delete",
    "project.ensure_conversation",
    "project.init_git",
    "project.remove_source",
    "project.set_compaction",
    "project.set_isolation",
    "project.set_remote",
    "project.sync_source",
    "project.update_source",
    "workspace.add_directory",
    "workspace.create",
    "workspace.delete",
    "workspace.ensure_conversation",
    "workspace.finish",
    "workspace.init_git",
    "workspace.measure_sizes",
    "workspace.reclaim",
    "workspace.remove_directory",
    "workspace.rename",
    "workspace.retry",
    "workspace.set_compaction",
    "workspace.set_locked",
];

pub(super) async fn execute() -> Vec<VerifiedStep> {
    let mut steps = projects::execute().await;
    steps.extend(workspaces::execute().await);
    steps.extend(recovery::execute().await);
    steps.extend(dispatch::execute().await);
    steps
}

fn configured(repo: &Path, root: &Path) -> AppState {
    let mut app = qa_state_before_admission(repo, root)
        .with_config(root.join("config.json"))
        .unwrap();
    app.set_projects_dir(root.join("projects"));
    app
}

fn setup(app: &mut AppState, method: &str, params: Value) -> Value {
    let reply = app.handle(req(method, params));
    assert_eq!(reply["ok"], true, "{method}: {reply}");
    reply["result"].clone()
}

fn read(fixture: &RareFixture, method: &str, params: Value) -> Value {
    setup(&mut fixture.state().lock().unwrap(), method, params)
}

fn project_row(fixture: &RareFixture, id: &str) -> Value {
    let rows = read(fixture, "project.list", json!({}));
    rows["projects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["project_id"] == id)
        .unwrap()
        .clone()
}

fn workspace_row(fixture: &RareFixture, id: &str) -> Value {
    read(fixture, "workspace.get", json!({"workspace_id": id}))
}

fn workspace_setup(fixture: &RareFixture, project: &str, name: &str) -> Value {
    read(
        fixture,
        "workspace.create",
        json!({
            "project_id": project, "name": name, "isolation": "worktree"
        }),
    )
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

fn disk_json(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

fn local_branch(path: &Path, branch: &str) -> bool {
    git2::Repository::open(path)
        .unwrap()
        .find_branch(branch, git2::BranchType::Local)
        .is_ok()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn lifecycle_recipes_execute_twenty_six_successful_mutations() {
    let steps = execute().await;
    let observed: BTreeSet<_> = steps.iter().map(|step| step.method.as_str()).collect();
    assert_eq!(observed, METHODS.iter().copied().collect());
}

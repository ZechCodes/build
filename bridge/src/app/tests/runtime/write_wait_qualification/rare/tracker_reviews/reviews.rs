use super::super::{RareFixture, VerifiedStep};
use crate::app::tests::project_agent::workspace;
use crate::app::tests::tracker::{filed, tracked};
use crate::git_fixture::git_in;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub(super) async fn execute() -> Vec<VerifiedStep> {
    let mut steps = manual().await;
    steps.extend(pull_request().await);
    steps
}

fn saved(fixture: &RareFixture, task: &str) -> Value {
    let app = fixture.state().lock().unwrap();
    serde_json::to_value(
        app.tracker_store()
            .unwrap()
            .load_review(task)
            .unwrap()
            .unwrap(),
    )
    .unwrap()
}

fn latest_snapshot(review: &Value) -> &Value {
    review["snapshots"].as_array().unwrap().last().unwrap()
}

fn head(root: &Path) -> String {
    git2::Repository::open(root)
        .unwrap()
        .head()
        .unwrap()
        .target()
        .unwrap()
        .to_string()
}

fn commit(root: &Path, name: &str) -> String {
    std::fs::write(root.join(name), format!("Qualification {name}\n")).unwrap();
    git_in(root, &["add", name]);
    git_in(root, &["commit", "-m", name]);
    head(root)
}

fn assert_tree_file(root: &Path, reference: &str, name: &str) {
    let repo = git2::Repository::open(root).unwrap();
    let tree = repo
        .find_reference(reference)
        .unwrap()
        .peel_to_commit()
        .unwrap()
        .tree()
        .unwrap();
    let entry = tree.get_path(Path::new(name)).unwrap();
    let blob = repo.find_blob(entry.id()).unwrap();
    assert_eq!(blob.content(), format!("Qualification {name}\n").as_bytes());
}

fn assert_published_status(fixture: &RareFixture, workspace: &str, status: &str) {
    let app = fixture.state().lock().unwrap();
    assert_eq!(
        app.read_view
            .load()
            .get("workspace.get", workspace)
            .unwrap()["active_review"]["status"],
        status
    );
}

async fn manual() -> Vec<VerifiedStep> {
    let root = tempfile::tempdir().unwrap();
    let (_repo_home, mut app, project) = tracked(root.path());
    let workspace_id = workspace(&mut app, &project, "qualification-manual");
    let task = filed(&mut app, &project, "Qualification manual review")["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let directory = app.workspaces.get(&workspace_id).unwrap().directories[0].clone();
    let committed = commit(&directory.path, "manual-review.txt");
    let mut fixture = RareFixture::new("tasks.review", app);
    let proof = fixture
        .call(
            "tasks.review.snapshot",
            json!({"task_id":task,
        "workspace_id":workspace_id,"expected_version":0}),
        )
        .await;
    let review = saved(&fixture, &task);
    assert_eq!(review["version"], 1);
    assert_eq!(
        latest_snapshot(&review)["directories"][0]["head"],
        committed
    );
    assert_eq!(latest_snapshot(&review)["author"]["kind"], "user");
    fixture
        .verify(
            proof,
            json!({"persisted_snapshot":latest_snapshot(&review)}),
        )
        .await;
    act(&mut fixture, &task, &directory.source_path).await;
    complete(&mut fixture, &task).await;
    fixture.finish().await
}

async fn act(fixture: &mut RareFixture, task: &str, source: &Path) {
    let before = saved(fixture, task);
    let snapshot = latest_snapshot(&before);
    let proof = fixture
        .call(
            "tasks.review.act",
            json!({"task_id":task,"expected_version":before["version"],
        "snapshot_id":snapshot["id"],"sources":[{"directory_id":snapshot["directories"][0]["id"],
        "merge":{"branch":"main"}}]}),
        )
        .await;
    let review = saved(fixture, task);
    let action = review["actions"].as_array().unwrap().last().unwrap();
    assert_eq!(action["status"], "succeeded");
    assert_eq!(action["actor"]["kind"], "user");
    assert_tree_file(source, "refs/heads/main", "manual-review.txt");
    fixture
        .verify(
            proof,
            json!({"persisted_action":action,"source_file":"manual-review.txt"}),
        )
        .await;
}

async fn complete(fixture: &mut RareFixture, task: &str) {
    let before = saved(fixture, task);
    let proof = fixture
        .call(
            "tasks.review.complete",
            json!({"task_id":task,"expected_version":before["version"],
        "description":"Verified qualification manual review"}),
        )
        .await;
    let review = saved(fixture, task);
    assert_eq!(review["state"], "completed");
    assert!(review["version"].as_u64().unwrap() > before["version"].as_u64().unwrap());
    let persisted_task = {
        let app = fixture.state().lock().unwrap();
        app.tracker_store()
            .unwrap()
            .load_tracker_task(task)
            .unwrap()
            .unwrap()
    };
    assert_eq!(persisted_task.status, "done");
    fixture
        .verify(
            proof,
            json!({"persisted_review":review,"task_status":persisted_task.status}),
        )
        .await;
}

async fn pull_request() -> Vec<VerifiedStep> {
    let root = tempfile::tempdir().unwrap();
    let (_repo_home, mut app, project) = tracked(root.path());
    let workspace_id = workspace(&mut app, &project, "qualification-pr");
    let directory = app.workspaces.get(&workspace_id).unwrap().directories[0].clone();
    commit(&directory.path, "opened-review.txt");
    let locked = app.handle(crate::app::tests::req(
        "workspace.set_locked",
        json!({"workspace_id":workspace_id,"locked":true}),
    ));
    assert_eq!(locked["ok"], true);
    git_in(&directory.source_path, &["branch", "release"]);
    let mut fixture = RareFixture::new("tasks.review", app);
    let task = open(&mut fixture, &workspace_id).await;
    push(&mut fixture, &task).await;
    update(&mut fixture, &task).await;
    refresh(&mut fixture, &task).await;
    close(&mut fixture, &task, &workspace_id).await;
    reopen(&mut fixture, &task, &workspace_id).await;
    merge(&mut fixture, &task, &workspace_id).await;
    fixture.finish().await
}

async fn open(fixture: &mut RareFixture, workspace: &str) -> String {
    let proof = fixture.call("tasks.review.open", json!({"workspace_id":workspace,
        "request_id":"qualification-pr-open","title":"Qualification PR","description":"Review committed work"})).await;
    let task = proof.reply["result"]["task"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let review = saved(fixture, &task);
    assert_eq!(review["pull_request"]["status"], "open");
    assert_eq!(review["snapshots"].as_array().unwrap().len(), 1);
    let binding = &review["bindings"][0];
    assert_received(
        binding,
        latest_snapshot(&review)["directories"][0]["head"]
            .as_str()
            .unwrap(),
    );
    assert_published_status(fixture, workspace, "open");
    fixture
        .verify(proof, json!({"persisted_open_review":review}))
        .await;
    task
}

fn working(review: &Value) -> PathBuf {
    PathBuf::from(
        review["bindings"][0]["working_repository"]
            .as_str()
            .unwrap(),
    )
}

fn assert_received(binding: &Value, expected: &str) {
    let repo =
        git2::Repository::open_bare(binding["receiving_repository"].as_str().unwrap()).unwrap();
    assert_eq!(
        repo.refname_to_id(binding["receiving_ref"].as_str().unwrap())
            .unwrap()
            .to_string(),
        expected
    );
}

async fn push(fixture: &mut RareFixture, task: &str) {
    let before = saved(fixture, task);
    let committed = commit(&working(&before), "pushed-review.txt");
    let binding = &before["bindings"][0];
    let proof = fixture
        .call(
            "tasks.review.push",
            json!({"task_id":task,"expected_version":before["version"],
        "sources":[{"directory_id":binding["directory_id"],"expected_head":committed,
        "expected_received_head":binding["last_received_head"]}]}),
        )
        .await;
    assert_eq!(proof.reply["result"]["sources"][0]["status"], "published");
    let review = saved(fixture, task);
    assert_eq!(
        latest_snapshot(&review)["directories"][0]["head"],
        committed
    );
    assert!(review["version"].as_u64().unwrap() > before["version"].as_u64().unwrap());
    assert_received(&review["bindings"][0], &committed);
    fixture
        .verify(
            proof,
            json!({"persisted_received_head":committed,"version":review["version"]}),
        )
        .await;
}

async fn update(fixture: &mut RareFixture, task: &str) {
    let before = saved(fixture, task);
    let proof = fixture
        .call(
            "tasks.review.update",
            json!({"task_id":task,"expected_version":before["version"],
        "bases":[{"directory_id":before["bindings"][0]["directory_id"],"branch":"release"}]}),
        )
        .await;
    let review = saved(fixture, task);
    assert_eq!(
        review["bindings"][0]["base_branch_ref"],
        "refs/heads/release"
    );
    assert_eq!(
        latest_snapshot(&review)["publication"]["reason"],
        "base_changed"
    );
    assert!(review["version"].as_u64().unwrap() > before["version"].as_u64().unwrap());
    fixture
        .verify(
            proof,
            json!({"persisted_binding":review["bindings"][0],"snapshot":latest_snapshot(&review)}),
        )
        .await;
}

async fn refresh(fixture: &mut RareFixture, task: &str) {
    let before = saved(fixture, task);
    let working = working(&before);
    let committed = commit(&working, "refreshed-review.txt");
    git_in(&working, &["push"]);
    assert_received(&before["bindings"][0], &committed);
    assert_ne!(
        latest_snapshot(&before)["directories"][0]["head"],
        committed
    );
    let proof = fixture
        .call(
            "tasks.review.refresh",
            json!({"task_id":task,"expected_version":before["version"]}),
        )
        .await;
    let review = saved(fixture, task);
    assert_eq!(
        latest_snapshot(&review)["directories"][0]["head"],
        committed
    );
    assert!(review["version"].as_u64().unwrap() > before["version"].as_u64().unwrap());
    fixture
        .verify(
            proof,
            json!({"reconciled_head":committed,"snapshot":latest_snapshot(&review)}),
        )
        .await;
}

async fn close(fixture: &mut RareFixture, task: &str, workspace: &str) {
    let before = saved(fixture, task);
    let proof = fixture
        .call(
            "tasks.review.close",
            json!({"task_id":task,"expected_version":before["version"],
        "description":"Qualification pause"}),
        )
        .await;
    let review = saved(fixture, task);
    assert_eq!(review["pull_request"]["status"], "closed");
    assert_published_status(fixture, workspace, "closed");
    fixture
        .verify(
            proof,
            json!({"persisted_status":"closed","version":review["version"]}),
        )
        .await;
}

async fn reopen(fixture: &mut RareFixture, task: &str, workspace: &str) {
    let before = saved(fixture, task);
    let proof = fixture
        .call(
            "tasks.review.reopen",
            json!({"task_id":task,"expected_version":before["version"]}),
        )
        .await;
    let review = saved(fixture, task);
    assert_eq!(review["pull_request"]["status"], "open");
    assert!(
        review["snapshots"].as_array().unwrap().len()
            > before["snapshots"].as_array().unwrap().len()
    );
    assert_published_status(fixture, workspace, "open");
    fixture
        .verify(
            proof,
            json!({"persisted_status":"open","version":review["version"]}),
        )
        .await;
}

async fn merge(fixture: &mut RareFixture, task: &str, workspace: &str) {
    let before = saved(fixture, task);
    let binding = &before["bindings"][0];
    let source = Path::new(binding["source_repository"].as_str().unwrap());
    let base_ref = binding["base_branch_ref"].as_str().unwrap();
    let base = git2::Repository::open(source)
        .unwrap()
        .refname_to_id(base_ref)
        .unwrap();
    let proof = fixture.call("tasks.review.merge", json!({"task_id":task,"expected_version":before["version"],
        "snapshot_id":latest_snapshot(&before)["id"],"sources":[{"directory_id":binding["directory_id"],
        "expected_base_head":base.to_string()}]})).await;
    let review = saved(fixture, task);
    assert_eq!(review["pull_request"]["status"], "merged");
    let intents = fixture
        .state()
        .lock()
        .unwrap()
        .tracker_store()
        .unwrap()
        .load_review_merge_intents(task)
        .unwrap();
    assert_eq!(
        intents.last().unwrap().state,
        crate::reviews::model::ReviewMergeState::Succeeded
    );
    assert_tree_file(source, base_ref, "refreshed-review.txt");
    assert_published_status(fixture, workspace, "merged");
    fixture
        .verify(
            proof,
            json!({"persisted_status":"merged","merged_file":"refreshed-review.txt"}),
        )
        .await;
}

#[cfg(test)]
mod controls {
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn rpc_recipe_manual_review_control() {
        crate::git_fixture::environment::isolated_git_test!();
        let steps = super::manual().await;
        let observed = steps
            .iter()
            .map(|step| step.method.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        let expected: std::collections::BTreeSet<_> = [
            "tasks.review.snapshot",
            "tasks.review.act",
            "tasks.review.complete",
        ]
        .into_iter()
        .collect();
        assert_eq!(observed, expected);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn rpc_recipe_pull_request_control() {
        crate::git_fixture::environment::isolated_git_test!();
        let steps = super::pull_request().await;
        let observed = steps
            .iter()
            .map(|step| step.method.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        let expected: std::collections::BTreeSet<_> = [
            "tasks.review.open",
            "tasks.review.push",
            "tasks.review.update",
            "tasks.review.refresh",
            "tasks.review.close",
            "tasks.review.reopen",
            "tasks.review.merge",
        ]
        .into_iter()
        .collect();
        assert_eq!(observed, expected);
    }
}

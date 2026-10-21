use super::{TrackerFixture, VerifiedStep};
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

fn saved(fixture: &TrackerFixture, task: &str) -> Value {
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

fn assert_published_status(fixture: &TrackerFixture, workspace: &str, status: &str) {
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
    let unwatched = app.handle(crate::app::tests::req(
        "tasks.unwatch",
        json!({"task_id":task}),
    ));
    assert_eq!(unwatched["ok"], true);
    let mut fixture = TrackerFixture::new("mcp-tracker-reviews", app);
    let proof = fixture
        .call(
            "snapshot_review",
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
    assert_eq!(latest_snapshot(&review)["author"], fixture.actor());
    fixture
        .verify(
            proof,
            json!({"persisted_snapshot":latest_snapshot(&review)}),
        )
        .await;
    review_reads(&fixture, &task).await;
    review_comment(&mut fixture, &task).await;
    act(&mut fixture, &task, &directory.source_path).await;
    complete(&mut fixture, &task).await;
    fixture.finish().await
}

async fn act(fixture: &mut TrackerFixture, task: &str, source: &Path) {
    let before = saved(fixture, task);
    let snapshot = latest_snapshot(&before);
    let proof = fixture
        .call(
            "act_review",
            json!({"task_id":task,"expected_version":before["version"],
        "snapshot_id":snapshot["id"],"sources":[{"directory_id":snapshot["directories"][0]["id"],
        "merge":{"branch":"main"}}]}),
        )
        .await;
    let review = saved(fixture, task);
    let action = review["actions"].as_array().unwrap().last().unwrap();
    assert_eq!(action["status"], "succeeded");
    assert_eq!(action["actor"], fixture.actor());
    assert_tree_file(source, "refs/heads/main", "manual-review.txt");
    fixture
        .verify(
            proof,
            json!({"persisted_action":action,"source_file":"manual-review.txt"}),
        )
        .await;
}

async fn complete(fixture: &mut TrackerFixture, task: &str) {
    let before = saved(fixture, task);
    let proof = fixture
        .call(
            "complete_review",
            json!({"task_id":task,"expected_version":before["version"],
        "description":"Verified qualification manual review"}),
        )
        .await;
    let review = saved(fixture, task);
    assert_eq!(review["state"], "completed");
    assert_eq!(review["completion"]["actor"], fixture.actor());
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
    let mut fixture = TrackerFixture::new("mcp-tracker-reviews", app);
    let task = open(&mut fixture, &workspace_id).await;
    push(&mut fixture, &task).await;
    update(&mut fixture, &task).await;
    refresh(&mut fixture, &task).await;
    close(&mut fixture, &task, &workspace_id).await;
    reopen(&mut fixture, &task, &workspace_id).await;
    merge(&mut fixture, &task, &workspace_id).await;
    fixture.finish().await
}

async fn open(fixture: &mut TrackerFixture, workspace: &str) -> String {
    let proof = fixture.call("open_review", json!({"workspace_id":workspace,
        "request_id":"qualification-pr-open","title":"Qualification PR","description":"Review committed work"})).await;
    let task = proof.reply["result"]["task"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let review = saved(fixture, &task);
    assert_eq!(review["pull_request"]["status"], "open");
    assert_eq!(review["pull_request"]["creator"], fixture.actor());
    assert_eq!(latest_snapshot(&review)["author"], fixture.actor());
    assert_eq!(review["snapshots"].as_array().unwrap().len(), 1);
    let binding = &review["bindings"][0];
    assert_received(
        binding,
        latest_snapshot(&review)["directories"][0]["head"]
            .as_str()
            .unwrap(),
    );
    assert_published_status(fixture, workspace, "open");
    assert_eq!(latest_snapshot(&review)["author"], fixture.actor());
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

async fn push(fixture: &mut TrackerFixture, task: &str) {
    let before = saved(fixture, task);
    let committed = commit(&working(&before), "pushed-review.txt");
    let binding = &before["bindings"][0];
    let proof = fixture
        .call(
            "push_review",
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
    assert_eq!(latest_snapshot(&review)["author"], fixture.actor());
    fixture
        .verify(
            proof,
            json!({"persisted_received_head":committed,"version":review["version"]}),
        )
        .await;
}

async fn update(fixture: &mut TrackerFixture, task: &str) {
    let before = saved(fixture, task);
    let proof = fixture
        .call(
            "update_review_base",
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
    assert_eq!(latest_snapshot(&review)["author"], fixture.actor());
    assert!(review["version"].as_u64().unwrap() > before["version"].as_u64().unwrap());
    fixture
        .verify(
            proof,
            json!({"persisted_binding":review["bindings"][0],"snapshot":latest_snapshot(&review)}),
        )
        .await;
}

async fn refresh(fixture: &mut TrackerFixture, task: &str) {
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
            "refresh_review",
            json!({"task_id":task,"expected_version":before["version"]}),
        )
        .await;
    let review = saved(fixture, task);
    assert_eq!(
        latest_snapshot(&review)["directories"][0]["head"],
        committed
    );
    assert!(review["version"].as_u64().unwrap() > before["version"].as_u64().unwrap());
    assert_eq!(latest_snapshot(&review)["author"], fixture.actor());
    fixture
        .verify(
            proof,
            json!({"reconciled_head":committed,"snapshot":latest_snapshot(&review)}),
        )
        .await;
}

async fn close(fixture: &mut TrackerFixture, task: &str, workspace: &str) {
    let before = saved(fixture, task);
    let proof = fixture
        .call(
            "close_review",
            json!({"task_id":task,"expected_version":before["version"],
        "description":"Qualification pause"}),
        )
        .await;
    let review = saved(fixture, task);
    assert_eq!(review["pull_request"]["status"], "closed");
    assert_eq!(review["completion"]["actor"], fixture.actor());
    assert_published_status(fixture, workspace, "closed");
    fixture
        .verify(
            proof,
            json!({"persisted_status":"closed","version":review["version"]}),
        )
        .await;
}

async fn reopen(fixture: &mut TrackerFixture, task: &str, workspace: &str) {
    let before = saved(fixture, task);
    let proof = fixture
        .call(
            "reopen_review",
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
    assert_eq!(latest_snapshot(&review)["author"], fixture.actor());
    fixture
        .verify(
            proof,
            json!({"persisted_status":"open","version":review["version"]}),
        )
        .await;
}

async fn merge(fixture: &mut TrackerFixture, task: &str, workspace: &str) {
    let before = saved(fixture, task);
    let binding = &before["bindings"][0];
    let source = Path::new(binding["source_repository"].as_str().unwrap());
    let base_ref = binding["base_branch_ref"].as_str().unwrap();
    let base = git2::Repository::open(source)
        .unwrap()
        .refname_to_id(base_ref)
        .unwrap();
    let proof = fixture.call("merge_review", json!({"task_id":task,"expected_version":before["version"],
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
    assert!(review["actions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|action| action["status"] == "succeeded" && action["actor"] == fixture.actor()));
    assert_published_status(fixture, workspace, "merged");
    fixture
        .verify(
            proof,
            json!({"persisted_status":"merged","merged_file":"refreshed-review.txt"}),
        )
        .await;
}

// These actions take the independent read lane and receive no invented write root.
async fn review_reads(fixture: &TrackerFixture, task: &str) {
    let review = saved(fixture, task);
    let snapshot = latest_snapshot(&review);
    let proof = fixture.call("get_review", json!({"task_id":task})).await;
    // get_review adds live destination choices to the durable review record.
    // Compare the actual stored metadata, rather than requiring those choices
    // to be absent from its enriched projection.
    for field in [
        "task_id",
        "workspace_id",
        "version",
        "state",
        "snapshots",
        "completion",
        "actions",
    ] {
        assert_eq!(
            proof.reply["result"]["review"][field], review[field],
            "review metadata {field}"
        );
    }
    eprintln!(
        "mcp_tracker_read_effect={}",
        json!({"action":"tracker_get_review",
        "task_id":task,"snapshot_id":snapshot["id"],"reply":proof.reply})
    );
    let proof = fixture
        .call(
            "read_review",
            json!({"task_id":task,
        "snapshot_id":snapshot["id"],"directory_id":snapshot["directories"][0]["id"],
        "mode":"blob","path":"manual-review.txt","patch":false}),
        )
        .await;
    assert_eq!(
        proof.reply["result"]["content_b64"],
        crate::encoding::b64encode(b"Qualification manual-review.txt\n")
    );
    assert_eq!(proof.reply["result"]["editable"], false);
    eprintln!(
        "mcp_tracker_read_effect={}",
        json!({"action":"tracker_read_review",
        "task_id":task,"verified_saved_blob":"manual-review.txt","reply":proof.reply})
    );
}

async fn review_comment(fixture: &mut TrackerFixture, task: &str) {
    let before = saved(fixture, task);
    let snapshot = latest_snapshot(&before);
    let anchor = json!({"snapshot_id":snapshot["id"],"directory_id":snapshot["directories"][0]["id"],
        "path":"README.md","side":"new","line":1});
    let opinion = json!({"snapshot_id":snapshot["id"],"verdict":"approve"});
    let (watching, before_comments) = {
        let app = fixture.state().lock().unwrap();
        let store = app.tracker_store().unwrap();
        let task = store.load_tracker_task(task).unwrap().unwrap();
        (
            task.watched,
            store.load_tracker_timeline(&task.id).unwrap().len(),
        )
    };
    assert!(!watching, "setup must permit a real automatic watch effect");
    let proof = fixture
        .call(
            "comment_task",
            json!({"task_id":task,"body":"MCP saved review opinion",
        "anchor":anchor,"opinion":opinion,"track":false,"notify_user":false,"mention_user":true}),
        )
        .await;
    let id = proof.reply["result"]["comment"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (comment, persisted_task, timeline) = {
        let app = fixture.state().lock().unwrap();
        let store = app.tracker_store().unwrap();
        (
            store.load_tracker_comment(&id).unwrap().unwrap(),
            store.load_tracker_task(task).unwrap().unwrap(),
            store.load_tracker_timeline(task).unwrap(),
        )
    };
    assert_eq!(
        serde_json::to_value(&comment.author).unwrap(),
        fixture.actor()
    );
    assert_eq!(comment.body, "MCP saved review opinion");
    assert_eq!(serde_json::to_value(&comment.anchor).unwrap(), anchor);
    assert_eq!(serde_json::to_value(&comment.opinion).unwrap(), opinion);
    assert!(comment.mentions_user);
    assert!(persisted_task.watched);
    assert!(timeline.len() > before_comments);
    assert_eq!(
        crate::app::tracker::unread_since_mark(&persisted_task, &timeline),
        1
    );
    fixture
        .verify(
            proof,
            json!({"persisted_comment":comment,"watched":persisted_task.watched,
        "watch_started_after":persisted_task.watch_started_after,"verified_unread":1}),
        )
        .await;
}

#[cfg(test)]
mod controls {
    fn variants(steps: &[super::VerifiedStep]) -> std::collections::BTreeSet<String> {
        steps
            .iter()
            .map(|step| {
                step.mcp_action.as_ref().unwrap()["action"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect::<std::collections::BTreeSet<_>>()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn mcp_recipe_manual_review_control() {
        crate::git_fixture::environment::isolated_git_test!();
        let steps = super::manual().await;
        assert_eq!(
            variants(&steps),
            [
                "tracker_snapshot_review",
                "tracker_review_comment_task",
                "tracker_act_review",
                "tracker_complete_review"
            ]
            .into_iter()
            .map(str::to_owned)
            .collect::<std::collections::BTreeSet<_>>()
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn mcp_recipe_pull_request_control() {
        crate::git_fixture::environment::isolated_git_test!();
        let steps = super::pull_request().await;
        assert_eq!(
            variants(&steps),
            [
                "tracker_open_review",
                "tracker_push_review",
                "tracker_update_review_base",
                "tracker_refresh_review",
                "tracker_close_review",
                "tracker_reopen_review",
                "tracker_merge_review"
            ]
            .into_iter()
            .map(str::to_owned)
            .collect::<std::collections::BTreeSet<_>>()
        );
    }
}

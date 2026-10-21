use super::{RareFixture, VerifiedStep};
use crate::app::tests::support::{
    add_external_worktree, dictated_agent_tab, planned_run_in_review, primary_agent_id, qa_state,
    wait_for, DictatedSession, SessionLog,
};
use crate::app::{AppState, TabKey};
use crate::git_fixture::init_repo;
use crate::harness::AgentStatus;
use crate::run::RunState;
use crate::store::PersistedRun;
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

pub(super) async fn execute() -> Vec<VerifiedStep> {
    let mut steps = adoption_and_abandon().await;
    steps.extend(review_actions().await);
    steps.extend(message().await);
    steps
}

fn saved(fixture: &RareFixture, id: &str) -> PersistedRun {
    fixture
        .state()
        .lock()
        .unwrap()
        .store
        .as_ref()
        .unwrap()
        .load_all_runs()
        .unwrap()
        .into_iter()
        .find(|run| run.id == id)
        .unwrap()
}

async fn adoption_and_abandon() -> Vec<VerifiedStep> {
    let (root, repo) = init_repo();
    let mut app = qa_state(&repo, root.path());
    let project = app.project_at(0).id.clone();
    add_external_worktree(
        &repo,
        root.path(),
        "qualification-adopt",
        "qualification-adopt",
    );
    let checkout = app
        .scan_external_worktrees_now(&project)
        .unwrap()
        .into_iter()
        .find(|checkout| checkout.branch.as_deref() == Some("qualification-adopt"))
        .unwrap();
    let mut fixture = RareFixture::new("run", app);
    let proof = fixture
        .call(
            "run.adopt",
            json!({"project_id":project,"worktree_id":checkout.id}),
        )
        .await;
    let id = proof.reply["result"]["run_id"].as_str().unwrap().to_owned();
    let persisted = saved(&fixture, &id);
    assert!(persisted.adopted);
    assert_eq!(
        std::fs::canonicalize(&persisted.worktree_path).unwrap(),
        std::fs::canonicalize(&checkout.path).unwrap()
    );
    assert!(fixture.state().lock().unwrap().runs[&id].agents.is_empty());
    fixture
        .verify(
            proof,
            json!({"persisted_run":persisted.id,"owned_checkout":persisted.worktree_path}),
        )
        .await;
    abandon(&mut fixture, &id, &repo).await;
    fixture.finish().await
}

async fn abandon(fixture: &mut RareFixture, id: &str, source: &Path) {
    let before = saved(fixture, id);
    assert!(Path::new(&before.worktree_path).exists());
    let proof = fixture.call("run.abandon", json!({"run_id":id})).await;
    let persisted = saved(fixture, id);
    assert_eq!(persisted.state, RunState::Abandoned);
    assert!(!Path::new(&before.worktree_path).exists());
    assert!(git2::Repository::open(source)
        .unwrap()
        .find_branch(&before.branch, git2::BranchType::Local)
        .is_ok());
    fixture
        .verify(
            proof,
            json!({"persisted_state":"abandoned","removed_checkout":before.worktree_path,
        "retained_branch":before.branch}),
        )
        .await;
}

struct ExistingSession {
    agent: String,
    log: SessionLog,
    key: TabKey,
    instance: String,
}

fn install_session(app: &mut AppState, owner: &str) -> ExistingSession {
    let agent = primary_agent_id(app, owner);
    let checkout = app.runs[owner].worktree.path.clone();
    let choice = app.runs[owner].agents.by_id(&agent).unwrap().choice.clone();
    let log = SessionLog::default();
    let mut tab = dictated_agent_tab(
        &checkout,
        owner,
        &agent,
        DictatedSession::reporting(AgentStatus::Working).recording_into(&log),
    );
    tab.session_instance =
        app.record_agent_session_start(owner, &agent, &checkout, &choice, "qualification-existing");
    let instance = tab.session_instance.as_ref().unwrap().id.clone();
    let key = TabKey::agent(&AppState::canonical_root(&checkout), &agent);
    app.session_registry.test_insert_tab(key.clone(), tab);
    app.session_locator_factory =
        Arc::new(|_, _| panic!("qualification must deliver to seeded existing session"));
    app.delivery_queue.clear_queued();
    ExistingSession {
        agent,
        log,
        key,
        instance,
    }
}

async fn review_actions() -> Vec<VerifiedStep> {
    let (root, repo) = init_repo();
    let mut app = qa_state(&repo, root.path());
    let (_, run) = planned_run_in_review(&mut app, "Qualification review actions");
    let session = install_session(&mut app, &run);
    let mut fixture = RareFixture::new("run", app);
    commit(&mut fixture, &run).await;
    request_changes(&mut fixture, &run, &session).await;
    fixture.finish().await
}

async fn commit(fixture: &mut RareFixture, run: &str) {
    let persisted = saved(fixture, run);
    let repo = git2::Repository::open(&persisted.worktree_path).unwrap();
    let old = repo.head().unwrap().target().unwrap();
    std::fs::write(
        Path::new(&persisted.worktree_path).join("qualification-commit.txt"),
        "Qualification commit\n",
    )
    .unwrap();
    let proof = fixture
        .call("run.git_action", json!({"run_id":run,"action":"commit"}))
        .await;
    let committed = repo.head().unwrap().target().unwrap();
    assert_ne!(committed, old);
    let tree = repo.find_commit(committed).unwrap().tree().unwrap();
    let blob = repo
        .find_blob(
            tree.get_path(Path::new("qualification-commit.txt"))
                .unwrap()
                .id(),
        )
        .unwrap();
    assert_eq!(blob.content(), b"Qualification commit\n");
    assert_eq!(saved(fixture, run).state, RunState::Review);
    fixture.verify(proof, json!({"committed_head":committed.to_string(),"verified_file":"qualification-commit.txt"})).await;
}

fn persisted_message(fixture: &RareFixture, run: &str, agent: &str, body: &str) -> Value {
    let app = fixture.state().lock().unwrap();
    let conversation = app.runs[run].agents.by_id(agent).unwrap().conversation_id();
    let items = app
        .store
        .as_ref()
        .unwrap()
        .thread_items(conversation)
        .unwrap();
    let message = items
        .into_iter()
        .map(|item| serde_json::to_value(item).unwrap())
        .find(|item| item["type"] == "message" && item["data"]["body"] == body)
        .unwrap();
    assert_eq!(message["data"]["role"], "user");
    message
}

async fn delivered_message(
    fixture: &RareFixture,
    run: &str,
    session: &ExistingSession,
    before: usize,
    body: &str,
) -> Value {
    wait_for(Duration::from_secs(10), || {
        let app = fixture.state().lock().unwrap();
        (session.log.turns().len() == before + 1 && app.delivery_queue.is_idle()).then_some(())
    })
    .await
    .expect("the actual queued run turn reached its existing session");
    let message = persisted_message(fixture, run, &session.agent, body);
    assert_eq!(message["data"]["delivery_status"], "sent");
    let turns = session.log.turns();
    assert_eq!(turns.len(), before + 1);
    let turn = &turns[before];
    assert!(
        turn.contains(body),
        "the actual turn contains the accepted body"
    );
    assert!(turn.contains(crate::operation::NATIVE_REVIEWER_MESSAGES_HEADING));
    assert!(turn.contains(message["data"]["id"].as_str().unwrap()));
    assert!(turn.contains(&format!("\"sequence\": {}", message["data"]["sequence"])));
    let app = fixture.state().lock().unwrap();
    let tab = app.session_registry.test_tab(&session.key).unwrap();
    assert_eq!(tab.session_instance.as_ref().unwrap().id, session.instance);
    assert!(tab.last_delivered_at.is_some());
    message
}

async fn request_changes(fixture: &mut RareFixture, run: &str, session: &ExistingSession) {
    let before = session.log.turns().len();
    let body = "Qualification requested changes";
    let proof = fixture
        .call(
            "run.request_changes",
            json!({"run_id":run,"agent_id":session.agent,
        "messages":[{"body":body,"anchor":null}]}),
        )
        .await;
    let message = delivered_message(fixture, run, session, before, body).await;
    assert_eq!(
        saved(fixture, run).state,
        RunState::Review,
        "the scripted QA change reaches review"
    );
    fixture
        .verify(
            proof,
            json!({"persisted_message":message,"session_turns":session.log.turns().len(),
                "session_instance":session.instance}),
        )
        .await;
}

async fn message() -> Vec<VerifiedStep> {
    let (root, repo) = init_repo();
    let mut app = qa_state(&repo, root.path());
    let (_, run) = planned_run_in_review(&mut app, "Qualification live message");
    let mut active = app.take_run(&run).unwrap();
    active.run.state = RunState::Building;
    app.finish_run_mutation(run.clone(), active).unwrap();
    let session = install_session(&mut app, &run);
    let mut fixture = RareFixture::new("run", app);
    let body = "Qualification live instruction";
    let proof = fixture
        .call("run.message", json!({"run_id":run,"message":body}))
        .await;
    let message = delivered_message(&fixture, &run, &session, 0, body).await;
    fixture
        .verify(
            proof,
            json!({"persisted_message":message,"session_turns":session.log.turns().len(),
                "session_instance":session.instance}),
        )
        .await;
    fixture.finish().await
}

#[cfg(test)]
mod controls {
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn rpc_recipe_run_adoption_control() {
        crate::git_fixture::environment::isolated_git_test!();
        let steps = super::adoption_and_abandon().await;
        assert_eq!(
            steps
                .iter()
                .map(|step| step.method.as_str())
                .collect::<Vec<_>>(),
            vec!["run.adopt", "run.abandon"]
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn rpc_recipe_run_review_control() {
        crate::git_fixture::environment::isolated_git_test!();
        let steps = super::review_actions().await;
        assert_eq!(
            steps
                .iter()
                .map(|step| step.method.as_str())
                .collect::<Vec<_>>(),
            vec!["run.git_action", "run.request_changes"]
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn rpc_recipe_run_message_control() {
        crate::git_fixture::environment::isolated_git_test!();
        let steps = super::message().await;
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].method, "run.message");
    }
}

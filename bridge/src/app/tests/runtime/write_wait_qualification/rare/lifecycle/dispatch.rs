use super::super::finite_provider::FiniteProvider;
use super::*;
use std::sync::atomic::Ordering;

struct EndDispatchSessions(Arc<Mutex<AppState>>);

impl Drop for EndDispatchSessions {
    fn drop(&mut self) {
        let sessions: Vec<_> = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .session_registry
            .test_tabs()
            .map(|(_, tab)| Arc::clone(&tab.session))
            .collect();
        for session in sessions {
            session.end();
        }
    }
}

pub(super) async fn execute() -> Vec<VerifiedStep> {
    let (directory, repo) = init_repo();
    let root = std::fs::canonicalize(directory.path()).unwrap();
    let provider = FiniteProvider::install(&root);
    provider.require_enrolled();
    let app = configured(&repo, &root);
    let project = app.project_at(0).id.clone();
    let mut fixture = RareFixture::new("lifecycle-dispatch", app);
    let cleanup = EndDispatchSessions(Arc::clone(fixture.state()));
    let proof = fixture.call("branch.dispatch", json!({
        "project_id": project, "instruction": "Add qualification endpoint", "provider": "claude"
    })).await;
    let result = &proof.reply["result"];
    let owner = result["run_id"].as_str().unwrap().to_string();
    let agent = result["agent_id"].as_str().unwrap().to_string();
    let branch = result["branch"].as_str().unwrap().to_string();
    assert_eq!(branch, "build/add-qualification-endpoint");
    let checkout = {
        let app = fixture.state().lock().unwrap();
        assert!(app.pending_rows.is_empty());
        assert!(app.runs[&owner].agents.by_id(&agent).is_some());
        assert!(app
            .store
            .as_ref()
            .unwrap()
            .load_all_runs()
            .unwrap()
            .iter()
            .any(|run| run.id == owner));
        app.runs[&owner].worktree.path.clone()
    };
    assert!(checkout.is_dir());
    assert!(local_branch(&repo, &branch));
    assert_eq!(
        git2::Repository::open(&checkout)
            .unwrap()
            .head()
            .unwrap()
            .shorthand(),
        Some(branch.as_str())
    );
    let page = read(
        &fixture,
        "thread.page",
        json!({"entity_id": owner, "agent_id": agent, "limit": 50}),
    );
    assert!(page["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["type"] == "message"
            && item["data"]["body"] == "Add qualification endpoint"));
    wait_for_delivery(&fixture, &checkout, &agent).await;
    assert_eq!(provider.opens.load(Ordering::SeqCst), 1);
    assert_eq!(provider.log.turns().len(), 1);
    assert_eq!(
        fixture
            .state()
            .lock()
            .unwrap()
            .session_registry
            .test_counts()
            .claims,
        0
    );
    fixture.verify(proof, json!({"durable_run": owner, "agent": agent, "branch": branch, "checkout": checkout, "head": head(&checkout), "instruction_in_thread": true, "finite_factory_opens": 1, "recorded_send_turns": 1})).await;
    let retirements = fixture
        .state()
        .lock()
        .unwrap()
        .retire_workspace_agents(&checkout);
    for retirement in retirements {
        assert!(
            retirement.wait(std::time::Duration::from_secs(5)),
            "QA session must be reaped before temporary checkout removal"
        );
    }
    assert!(provider.log.ended());
    drop(cleanup);
    fixture.finish().await
}

async fn wait_for_delivery(fixture: &RareFixture, checkout: &Path, agent: &str) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let key = crate::app::TabKey::agent(checkout, agent);
    loop {
        let delivered = fixture
            .state()
            .lock()
            .unwrap()
            .session_registry
            .test_tab(&key)
            .is_some_and(|tab| tab.last_delivered_at.is_some());
        if delivered {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "branch dispatch must deliver its instruction to the finite QA harness"
        );
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn finite_branch_dispatch_has_independent_effect_proof() {
    let steps = execute().await;
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].method, "branch.dispatch");
    assert_eq!(steps[0].effect["finite_factory_opens"], 1);
}

//! Actual capture requests with durable route and finite-session effects.
use super::provider::FiniteProvider;
use super::*;
use crate::mcp::BridgeAction;
use std::time::Duration;

pub(super) async fn execute() -> Vec<VerifiedStep> {
    let (directory, repository) = init_repo();
    let provider = FiniteProvider::install(directory.path());
    provider.require_enrolled();
    let mut app = qa_state(&repository, directory.path());
    app.default_harness = AgentProvider::Claude;
    app.router_choice = Some(ModelChoice {
        provider: AgentProvider::Claude,
        model: None,
        effort: Some("low".into()),
    });
    let project = app.project_at(0).id.clone();
    // A separate still-routing capture is the cancellation precondition.
    let (cancel_id, cancel_agent) = captured(&mut app, "discard this isolated capture");
    let cancel_root = app.router_sessions[&cancel_id].scratch_dir().to_path_buf();
    let mut fixture = RareFixture::new("capture-routing", app);
    let cleanup = EndFixtureSessions(Arc::clone(fixture.state()));

    let created = fixture
        .call(
            "capture.create",
            json!({"text":"route this qualification capture"}),
        )
        .await;
    let id = created.reply["result"]["id"].as_str().unwrap().to_owned();
    wait_for(Duration::from_secs(2), || {
        let app = fixture.state().lock().unwrap();
        let router = app.router_sessions.get(&id)?;
        let root = AppState::canonical_root(router.scratch_dir());
        let tab = app
            .session_registry
            .test_tab(&TabKey::agent(&root, router.agent_id()))?;
        (router.started()
            && tab.session_is_live()
            && app.delivery_queue.is_idle()
            && provider
                .log
                .turns()
                .iter()
                .any(|turn| turn.contains("route this qualification capture")))
        .then_some(())
    })
    .await
    .expect("finite capture router did not publish and consume its turn");
    {
        let app = fixture.state().lock().unwrap();
        let saved = app.store.as_ref().unwrap().load_all_captures().unwrap();
        let saved = saved.iter().find(|capture| capture.id == id).unwrap();
        assert_eq!(saved.text, "route this qualification capture");
        assert!(app.router_sessions.contains_key(&id));
        assert!(!app.delivery_queue.queued().any(|turn| turn.owner == id));
    }
    assert!(provider
        .log
        .turns()
        .iter()
        .any(|turn| turn.contains("route this qualification capture")));
    fixture.verify(created,json!({"capture_id":id,"persisted_text":"route this qualification capture","router_turn_delivered":true})).await;

    {
        let mut app = fixture.state().lock().unwrap();
        app.router_action(
            &id,
            BridgeAction::AskUser {
                question: "which project?".into(),
                options: Vec::new(),
            },
        )
        .unwrap();
        app.settle_router_session(&id);
    }
    let answered = fixture
        .call(
            "capture.answer",
            json!({"capture_id":id,"text":"the bridge project"}),
        )
        .await;
    wait_for(Duration::from_secs(2), || {
        let app = fixture.state().lock().unwrap();
        (app.delivery_queue.is_idle()
            && provider
                .log
                .turns()
                .iter()
                .any(|turn| turn.contains("the bridge project")))
        .then_some(())
    })
    .await
    .expect("finite refired router did not consume its answer");
    assert_eq!(
        answered.reply["result"]["question"]["answer"],
        "the bridge project"
    );
    {
        let app = fixture.state().lock().unwrap();
        let saved = app.store.as_ref().unwrap().load_all_captures().unwrap();
        let saved = saved.iter().find(|capture| capture.id == id).unwrap();
        assert_eq!(
            saved.question.as_ref().unwrap().answer.as_deref(),
            Some("the bridge project")
        );
        assert!(!app.delivery_queue.queued().any(|turn| turn.owner == id));
    }
    assert!(provider
        .log
        .turns()
        .iter()
        .any(|turn| turn.contains("the bridge project")));
    fixture.verify(answered,json!({"capture_id":id,"saved_answer":"the bridge project","refired_router_delivered":true})).await;

    let branch = "build/rare-capture-reroute";
    assert!(!local_branch_exists(&repository, branch).unwrap());
    let rerouted = fixture
        .call(
            "capture.reroute",
            json!({"capture_id":id,"project_id":project,"kind":"branch","branch":branch}),
        )
        .await;
    assert_eq!(rerouted.reply["result"]["routing"]["kind"], "branch");
    assert_eq!(rerouted.reply["result"]["routing"]["target_id"], branch);
    assert!(local_branch_exists(&repository, branch).unwrap());
    let run = {
        let app = fixture.state().lock().unwrap();
        let stored = app.store.as_ref().unwrap().load_all_runs().unwrap();
        stored
            .iter()
            .find(|run| run.branch == branch)
            .unwrap()
            .id
            .clone()
    };
    fixture
        .verify(
            rerouted,
            json!({"capture_id":id,"local_branch_created":branch,"durable_run":run}),
        )
        .await;

    let cancelled = fixture
        .call("capture.cancel", json!({"capture_id":cancel_id}))
        .await;
    assert_eq!(cancelled.reply["result"]["cancelled"], true);
    {
        let app = fixture.state().lock().unwrap();
        assert!(!app.captures.contains_key(&cancel_id));
        assert!(!app.router_sessions.contains_key(&cancel_id));
        assert!(!app
            .session_registry
            .contains(&TabKey::agent(&cancel_root, &cancel_agent)));
        assert!(!app
            .delivery_queue
            .queued()
            .any(|turn| turn.owner == cancel_id));
        assert!(!app
            .store
            .as_ref()
            .unwrap()
            .load_all_captures()
            .unwrap()
            .iter()
            .any(|capture| capture.id == cancel_id));
    }
    let retired = wait_for(Duration::from_secs(5), || {
        let pending = cancel_root
            .parent()
            .unwrap()
            .read_dir()
            .ok()
            .is_some_and(|entries| {
                entries
                    .filter_map(Result::ok)
                    .any(|entry| entry.file_name().to_string_lossy().starts_with(&cancel_id))
            });
        (!cancel_root.exists() && !pending).then_some(())
    })
    .await;
    assert!(
        retired.is_some(),
        "cancelled router scratch retirement did not finish"
    );
    fixture.verify(cancelled,json!({"removed_capture":cancel_id,"router_removed":cancel_agent,"scratch_removed":cancel_root,"store_absent":true})).await;
    provider.status(AgentStatus::Ended { code: Some(0) });
    drop(cleanup);
    let steps = fixture.finish().await;
    drop(provider);
    steps
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn actual_capture_mutations_have_four_delivered_effect_proofs() {
    let steps = execute().await;
    assert_eq!(steps.len(), 4);
    assert!(steps
        .iter()
        .all(|step| step.reply["ok"] == true && step.mcp_action.is_none()));
}

use super::*;
use crate::update::{Release, UpdateBackend, UpdateConfig, UpdateService};
use async_trait::async_trait;

struct ReleaseBackend;

#[async_trait]
impl UpdateBackend for ReleaseBackend {
    async fn latest(&self) -> Result<Release, String> {
        Ok(Release {
            version: "0.3.0".into(),
            tag: "bridge-v0.3.0".into(),
            published_at: Some("2026-09-23T20:00:00Z".into()),
        })
    }

    async fn install(&self, _release: &Release, _attempt_id: &str) -> Result<(), String> {
        Ok(())
    }
}

fn update_fixture(root: &Path, development_build: bool) -> Arc<UpdateService> {
    Arc::new(
        UpdateService::new(
            UpdateConfig {
                status_path: root.join("store/bridge-update-status.json"),
                result_path: root.join("updates/result.json"),
                running_version: "0.2.0".into(),
                platform: "linux-x86_64".into(),
                development_build,
                check_interval: Duration::from_secs(24 * 60 * 60),
            },
            Arc::new(ReleaseBackend),
        )
        .unwrap(),
    )
}

fn pending_turn(root: &Path) -> PendingAgentTurn {
    PendingAgentTurn {
        operation_id: None,
        root: root.to_path_buf(),
        owner: "run-test".into(),
        agent_id: "agent-test".into(),
        conversation_id: "conversation-test".into(),
        model_choice: ModelChoice::default(),
        choice_revision: 1,
        interrupt: false,
        say: None,
        phase: "test",
        wants_catch_up: false,
        survives_refusal: false,
    }
}

#[test]
fn idle_install_counts_queued_settling_and_in_flight_turns() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    assert!(!app.update_has_working_agents());

    app.delivery_queue.enqueue(pending_turn(&repo));
    assert!(app.update_has_working_agents());
    app.delivery_queue.clear_queued();

    app.delivery_queue
        .enqueue_with_next_delivery(pending_turn(&repo));
    assert!(app.update_has_working_agents());
    app.delivery_queue.retain_queued(|_| false);

    let ticket = app.delivery_queue.start(&pending_turn(&repo));
    assert!(app.update_has_working_agents());
    app.delivery_queue.settle(ticket);
    assert!(!app.update_has_working_agents());
}

#[test]
fn idle_install_counts_recorded_work_without_a_live_tab() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let created = app
        .plan_create(&json!({"goal":"Keep the agent working", "dispatch":false}))
        .unwrap();
    let plan_id = created["plan_id"].as_str().unwrap();
    let agent = app
        .plans
        .get_mut(plan_id)
        .unwrap()
        .agents
        .iter_mut()
        .next()
        .unwrap();
    agent.working_since = Some("2026-09-23T20:00:00Z".into());
    assert!(app.update_has_working_agents());
}

async fn update_event(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<crate::carrier::OutboundEnvelope>,
    key: &str,
    state: &str,
) -> Value {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let outbound = tokio::time::timeout_at(deadline, rx.recv())
            .await
            .expect("update event arrives")
            .expect("session remains open");
        let event = SessionSender::decrypt_push(key, &outbound);
        if event["type"] == "bridge.update_status" && event["state"] == state {
            return event;
        }
    }
}

#[tokio::test]
async fn update_status_rpc_push_probe() {
    let (dir, repo) = init_repo();
    let service = update_fixture(dir.path(), false);
    let state = qa_state(&repo, dir.path())
        .with_update_service(service)
        .shared();
    let handler = AppState::handler(state);
    let (legacy, mut legacy_rx, legacy_key) = SessionSender::observable("legacy");
    let (subscribed, mut subscribed_rx, subscribed_key) = SessionSender::observable("subscribed");

    for (sender, params) in [
        (legacy.clone(), json!({})),
        (subscribed.clone(), json!({"changes":"subscriptions"})),
    ] {
        let greeting = handler.call(sender, req("session.hello", params));
        assert_eq!(greeting["ok"], true, "{greeting}");
        assert!(greeting["result"]["events"]
            .as_array()
            .unwrap()
            .contains(&json!("bridge.update_status")));
    }
    let initial_legacy = update_event(&mut legacy_rx, &legacy_key, "idle").await;
    let initial_subscribed = update_event(&mut subscribed_rx, &subscribed_key, "idle").await;
    assert_eq!(initial_legacy, initial_subscribed);
    let initial_reply = handler.call(legacy.clone(), req("bridge.update_status", json!({})));
    assert_eq!(initial_reply["result"]["state"], "idle", "{initial_reply}");

    let accepted = handler.call(legacy, req("bridge.check_update", json!({})));
    assert_eq!(accepted["ok"], true, "{accepted}");
    let legacy_event = update_event(&mut legacy_rx, &legacy_key, "available").await;
    let subscribed_event = update_event(&mut subscribed_rx, &subscribed_key, "available").await;
    assert_eq!(legacy_event, subscribed_event);
    let reply = handler.call(subscribed, req("bridge.update_status", json!({})));
    assert_eq!(reply["ok"], true, "{reply}");
    let mut event_status = legacy_event.clone();
    event_status.as_object_mut().unwrap().remove("type");
    assert_eq!(reply["result"], event_status);
    println!(
        "BRIDGE_UPDATE_WIRE={}",
        json!({"initial_reply":initial_reply,"reply":reply,"event":legacy_event})
    );
}

#[tokio::test]
async fn development_build_can_check_but_cannot_install() {
    let (dir, repo) = init_repo();
    let service = update_fixture(dir.path(), true);
    let state = qa_state(&repo, dir.path())
        .with_update_service(service)
        .shared();
    let handler = AppState::handler(state);
    let check = call(&handler, "bridge.check_update", json!({}));
    assert_eq!(check["ok"], true, "{check}");
    let install = call(&handler, "bridge.install_update", json!({"when":"now"}));
    assert_eq!(install["error_code"], "unavailable", "{install}");
    let invalid = call(&handler, "bridge.install_update", json!({"when":"later"}));
    assert_eq!(invalid["error_code"], "invalid_params", "{invalid}");
}

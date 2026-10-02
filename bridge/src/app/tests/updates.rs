use super::*;
use crate::update::{Release, UpdateBackend, UpdateConfig, UpdateService};
use async_trait::async_trait;
use tokio::sync::Notify;

struct PausedLaunchBackend {
    entered: Arc<Notify>,
    release: Arc<Notify>,
}

#[async_trait]
impl UpdateBackend for PausedLaunchBackend {
    async fn latest(&self) -> Result<Release, String> {
        Ok(Release {
            version: "0.3.0".into(),
            tag: "bridge-v0.3.0".into(),
            published_at: None,
        })
    }

    async fn install(&self, release: &Release, attempt_id: &str) -> Result<(), String> {
        self.launch(release, attempt_id).await
    }

    async fn launch(&self, _release: &Release, _attempt_id: &str) -> Result<(), String> {
        self.entered.notify_one();
        self.release.notified().await;
        Err("helper launch failed before ownership".into())
    }
}

#[tokio::test]
async fn idle_handoff_blocks_real_rpc_until_failed_launch_reopens_admission() {
    if !crate::git_fixture::environment::GitEnvironment::unsigned().run_test() {
        return;
    }
    let (dir, repo) = init_repo();
    let backend = Arc::new(PausedLaunchBackend {
        entered: Arc::new(Notify::new()),
        release: Arc::new(Notify::new()),
    });
    let service = Arc::new(
        UpdateService::new(
            UpdateConfig {
                status_path: dir.path().join("store/bridge-update-status.json"),
                result_path: dir.path().join("updates/result.json"),
                running_version: "0.2.0".into(),
                platform: "linux-x86_64".into(),
                development_build: false,
                replaceable_development_build: false,
                running_from_cargo_target: false,
                check_interval: Duration::from_secs(24 * 60 * 60),
            },
            backend.clone(),
        )
        .unwrap(),
    );
    let state = qa_state(&repo, dir.path())
        .with_update_service(service.clone())
        .shared();
    let handler = AppState::handler(state);
    service.check().await.unwrap();
    service
        .install(crate::update::InstallWhen::Idle, true)
        .await
        .unwrap();
    let ticking = tokio::spawn({
        let service = service.clone();
        async move { service.tick(false).await }
    });
    backend.entered.notified().await;

    let parent = dir.path().join("new-projects");
    let request = json!({ "name": "during-handoff", "parent": parent });
    let (sender, _, _) = SessionSender::observable("update-handoff");
    let blocked = handler.call(sender.clone(), req("project.create", request.clone()));
    assert_eq!(blocked["error_code"], "busy", "{blocked}");
    assert!(!parent.join("during-handoff").exists());

    backend.release.notify_one();
    assert!(ticking.await.unwrap().is_err());
    let admitted = handler.call(sender, req("project.create", request));
    assert_eq!(admitted["ok"], true, "{admitted}");
}

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
    update_fixture_replaceable(root, development_build, false)
}

fn update_fixture_replaceable(
    root: &Path,
    development_build: bool,
    replaceable_development_build: bool,
) -> Arc<UpdateService> {
    Arc::new(
        UpdateService::new(
            UpdateConfig {
                status_path: root.join("store/bridge-update-status.json"),
                result_path: root.join("updates/result.json"),
                running_version: "0.2.0".into(),
                platform: "linux-x86_64".into(),
                development_build,
                replaceable_development_build,
                running_from_cargo_target: false,
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
    let plan_id = file_legacy_task(&mut app, "Keep the agent working");
    let agent = app
        .plans
        .get_mut(&plan_id)
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
    // Confirming does not help a build the service does not run.
    let confirmed = call(
        &handler,
        "bridge.install_update",
        json!({"when":"now","replace_development_build":true}),
    );
    assert_eq!(confirmed["error_code"], "unavailable", "{confirmed}");
    let invalid = call(&handler, "bridge.install_update", json!({"when":"later"}));
    assert_eq!(invalid["error_code"], "invalid_params", "{invalid}");
}

async fn wait_for_release(service: &UpdateService) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while service.status().latest_release.is_none() {
        assert!(tokio::time::Instant::now() < deadline, "check finishes");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn replacing_a_development_build_needs_the_confirmation_on_the_request() {
    let (dir, repo) = init_repo();
    let service = update_fixture_replaceable(dir.path(), true, true);
    let state = qa_state(&repo, dir.path())
        .with_update_service(service.clone())
        .shared();
    let handler = AppState::handler(state);
    let check = call(&handler, "bridge.check_update", json!({}));
    assert_eq!(check["ok"], true, "{check}");
    wait_for_release(&service).await;
    let status = call(&handler, "bridge.update_status", json!({}));
    assert_eq!(status["result"]["can_install"], false, "{status}");
    assert_eq!(
        status["result"]["can_replace_development_build"], true,
        "{status}"
    );
    let unconfirmed = call(&handler, "bridge.install_update", json!({"when":"now"}));
    assert_eq!(unconfirmed["error_code"], "conflict", "{unconfirmed}");
    let declined = call(
        &handler,
        "bridge.install_update",
        json!({"when":"now","replace_development_build":false}),
    );
    assert_eq!(declined["error_code"], "conflict", "{declined}");
    let confirmed = call(
        &handler,
        "bridge.install_update",
        json!({"when":"now","replace_development_build":true}),
    );
    assert_eq!(confirmed["ok"], true, "{confirmed}");
    assert_eq!(confirmed["result"]["state"], "installing", "{confirmed}");
}

use super::*;
use crate::harness::AgentStatus;

async fn scheduled(updates: &UpdateService) {
    updates.check().await.unwrap();
    assert_eq!(
        updates
            .install(crate::update::InstallWhen::Idle, true)
            .await
            .unwrap()
            .state,
        UpdateState::ScheduledWhenIdle
    );
}

async fn eventually_installs(updates: &UpdateService) {
    let deadline = tokio::time::Instant::now() + WATCHDOG;
    loop {
        let status = updates.tick(false).await.unwrap();
        if status.state == UpdateState::Installing {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "last admitted work must release: {status:?}"
        );
        tokio::task::yield_now().await;
    }
}

async fn deferred_request_keeps_admission(fail: bool, cancel: bool) {
    let (dir, repo) = init_repo();
    let updates = update_fixture(dir.path(), false);
    scheduled(&updates).await;
    let app = qa_state(&repo, dir.path()).with_update_service(Arc::clone(&updates));
    let (state, handler, _) = passive_handler(app);
    let (entered, arrived) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let weak = Arc::downgrade(&state);
    state.lock().unwrap().deferred_work = Some(DeferredWork::External(Box::new(move || {
        let unlocked = weak.upgrade().unwrap().try_lock().is_ok();
        let _ = entered.send(unlocked);
        released
            .recv_timeout(WATCHDOG)
            .map_err(|error| error.to_string())?;
        if fail {
            Err("controlled external refusal".into())
        } else {
            let state = weak.upgrade().unwrap();
            let app = state.lock().unwrap();
            Ok(json!({"user_session":app.user_session_json()}))
        }
    })));
    let request = tokio::task::spawn_blocking(move || {
        handler.call(
            SessionSender::detached("deferred"),
            req("user.present", json!({})),
        )
    });
    // This test has its own runtime worker; the external callback is blocking
    // work. Always release and join it before asserting the observations.
    let arrival = arrived.recv_timeout(WATCHDOG);
    if cancel {
        request.abort(); // Already-running blocking work must retain its lease.
    }
    let while_running = updates.tick(false).await;
    let gate_open = updates.admission().try_enter().is_some();
    let released = release.send(());
    let answer = request.await;
    eventually_installs(&updates).await;
    assert!(
        arrival.unwrap(),
        "external stage must release the app guard"
    );
    released.unwrap();
    assert_eq!(while_running.unwrap().state, UpdateState::ScheduledWhenIdle);
    assert!(
        gate_open,
        "scheduled work permits other requests until the lease is released"
    );
    let answer = answer.expect("aborting running blocking work cannot discard its completion");
    assert_eq!(answer["ok"], !fail, "{answer}");
    if fail {
        assert_eq!(answer["error"], "controlled external refusal", "{answer}");
    } else {
        let app = state.lock().unwrap();
        assert_eq!(
            answer["result"]["user_session"]["last_activity_ms"],
            app.user_session.last_activity_ms.unwrap()
        );
        assert_eq!(
            app.store.as_ref().unwrap().load_user_session().unwrap(),
            Some(app.user_session)
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn routed_admission_spans_successful_external_stage_and_reply() {
    deferred_request_keeps_admission(false, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn routed_admission_releases_after_external_refusal() {
    deferred_request_keeps_admission(true, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelling_running_rpc_does_not_release_its_external_stage_lease() {
    deferred_request_keeps_admission(false, true).await;
}

struct PausedSession {
    entered: mpsc::Sender<String>,
    release: Mutex<mpsc::Receiver<()>>,
}

impl AgentSession for PausedSession {
    fn send_turn(&self, turn: &Turn) -> Result<(), HarnessError> {
        let _ = self.entered.send(turn.text.clone());
        self.release
            .lock()
            .unwrap()
            .recv_timeout(WATCHDOG)
            .map_err(|error| HarnessError::Session(error.to_string()))
    }
    fn status(&self) -> AgentStatus {
        AgentStatus::Waiting
    }
    fn quiet_for(&self) -> Duration {
        Duration::ZERO
    }
    fn exited_within(&self, _: Duration) -> bool {
        false
    }
    fn end(&self) {}
    fn backdate_last_output(&self, _: Duration) {}
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn routed_post_worker_retains_admission_after_reply_until_delivery_finishes() {
    let (dir, repo) = init_repo();
    let updates = update_fixture(dir.path(), false);
    scheduled(&updates).await;
    let mut app = qa_state(&repo, dir.path()).with_update_service(Arc::clone(&updates));
    let owner = adopted_run(&mut app, &repo, dir.path(), "admitted-delivery");
    let agent = primary_agent_id(&app, &owner);
    let root = app.runs[&owner].worktree.path.clone();
    let (entered, arrived) = mpsc::channel();
    let (release, released) = mpsc::channel();
    insert_agent_tab(
        &mut app,
        &root,
        &owner,
        &agent,
        PausedSession {
            entered,
            release: Mutex::new(released),
        },
    );
    app.delivery_queue.clear_queued();
    let (state, handler, _) = passive_handler(app);
    let params = json!({"entity_id":owner,"agent_id":agent,"conversation_id":agent,
        "body":"lease survives the reply"});
    let answer = handler.call(
        SessionSender::detached("posting"),
        req("thread.post", params),
    );
    let arrival = arrived.recv_timeout(WATCHDOG);
    // false deliberately ignores working-agent/queue state: only the admitted
    // background worker can keep this otherwise idle handoff scheduled.
    let after_reply = updates.tick(false).await;
    let stored = state
        .lock()
        .unwrap()
        .store
        .as_ref()
        .unwrap()
        .thread_items(&agent);
    let released = release.send(());
    wait_for_deliveries(&state).await;
    eventually_installs(&updates).await;
    released.unwrap();
    assert_eq!(answer["ok"], true, "{answer}");
    assert_eq!(after_reply.unwrap().state, UpdateState::ScheduledWhenIdle);
    assert_native_delivery(arrival.unwrap(), stored.unwrap());
    let app = state.lock().unwrap();
    assert!(
        app.session_registry
            .test_tab(&TabKey::agent(&root, &agent))
            .unwrap()
            .last_delivered_at
            .is_some(),
        "the real worker settled its warm delivery"
    );
}

/// The warm turn carries its native immutable message envelope, not only the
/// fixed nudge. Compare its immutable identity/text with the committed message;
/// delivery receipts legitimately advance after the payload has been frozen.
fn assert_native_delivery(prompt: String, stored: Vec<crate::thread::ThreadItem>) {
    assert!(
        prompt.contains(crate::app::NEW_THREAD_MESSAGES_PROMPT),
        "{prompt}"
    );
    let (_, payload) = prompt
        .split_once(crate::operation::NATIVE_REVIEWER_MESSAGES_HEADING)
        .expect("warm delivery includes the real native envelope");
    let accepted: Vec<Value> = serde_json::from_str(payload.trim()).unwrap();
    assert_eq!(accepted.len(), 1);
    assert_eq!(accepted[0]["body"], "lease survives the reply");
    assert_eq!(accepted[0]["role"], "user");
    let persisted: Vec<Value> = stored
        .iter()
        .filter_map(|item| match item {
            crate::thread::ThreadItem::Message(message)
                if message.body == "lease survives the reply" =>
            {
                Some(serde_json::to_value(message).unwrap())
            }
            _ => None,
        })
        .collect();
    assert_eq!(persisted.len(), 1);
    assert_eq!(persisted[0]["delivery_status"], "submitted");
    for field in ["id", "sequence", "role", "body", "created_at"] {
        assert!(
            !accepted[0][field].is_null(),
            "native message must include {field}"
        );
        assert_eq!(accepted[0][field], persisted[0][field], "{field}");
    }
}

//! Real stream-json reader to the offline failure pump, using only a fake child.
use super::project_agent::{items, project_agent, workspace};
use super::tracker::{filed, tracked};
use super::*;
use crate::harness::adk::AdkSession;

#[tokio::test]
async fn adk_prompt_too_long_reaches_attention_task_and_coordinator_without_a_client() {
    let temp = tempfile::tempdir().unwrap();
    let (_home, mut app, project) = tracked(temp.path());
    let (coordinator_owner, coordinator) = project_agent(&mut app, &project);
    let ws = workspace(&mut app, &project, "offline ADK overflow");
    let task = filed(&mut app, &project, "Over-window ADK turn");
    let task_id = task["id"].as_str().unwrap().to_string();
    let assigned = app.handle(req(
        "tasks.assign",
        json!({"task_id":task_id, "assignee":{"kind":"new_agent", "workspace_id":ws}}),
    ));
    assert_eq!(assigned["ok"], true, "{assigned}");
    let dispatch = &assigned["result"]["dispatch"];
    let owner = dispatch["entity_id"].as_str().unwrap().to_string();
    let agent = dispatch["agent_id"].as_str().unwrap().to_string();
    let operation = dispatch["operation_id"].as_str().unwrap().to_string();
    let watched = app.handle(req(
        "conversation.watch",
        json!({"entity_id": owner, "agent_id": agent}),
    ));
    assert_eq!(watched["ok"], true, "{watched}");
    // The normal delivery path has consumed the assignment's task authority.
    super::tracker_activity::consume_assignment_turns(&mut app);
    let input = temp.path().join("adk-input");
    let spec = HarnessSpec::new("sh").arg("-c").arg(r#"
        printf '%s\n' '{"type":"system","subtype":"init","session_id":"synthetic-overflow","model":"synthetic"}'
        while IFS= read -r line; do
            printf '%s\n' "$line" >> "$1"
            printf '%s\n' '{"type":"assistant","error":"invalid_request","message":{"content":[{"type":"text","text":"Prompt is too long"}]}}'
            printf '%s\n' '{"type":"result","subtype":"success","is_error":true,"duration_ms":40,"result":"Prompt is too long"}'
        done
    "#).arg("adk-overflow").arg(input.to_string_lossy());
    let choice = ModelChoice {
        provider: AgentProvider::ClaudeAdk,
        ..ModelChoice::default()
    };
    let (session, activity) = AdkSession::spawn(&spec, None, &choice).unwrap();
    wait_for(Duration::from_secs(5), || {
        (session.status() == AgentStatus::Waiting).then_some(())
    })
    .await
    .expect("the fake child initializes");
    let root = app.runs[&owner].worktree.path.clone();
    let key = insert_agent_tab(&mut app, &root, &owner, &agent, session);
    let coordinator_root = app.runs[&coordinator_owner].worktree.path.clone();
    let coordinator_log = SessionLog::default();
    insert_agent_tab(
        &mut app,
        &coordinator_root,
        &coordinator_owner,
        &coordinator,
        DictatedSession::reporting(AgentStatus::Waiting).recording_into(&coordinator_log),
    );
    let worker = Arc::clone(&app.session_registry.test_tab(&key).unwrap().session);
    let state = app.shared();
    // No client frame or synthetic TurnFailed: the real ADK reader owns it.
    spawn_activity_pump(&state, key, Some(activity));
    worker
        .send_turn(&Turn::new("wake already stuck agent").with_operation_id(operation))
        .unwrap();
    wait_for(Duration::from_secs(5), || {
        let mut app = state.lock().unwrap();
        items(&mut app, &owner, &agent)
            .iter()
            .any(|row| row["data"]["outcome"] == "failed")
            .then_some(())
    })
    .await
    .expect("the surviving refused ADK turn becomes visible attention");
    wait_for(
        crate::app::runtime::delivery::queue::NOTICE_SETTLE_WINDOW + Duration::from_secs(5),
        || (!coordinator_log.turns().is_empty()).then_some(()),
    )
    .await
    .expect("the coordinator receives the failure without a client");
    assert_eq!(
        worker.status(),
        AgentStatus::Waiting,
        "provider refusal keeps the child alive"
    );
    let mut app = state.lock().unwrap();
    let reports = items(&mut app, &owner, &agent);
    assert_eq!(
        reports
            .iter()
            .filter(|row| row["data"]["outcome"] == "failed")
            .count(),
        1
    );
    assert!(
        !reports.iter().any(|row| row["data"]["from_build"] == true),
        "no self-directed failure notice"
    );
    let thread = app.agent_conversation(&owner, Some(&agent)).unwrap();
    let unread = app.agent_unread(
        &owner,
        app.runs[&owner].agents.by_id(&agent).unwrap(),
        thread,
    );
    assert_eq!(unread.reason, Some("run_failed"));
    assert!(unread.count > 0, "the Inbox has unread attention");
    let entry = super::board::attention::board_entry(&mut app, &owner);
    assert_eq!(entry["needs_attention"], true, "{entry}");
    assert_eq!(entry["unread_reason"], "run_failed", "{entry}");
    let read = app.handle(req("tasks.get", json!({"task_id":task_id})));
    let comments: Vec<_> = read["result"]["timeline"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["type"] == "comment")
        .collect();
    assert_eq!(comments.len(), 1, "{read}");
    assert!(comments[0]["body"]
        .as_str()
        .unwrap()
        .contains("Prompt is too long"));
    let notices = items(&mut app, &coordinator_owner, &coordinator);
    assert_eq!(
        notices
            .iter()
            .filter(|row| row["data"]["from_build"] == true
                && row["data"]["body"]
                    .as_str()
                    .is_some_and(|body| body.contains("Prompt is too long")))
            .count(),
        1
    );
    assert_eq!(coordinator_log.turns().len(), 1);
    assert_eq!(
        std::fs::read_to_string(input).unwrap().lines().count(),
        1,
        "the failed agent is never woken to read its own failure"
    );
    worker.end();
}

#[tokio::test]
async fn managed_compaction_block_fails_due_wakes_once_and_allows_below_threshold_input() {
    managed_block_scenario(ManagedBlockScenario::Warm).await;
}

#[tokio::test]
async fn managed_block_refuses_first_due_wake_before_adk_init_without_promoting_active_policy() {
    managed_block_scenario(ManagedBlockScenario::BeforeInit).await;
}

#[tokio::test]
async fn managed_block_deduplicates_a_failure_outside_the_restored_resident_tail() {
    managed_block_scenario(ManagedBlockScenario::HistoricalTail).await;
}

#[tokio::test]
async fn managed_block_repeat_guard_does_not_wait_for_store_writer_or_read_pool() {
    managed_block_scenario(ManagedBlockScenario::BusyStore).await;
}

#[tokio::test]
async fn cold_managed_fallback_compacts_saved_context_before_the_first_wake() {
    for above in [-1_i64, 0, 1] {
        let fixture =
            ManagedBlockFixture::new(ManagedBlockScenario::BeforeInitCommandAvailable).await;
        let turn = {
            let mut app = fixture.state.lock().unwrap();
            app.record_agent_turn_context(
                &fixture.owner,
                &fixture.agent,
                crate::harness::TurnContext {
                    context_tokens: fixture.threshold.checked_add_signed(above).unwrap(),
                    cache_read_tokens: 0,
                },
            );
            let queued = app.delivery_queue.queued_last().unwrap();
            PendingAgentTurn {
                operation_id: queued.operation_id.clone(),
                root: queued.root.clone(),
                owner: queued.owner.clone(),
                agent_id: queued.agent_id.clone(),
                conversation_id: queued.conversation_id.clone(),
                model_choice: queued.model_choice.clone(),
                choice_revision: queued.choice_revision,
                interrupt: false,
                phase: "cold-fallback",
                say: None,
                wants_catch_up: false,
                survives_refusal: false,
            }
        };
        assert!(fixture.worker.compaction_policy().is_none());
        let due = above >= 0;
        assert_eq!(
            crate::app::runtime::delivery::compaction::compact_before_turn(
                &fixture.state,
                &turn,
                Spawned::Fresh,
                "first wake",
                &a_frame(&fixture.state),
            ),
            due,
            "only a due cold resumed managed fallback compacts before input"
        );
        if due {
            assert_eq!(
                fixture.state.lock().unwrap().runs[&fixture.owner]
                    .agents
                    .by_id(&fixture.agent)
                    .unwrap()
                    .last_context_tokens,
                None
            );
            wait_for(Duration::from_secs(5), || {
                (fixture.worker.status() == AgentStatus::Waiting).then_some(())
            })
            .await
            .expect("cold compact completes");
        }
        let expected = if due { 2 } else { 1 };
        deliver_pending_agent_turns(&fixture.state);
        let sent = wait_for(Duration::from_secs(5), || {
            let text = std::fs::read_to_string(&fixture.input).ok()?;
            (text.lines().count() == expected).then_some(text)
        })
        .await
        .expect("the queued waking assignment follows compaction");
        let turns: Vec<serde_json::Value> = sent
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        if due {
            assert_eq!(turns[0]["message"]["content"][0]["text"], "/compact");
        }
        assert_ne!(
            turns.last().unwrap()["message"]["content"][0]["text"],
            "/compact"
        );
        deliver_pending_agent_turns(&fixture.state);
        assert_eq!(
            std::fs::read_to_string(&fixture.input)
                .unwrap()
                .lines()
                .count(),
            expected
        );
        fixture.worker.end();
    }
}

#[derive(Clone, Copy)]
enum ManagedBlockScenario {
    Warm,
    BeforeInit,
    HistoricalTail,
    BusyStore,
    BeforeInitCommandAvailable,
}

async fn managed_block_scenario(scenario: ManagedBlockScenario) {
    let fixture = ManagedBlockFixture::new(scenario).await;
    fixture.assert_first_refusal();
    fixture.assert_repeated_refusal(scenario).await;
    fixture.queue_below_threshold();
    fixture.complete_below_threshold().await;
    fixture.assert_reblocked_cycle().await;
    fixture.worker.end();
}

struct ManagedBlockFixture {
    _temp: tempfile::TempDir,
    _home: tempfile::TempDir,
    state: Arc<Mutex<AppState>>,
    owner: String,
    agent: String,
    task_id: String,
    operation: String,
    input: PathBuf,
    key: TabKey,
    worker: Arc<dyn AgentSession>,
    coordinator_log: SessionLog,
    threshold: u64,
    cold: bool,
}

impl ManagedBlockFixture {
    async fn new(scenario: ManagedBlockScenario) -> Self {
        use crate::harness::compaction::{
            CompactionEnforcement, CompactionLaunchIdentity, PolicySession,
        };
        let temp = tempfile::tempdir().unwrap();
        let (_home, mut app, project) = tracked(temp.path());
        let (coordinator_owner, coordinator) = project_agent(&mut app, &project);
        let ws = workspace(&mut app, &project, "managed compaction blocked");
        let ensured = app.handle(req(
            "workspace.ensure_conversation",
            json!({"workspace_id":ws}),
        ));
        assert_eq!(ensured["ok"], true, "{ensured}");
        let owner = ensured["result"]["run_id"].as_str().unwrap().to_string();
        let added = app.handle(req("agent.add", json!({"entity_id":owner})));
        assert_eq!(added["ok"], true, "{added}");
        let agent = added["result"]["agent"]["id"].as_str().unwrap().to_string();
        let choice = ModelChoice {
            provider: AgentProvider::ClaudeAdk,
            ..ModelChoice::default()
        };
        app.set_agent_model_choice(&owner, &agent, choice.clone())
            .unwrap();
        let task = filed(&mut app, &project, "First wake with compaction blocked");
        let task_id = task["id"].as_str().unwrap().to_string();
        let assigned = app.handle(req(
            "tasks.assign",
            json!({
                "task_id":task_id, "assignee":{"kind":"agent", "agent_id":agent},
            }),
        ));
        assert_eq!(assigned["ok"], true, "{assigned}");
        let operation = assigned["result"]["dispatch"]["operation_id"]
            .as_str()
            .unwrap()
            .to_string();
        let input = temp.path().join("managed-blocked-input");
        let cold = matches!(
            scenario,
            ManagedBlockScenario::BeforeInit | ManagedBlockScenario::BeforeInitCommandAvailable
        );
        let session = open_managed_child(&input, &choice, cold).await;
        let mut policy = crate::harness::harness_for(choice.provider)
            .prepare_compaction_policy(
                app.resolve_compaction_policy(&owner, &agent).unwrap(),
                choice,
                None,
                CompactionLaunchIdentity::new(
                    app.agent_conversation(&owner, Some(&agent))
                        .unwrap()
                        .id
                        .clone(),
                ),
            )
            .unwrap();
        // Captured managed fallback; only DISABLE_COMPACT also blocks the command.
        policy.enforcement = CompactionEnforcement::BetweenTurn;
        policy.native_settings = None;
        policy.translated_trigger = None;
        policy.capabilities.between_turn_compact =
            matches!(scenario, ManagedBlockScenario::BeforeInitCommandAvailable);
        policy.reason = Some(if policy.capabilities.between_turn_compact {
            "managed policy DISABLE_AUTO_COMPACT disables native compaction".into()
        } else {
            "managed policy DISABLE_COMPACT disables native and manual compaction".into()
        });
        let observation = app.agent_compaction_observation(&owner, &agent, Some(&policy), None);
        assert_eq!(observation["enforcement"], "between_turn");
        assert_eq!(observation["reason"], policy.reason.as_deref().unwrap());
        let threshold = policy.requested.tokens;
        app.record_agent_turn_context(
            &owner,
            &agent,
            crate::harness::TurnContext {
                context_tokens: threshold,
                cache_read_tokens: 0,
            },
        );
        let root = app.runs[&owner].worktree.path.clone();
        let key = insert_agent_tab(&mut app, &root, &owner, &agent, session);
        let tab = app.session_registry.test_tab_mut(&key).unwrap();
        tab.session = PolicySession::attach(Arc::clone(&tab.session), policy);
        let worker = Arc::clone(&tab.session);
        assert_managed_opening_state(worker.as_ref(), cold);
        let coordinator_root = app.runs[&coordinator_owner].worktree.path.clone();
        let coordinator_log = SessionLog::default();
        insert_agent_tab(
            &mut app,
            &coordinator_root,
            &coordinator_owner,
            &coordinator,
            DictatedSession::reporting(AgentStatus::Waiting).recording_into(&coordinator_log),
        );
        let state = app.shared();
        Self {
            _temp: temp,
            _home,
            state,
            owner,
            agent,
            task_id,
            operation,
            input,
            key,
            worker,
            coordinator_log,
            threshold,
            cold,
        }
    }
    fn assert_first_refusal(&self) {
        let state = &self.state;
        let owner = &self.owner;
        let agent = &self.agent;
        let task_id = &self.task_id;
        let operation = &self.operation;
        // Drive the real queued assignment; no synthetic TurnFailed or consumed receipt.
        deliver_pending_agent_turns(state);
        {
            let mut app = state.lock().unwrap();
            assert_managed_block_failure(&mut app, owner, agent, task_id, 1);
            let entry = super::board::attention::board_entry(&mut app, owner);
            assert_eq!(entry["unread_reason"], "run_failed", "{entry}");
            assert_eq!(
                app.delivery_queue.queued_len(),
                0,
                "blocked wake is terminal, not replayed"
            );
            assert!(app
                .operation_receipt(operation)
                .unwrap()
                .unwrap()
                .execution_error
                .is_some());
        }
    }

    async fn assert_repeated_refusal(&self, scenario: ManagedBlockScenario) {
        let state = &self.state;
        let owner = &self.owner;
        let agent = &self.agent;
        let task_id = &self.task_id;
        let coordinator_log = &self.coordinator_log;
        if matches!(scenario, ManagedBlockScenario::HistoricalTail) {
            restore_tail_after_managed_failure(&mut state.lock().unwrap(), owner, agent);
        }
        let repeated = state.lock().unwrap().handle(req(
            "thread.post",
            json!({
                "entity_id":owner, "agent_id":agent, "conversation_id":agent,
                "operation_id":"managed-block-repeated-wake", "body":"another due wake",
            }),
        ));
        assert_eq!(repeated["ok"], true, "{repeated}");
        if matches!(scenario, ManagedBlockScenario::BusyStore) {
            assert_repeated_guard_never_queries_store(state, owner, agent).await;
        }
        deliver_pending_agent_turns(state);
        {
            let mut app = state.lock().unwrap();
            assert_managed_block_failure(&mut app, owner, agent, task_id, 1);
            assert_eq!(
                app.delivery_queue.queued_len(),
                0,
                "repeated due wakes are not replayed"
            );
        }
        DeliveryRunner::drain(state, &a_frame(state));
        wait_for(
            crate::app::runtime::delivery::queue::NOTICE_SETTLE_WINDOW + Duration::from_secs(5),
            || (!coordinator_log.turns().is_empty()).then_some(()),
        )
        .await
        .expect("the coordinator is notified without a client or recovery loop");
        assert_eq!(coordinator_log.turns().len(), 1);
    }

    fn queue_below_threshold(&self) {
        let state = &self.state;
        let owner = &self.owner;
        let agent = &self.agent;
        let task_id = &self.task_id;
        let input = &self.input;
        let worker = &self.worker;
        let threshold = &self.threshold;
        let cold = &self.cold;
        {
            let mut app = state.lock().unwrap();
            assert_managed_block_failure(&mut app, owner, agent, task_id, 1);
            assert!(!items(&mut app, owner, agent)
                .iter()
                .any(|row| row["data"]["from_build"] == true));
            assert!(
                std::fs::read(input).unwrap_or_default().is_empty(),
                "no due wake, /compact, replay or self-notice reaches the blocked child"
            );
            assert_managed_opening_state(worker.as_ref(), *cold);
            app.record_agent_turn_context(
                owner,
                agent,
                crate::harness::TurnContext {
                    context_tokens: threshold - 1,
                    cache_read_tokens: 0,
                },
            );
            let below = app.handle(req(
                "thread.post",
                json!({
                    "entity_id":owner, "agent_id":agent, "conversation_id":agent,
                    "operation_id":"managed-block-below-threshold", "body":"below threshold wake",
                }),
            ));
            assert_eq!(below["ok"], true, "{below}");
        }
    }

    async fn complete_below_threshold(&self) {
        let state = &self.state;
        let owner = &self.owner;
        let agent = &self.agent;
        let task_id = &self.task_id;
        let input = &self.input;
        let key = &self.key;
        let worker = &self.worker;
        let coordinator_log = &self.coordinator_log;
        let instance = state
            .lock()
            .unwrap()
            .session_registry
            .test_tab(key)
            .unwrap()
            .session_instance
            .clone();
        spawn_status_pump(
            state,
            self.key.clone(),
            Arc::clone(worker),
            instance,
            worker.status_changed(),
        );
        deliver_pending_agent_turns(state);
        let sent = wait_for(Duration::from_secs(5), || {
            let text = std::fs::read_to_string(input).ok()?;
            text.contains("below threshold wake").then_some(text)
        })
        .await
        .expect("below-threshold input reaches the live child normally");
        assert_eq!(
            sent.lines().count(),
            1,
            "only the below-threshold delivery reaches the child"
        );
        assert!(!sent.contains("/compact"));
        assert_managed_block_failure(&mut state.lock().unwrap(), owner, agent, task_id, 1);
        assert_eq!(coordinator_log.turns().len(), 1);
        wait_for(Duration::from_secs(5), || {
            let app = state.lock().unwrap();
            let record = app.runs[owner].agents.by_id(agent).unwrap();
            (worker.status() == AgentStatus::Waiting
                && record.last_context_tokens == Some(0)
                && record.working_since.is_none())
            .then_some(())
        })
        .await
        .expect("the below-threshold normal turn completes and its status pump settles");
    }

    async fn assert_reblocked_cycle(&self) {
        let state = &self.state;
        let owner = &self.owner;
        let agent = &self.agent;
        let task_id = &self.task_id;
        let input = &self.input;
        let coordinator_log = &self.coordinator_log;
        let threshold = &self.threshold;
        {
            let mut app = state.lock().unwrap();
            app.record_agent_turn_context(
                owner,
                agent,
                crate::harness::TurnContext {
                    context_tokens: *threshold,
                    cache_read_tokens: 0,
                },
            );
            let next_cycle = app.handle(req(
                "thread.post",
                json!({
                    "entity_id":owner, "agent_id":agent, "conversation_id":agent,
                    "operation_id":"managed-block-new-cycle", "body":"new due cycle after recovery",
                }),
            ));
            assert_eq!(next_cycle["ok"], true, "{next_cycle}");
        }
        deliver_pending_agent_turns(state);
        assert_managed_block_failure(&mut state.lock().unwrap(), owner, agent, task_id, 2);
        wait_for(
            crate::app::runtime::delivery::queue::NOTICE_SETTLE_WINDOW + Duration::from_secs(5),
            || (coordinator_log.turns().len() == 2).then_some(()),
        )
        .await
        .expect("a new blocked cycle produces fresh coordinator attention");
        assert_eq!(
            std::fs::read_to_string(input).unwrap().lines().count(),
            1,
            "the reblocked wake is also refused without delivery or compaction"
        );
    }
}

fn assert_managed_block_failure(
    app: &mut AppState,
    owner: &str,
    agent: &str,
    task_id: &str,
    expected_failures: usize,
) {
    let reports = app
        .store
        .as_ref()
        .unwrap()
        .thread_page(agent, None, 1_000)
        .unwrap();
    assert_eq!(
        reports
            .iter()
            .filter(
                |row| matches!(row, crate::thread::ThreadItem::Message(message)
                if message.outcome == Some(crate::thread::MessageOutcome::Failed))
            )
            .count(),
        expected_failures,
        "each blocked cycle must create exactly one visible RunFailed attention"
    );
    let entry = super::board::attention::board_entry(app, owner);
    assert_eq!(entry["needs_attention"], true, "{entry}");
    let read = app.handle(req("tasks.get", json!({"task_id":task_id})));
    let comments: Vec<_> = read["result"]["timeline"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["type"] == "comment")
        .collect();
    assert_eq!(comments.len(), 1, "{read}");
    assert!(
        comments[0]["body"]
            .as_str()
            .unwrap()
            .contains("DISABLE_COMPACT"),
        "{read}"
    );
}

fn restore_tail_after_managed_failure(app: &mut AppState, owner: &str, agent: &str) {
    app.edit_agent_conversation(owner, agent, |thread, _| {
        for _ in 0..=crate::store::RESIDENT_CONVERSATION_TAIL {
            thread.push_event(
                crate::thread::ThreadEventKind::ToolUse,
                Some("later activity".into()),
                None,
                None,
                now_rfc3339(),
            );
        }
        Ok(())
    })
    .unwrap();
    let mut reloaded = app
        .store
        .as_ref()
        .unwrap()
        .load_all_runs()
        .unwrap()
        .into_iter()
        .find(|run| run.id == owner)
        .unwrap();
    let restored = reloaded
        .agents
        .iter_mut()
        .find(|record| record.id == agent)
        .unwrap()
        .thread
        .clone();
    assert_eq!(
        restored.items.len(),
        crate::store::RESIDENT_CONVERSATION_TAIL
    );
    assert!(
        restored.managed_compaction_failure_reported,
        "the marker must survive the actual SQLite reload"
    );
    assert!(!restored.items.iter().any(
        |row| matches!(row, crate::thread::ThreadItem::Message(message)
        if message.outcome == Some(crate::thread::MessageOutcome::Failed))
    ));
    app.edit_agent_conversation(owner, agent, |thread, _| {
        *thread = restored;
        Ok(())
    })
    .unwrap();
}

async fn assert_repeated_guard_never_queries_store(
    state: &Arc<Mutex<AppState>>,
    owner: &str,
    agent: &str,
) {
    let store = state.lock().unwrap().store.clone().unwrap();
    let pool = store.read_pool();
    let mut readers = Vec::new();
    for _ in 0..8 {
        readers.push(pool.acquire().await);
    }
    let (held, ready) = std::sync::mpsc::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let writer = std::thread::spawn(move || {
        let _guard = store.writer_for_test();
        held.send(()).unwrap();
        wait.recv().unwrap();
    });
    ready.recv_timeout(Duration::from_secs(5)).unwrap();
    let work_state = Arc::clone(state);
    let owner = owner.to_string();
    let agent = agent.to_string();
    let (finished, completed) = std::sync::mpsc::channel();
    let guard = std::thread::spawn(move || {
        let mut app = work_state.lock().unwrap();
        let queued = app.delivery_queue.queued_last().unwrap();
        let turn = PendingAgentTurn {
            operation_id: None,
            root: queued.root.clone(),
            owner,
            agent_id: agent,
            conversation_id: queued.conversation_id.clone(),
            model_choice: queued.model_choice.clone(),
            choice_revision: queued.choice_revision,
            interrupt: false,
            phase: "budget",
            say: None,
            wants_catch_up: false,
            survives_refusal: false,
        };
        let started = std::time::Instant::now();
        let result = app.refuse_blocked_compaction(&turn);
        finished.send((result, started.elapsed())).unwrap();
    });
    let result = completed.recv_timeout(Duration::from_millis(100));
    release.send(()).unwrap();
    drop(readers);
    writer.join().unwrap();
    guard.join().unwrap();
    let (blocked, elapsed) =
        result.expect("duplicate guard must finish while writer and all readers are occupied");
    assert!(blocked.is_some());
    assert!(
        elapsed < Duration::from_millis(100),
        "duplicate guard took {elapsed:?}"
    );
}

async fn open_managed_child(input: &Path, choice: &ModelChoice, cold: bool) -> AdkSession {
    let script = if cold {
        r#"
            IFS= read -r line || exit
            printf '%s\n' "$line" >> "$1"
            printf '%s\n' '{"type":"system","subtype":"init","session_id":"managed-blocked","model":"synthetic"}'
            printf '%s\n' '{"type":"result","subtype":"success","is_error":false}'
            while IFS= read -r line; do
                printf '%s\n' "$line" >> "$1"
                printf '%s\n' '{"type":"result","subtype":"success","is_error":false}'
            done
        "#
    } else {
        r#"
            printf '%s\n' '{"type":"system","subtype":"init","session_id":"managed-blocked","model":"synthetic"}'
            while IFS= read -r line; do
                printf '%s\n' "$line" >> "$1"
                printf '%s\n' '{"type":"result","subtype":"success","is_error":false}'
            done
        "#
    };
    let spec = HarnessSpec::new("sh")
        .arg("-c")
        .arg(script)
        .arg("managed-blocked")
        .arg(input.to_string_lossy());
    let (session, _activity) = AdkSession::spawn(&spec, None, choice).unwrap();
    if !cold {
        wait_for(Duration::from_secs(5), || {
            (session.status() == AgentStatus::Waiting).then_some(())
        })
        .await
        .expect("the fake initializes without receiving a wake");
    }
    session
}

fn assert_managed_opening_state(worker: &dyn AgentSession, cold: bool) {
    assert_eq!(
        worker.status(),
        if cold {
            AgentStatus::Starting
        } else {
            AgentStatus::Waiting
        }
    );
    if cold {
        assert!(
            worker.compaction_policy().is_none(),
            "admission cannot promote active policy before init"
        );
    }
}

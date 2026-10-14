use super::project_agent::{items, project_agent, rooted, workspace};
use super::tracker::{filed, tracked};
use super::*;
use crate::provider_recovery::{RecoveryEpisode, RecoveryScope, RecoveryStage};

struct RecoveryFixture {
    root: tempfile::TempDir,
    _home: tempfile::TempDir,
    state: Option<AppState>,
    task: String,
    coordinator_owner: String,
    coordinator: String,
    episode: RecoveryEpisode,
}

impl RecoveryFixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let (home, mut state, project) = tracked(root.path());
        let (coordinator_owner, coordinator) = project_agent(&mut state, &project);
        let ws = workspace(&mut state, &project, "durable provider failure");
        let task = filed(&mut state, &project, "Report abandoned provider recovery");
        let task = task["id"].as_str().unwrap().to_string();
        let assigned = state.handle(req(
            "tasks.assign",
            json!({
                "task_id":task, "assignee":{"kind":"new_agent", "workspace_id":ws},
            }),
        ));
        assert_eq!(assigned["ok"], true, "{assigned}");
        let dispatch = &assigned["result"]["dispatch"];
        let owner = dispatch["entity_id"].as_str().unwrap().to_string();
        let agent = dispatch["agent_id"].as_str().unwrap().to_string();
        let operation = dispatch["operation_id"].as_str().unwrap().to_string();
        super::tracker_activity::consume_assignment_turns(&mut state);
        state
            .transition_delivery_operation(
                &operation,
                crate::operation::OperationStatus::Queued,
                crate::operation::OperationStatus::Claimed,
                None,
            )
            .unwrap();
        state
            .transition_delivery_operation(
                &operation,
                crate::operation::OperationStatus::Claimed,
                crate::operation::OperationStatus::Delivered,
                None,
            )
            .unwrap();
        state
            .record_native_operation_seen(&owner, &agent, &operation)
            .unwrap();
        state.record_agent_working_since(&owner, &agent, Some(now_rfc3339()));
        let thread_id = state
            .agent_conversation(&owner, Some(&agent))
            .unwrap()
            .id
            .clone();
        let episode = RecoveryEpisode {
            scope: RecoveryScope {
                entity_id: owner,
                agent_id: agent,
                conversation_id: dispatch["agent_id"].as_str().unwrap().into(),
                thread_id,
            },
            id: "revision-621-episode".into(),
            operation_id: Some(operation),
            summary: "provider stream disconnected".into(),
            retry_due_ms: 4_000_000_000_000,
            stage: RecoveryStage::Pending,
            active: true,
            needs_notice: false,
            recipient: None,
        };
        let directory = root
            .path()
            .join("provider-recovery")
            .join(sha256_hex(episode.scope.agent_id.as_bytes()));
        std::fs::create_dir_all(directory).unwrap();
        crate::provider_recovery::save_episode(root.path(), &episode).unwrap();
        Self {
            root,
            _home: home,
            state: Some(state),
            task,
            coordinator_owner,
            coordinator,
            episode,
        }
    }

    fn state(&mut self) -> &mut AppState {
        self.state.as_mut().unwrap()
    }

    fn reopen(&mut self) {
        drop(self.state.take());
        self.state = Some(
            rooted(self.root.path())
                .with_task_store(self.root.path().join("store"))
                .unwrap(),
        );
    }

    fn persisted_episode(&self) -> RecoveryEpisode {
        crate::provider_recovery::read_episode(
            self.root.path(),
            &self.episode.scope.agent_id,
            &self.episode.id,
        )
        .unwrap()
        .unwrap()
    }

    fn assert_single_reports(&mut self) {
        let scope = self.episode.scope.clone();
        let reports = items(self.state(), &scope.entity_id, &scope.agent_id);
        assert_eq!(
            reports
                .iter()
                .filter(|row| row["data"]["outcome"] == "failed")
                .count(),
            1,
            "{reports:?}"
        );
        let task = self.task.clone();
        let read = self
            .state()
            .handle(req("tasks.get", json!({"task_id":task})));
        assert_eq!(
            read["result"]["timeline"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|row| row["type"] == "comment")
                .count(),
            1,
            "{read}"
        );
        let owner = self.coordinator_owner.clone();
        let coordinator = self.coordinator.clone();
        let notices = items(self.state(), &owner, &coordinator);
        assert_eq!(
            notices
                .iter()
                .filter(|row| row["data"]["from_build"] == true
                    && row["data"]["body"]
                        .as_str()
                        .is_some_and(|body| body.contains("provider stream disconnected")))
                .count(),
            1,
            "{notices:?}"
        );
    }
}

#[test]
fn revision_621_recovery_reports_and_task_notice_survive_store_reopen_once() {
    let mut fixture = RecoveryFixture::new();
    fixture.reopen();
    for _ in 0..2 {
        fixture.state().replay_provider_recovery_episodes().unwrap();
        fixture.assert_single_reports();
        let record = fixture.persisted_episode();
        assert_eq!(record.stage, RecoveryStage::Terminal);
        assert!(record.active && !record.needs_notice);
        assert!(record.recipient.is_some());
        fixture.state().delivery_queue.lapse_settle_windows();
        let ready = fixture.state().delivery_queue.take_ready(|_| false);
        assert_eq!(
            ready.len(),
            1,
            "a notice wakes its coordinator, not the failed worker"
        );
        assert_eq!(ready[0].agent_id, fixture.coordinator);
        fixture.reopen();
    }
}

#[test]
fn revision_621_recovery_requeues_unread_notice_after_lost_volatile_wake() {
    let mut fixture = RecoveryFixture::new();
    fixture.state().replay_provider_recovery_episodes().unwrap();
    fixture.reopen();
    fixture.state().replay_provider_recovery_episodes().unwrap();
    fixture.assert_single_reports();
    fixture.state().delivery_queue.lapse_settle_windows();
    let ready = fixture.state().delivery_queue.take_ready(|_| false);
    assert_eq!(
        ready.len(),
        1,
        "restart must requeue the saved unread notice"
    );
    let recipient = fixture.persisted_episode().recipient.unwrap();
    fixture
        .state()
        .edit_agent_conversation(
            &recipient.scope.entity_id,
            &recipient.scope.agent_id,
            |thread, _| {
                let sequence = thread
                    .items
                    .iter()
                    .find_map(|item| match item {
                        crate::thread::ThreadItem::Message(message)
                            if message.id == recipient.message_id =>
                        {
                            Some(message.sequence)
                        }
                        _ => None,
                    })
                    .unwrap();
                thread.read_native_legacy_messages(sequence, sequence, &now_rfc3339());
                Ok(())
            },
        )
        .unwrap();
    fixture.reopen();
    fixture.state().replay_provider_recovery_episodes().unwrap();
    fixture.state().delivery_queue.lapse_settle_windows();
    assert!(
        fixture
            .state()
            .delivery_queue
            .take_ready(|_| false)
            .is_empty(),
        "a consumed notice cannot wake again"
    );
}

#[test]
fn revision_621_failed_report_write_keeps_durable_notice_pending() {
    let mut fixture = RecoveryFixture::new();
    fixture.state().store.as_ref().unwrap().fail_next_write();
    assert!(fixture.state().replay_provider_recovery_episodes().is_err());
    assert!(fixture.persisted_episode().needs_notice);
    fixture.reopen();
    fixture.state().replay_provider_recovery_episodes().unwrap();
    fixture.assert_single_reports();
    assert!(!fixture.persisted_episode().needs_notice);
}

#[test]
fn revision_621_live_failure_handler_does_not_abandon_a_pending_episode() {
    let mut fixture = RecoveryFixture::new();
    let episode = fixture.episode.clone();
    assert!(fixture
        .state()
        .record_provider_recovery_failure(
            &episode.scope.entity_id,
            &episode.scope.agent_id,
            &episode.id
        )
        .is_err());
    assert_eq!(fixture.persisted_episode().stage, RecoveryStage::Pending);
    assert!(!fixture.persisted_episode().needs_notice);
}

#[test]
fn revision_621_boot_replays_outbox_without_roster_and_when_resume_is_opted_out() {
    for opted_out in [false, true] {
        let mut fixture = RecoveryFixture::new();
        if opted_out {
            fixture
                .state()
                .resume_roster("old")
                .save(fixture.root.path())
                .unwrap();
            std::fs::write(fixture.root.path().join(crate::resume::OPT_OUT_FILE), "").unwrap();
        }
        let state = fixture.state.take().unwrap().shared();
        assert!(AppState::resume_after_restart(&state, fixture.root.path(), "new").is_empty());
        fixture.state = Some(Arc::try_unwrap(state).ok().unwrap().into_inner().unwrap());
        fixture.assert_single_reports();
        assert!(!fixture.persisted_episode().needs_notice);
        fixture.state().delivery_queue.lapse_settle_windows();
        let ready = fixture.state().delivery_queue.take_ready(|_| false);
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].agent_id, fixture.coordinator);
    }
}

#[test]
fn revision_621_operationless_old_outbox_preserves_a_new_assignment() {
    let mut fixture = RecoveryFixture::new();
    fixture.episode.operation_id = None;
    fixture.episode.active = false;
    fixture.episode.stage = RecoveryStage::Terminal;
    fixture.episode.needs_notice = true;
    crate::provider_recovery::save_episode(fixture.root.path(), &fixture.episode).unwrap();
    let scope = fixture.episode.scope.clone();
    let project = fixture
        .state()
        .projects
        .project_id_of(&scope.entity_id)
        .unwrap()
        .to_string();
    let task = filed(
        fixture.state(),
        &project,
        "Newer assignment survives old operationless failure",
    );
    let task = task["id"].as_str().unwrap().to_string();
    let assigned = fixture.state().handle(req(
        "tasks.assign",
        json!({"task_id":task,"assignee":{"kind":"agent","agent_id":scope.agent_id}}),
    ));
    assert_eq!(assigned["ok"], true, "{assigned}");
    super::tracker_activity::consume_assignment_turns(fixture.state());
    fixture
        .state()
        .record_agent_working_since(&scope.entity_id, &scope.agent_id, None);
    fixture.state().record_agent_working_since(
        &scope.entity_id,
        &scope.agent_id,
        Some("2026-10-13T22:01:00Z".into()),
    );
    let operation = fixture
        .state()
        .task_failure_operation(&scope.agent_id)
        .unwrap()
        .to_string();
    fixture.state().replay_provider_recovery_episodes().unwrap();
    let read = fixture
        .state()
        .handle(req("tasks.get", json!({"task_id":task})));
    assert!(
        !read["result"]["timeline"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["type"] == "comment"),
        "{read}"
    );
    assert_eq!(
        fixture.state().task_failure_operation(&scope.agent_id),
        Some(operation.as_str())
    );
    assert_eq!(
        fixture.state().runs[&scope.entity_id]
            .agents
            .by_id(&scope.agent_id)
            .unwrap()
            .working_since
            .as_deref(),
        Some("2026-10-13T22:01:00Z")
    );
}

#[test]
fn revision_621_crash_after_output_writes_reuses_deterministic_messages() {
    let mut fixture = RecoveryFixture::new();
    fixture.state().replay_provider_recovery_episodes().unwrap();
    let episode = fixture.episode.clone();
    crate::provider_recovery::update_episode(
        fixture.root.path(),
        &episode.scope.agent_id,
        &episode.id,
        |record| record.needs_notice = true,
    )
    .unwrap();
    fixture.reopen();
    fixture.state().replay_provider_recovery_episodes().unwrap();
    fixture.assert_single_reports();
    assert!(!fixture.persisted_episode().needs_notice);
}

#[test]
fn revision_621_stale_failure_scope_does_not_mutate_replacement_thread() {
    let mut fixture = RecoveryFixture::new();
    let scope = fixture.episode.scope.clone();
    let project = fixture
        .state()
        .projects
        .project_id_of(&scope.entity_id)
        .unwrap()
        .to_string();
    let reset = fixture.state().handle(req(
        "conversation.reset",
        json!({
            "project_id":project,"entity_id":scope.entity_id,"agent_id":scope.agent_id,
            "conversation_id":scope.conversation_id,"expected_thread_id":scope.thread_id,
        }),
    ));
    assert_eq!(reset["ok"], true, "{reset}");
    fixture.state().replay_provider_recovery_episodes().unwrap();
    let thread = fixture
        .state()
        .agent_conversation(&scope.entity_id, Some(&scope.agent_id))
        .unwrap();
    assert!(thread.items.is_empty());
    assert!(!fixture.persisted_episode().active);
}

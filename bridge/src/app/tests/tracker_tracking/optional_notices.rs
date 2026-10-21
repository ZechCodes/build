//! Automatic notices qualify only through their actual typed post and publication path.
use super::*;
use crate::timing::optional_notices::{OptionalNoticeKind as Kind, OptionalNotices};
use crate::timing::write_wait::{WriteCohort, WriteWaitRegistry, WAIT_BUDGET};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

struct Fixture {
    _directory: tempfile::TempDir,
    _repository: tempfile::TempDir,
    state: AppState,
    project: String,
    watcher: (String, String),
    task: String,
    registry: Arc<WriteWaitRegistry>,
    now: Arc<Mutex<Instant>>,
}
impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let (repository, mut state, project) = tracked(directory.path());
        let (watcher, task) = watching(&mut state, &project, "optional notice witness");
        state.initialize_snapshot_projection();
        let now = Arc::new(Mutex::new(Instant::now()));
        let read_now = now.clone();
        let registry = WriteWaitRegistry::with_clock(Arc::new(move || *read_now.lock().unwrap()));
        state.frame_clock.set_write_wait_registry(registry.clone());
        Self {
            _directory: directory,
            _repository: repository,
            state,
            project,
            watcher,
            task,
            registry,
            now,
        }
    }
    fn enroll(&self, original: &str, kind: Kind) -> Arc<OptionalNotices> {
        self.state
            .frame_clock
            .capture_optional_notices(vec![(original.into(), kind)])
    }
    fn update(&mut self, original: &str) -> crate::timing::write_wait::WriteContext {
        let root = self
            .registry
            .begin(original, "tasks.update", *self.now.lock().unwrap());
        let context = root.context();
        let reply = {
            let _bound = context.bind_sync();
            self.state.handle(req(
                "tasks.update",
                json!({"task_id":self.task,"status":"ready"}),
            ))
        };
        root.admitted();
        root.acknowledged();
        root.complete();
        assert_eq!(reply["ok"], true, "{reply}");
        context
    }
    fn advance(&self, seconds: u64) {
        *self.now.lock().unwrap() += Duration::from_secs(seconds);
    }
    fn finish(&mut self) {
        self.state.delivery_queue.lapse_settle_windows();
        let mut turns = self.state.delivery_queue.take_ready(|_| false);
        for turn in &mut turns {
            turn.complete_logical_origins();
        }
    }
    fn row(&self, id: &str) -> crate::timing::write_wait::WriteSnapshot {
        self.registry
            .snapshot()
            .into_iter()
            .find(|r| r.original_id == id)
            .unwrap()
    }
}

#[test]
fn optional_tracker_notice_requires_enrollment_and_completed_real_publication() {
    let mut fixture = Fixture::new();
    let capture = fixture.enroll("notice-after-publication", Kind::TrackerWatchNotice);
    let context = fixture.update("notice-after-publication");
    assert!(
        fixture
            .state
            .origin_publication_complete(&context, &fixture.watcher.0),
        "the actual helper has completed local publication before optional eligibility"
    );
    let visible = notices(&mut fixture.state, &fixture.watcher.0, &fixture.watcher.1);
    assert_eq!(
        visible.len(),
        1,
        "the actual automatic message is already durable"
    );
    assert!(fixture
        .state
        .delivery_queue
        .take_ready(|_| false)
        .is_empty());
    fixture.advance(6);
    let root = fixture.row("notice-after-publication");
    let samples = capture.samples();
    let active = capture.summary();
    fixture.finish();
    let completed = capture.summary();
    assert!(
        root.root_finished && root.outstanding.is_empty(),
        "{root:?}"
    );
    assert_eq!(samples.len(), 1);
    assert_eq!(samples[0].original_ids, ["notice-after-publication"]);
    assert_eq!(samples[0].kind, Kind::TrackerWatchNotice);
    assert_eq!(samples[0].delivery.cohort, WriteCohort::OptionalNotice);
    assert!(samples[0].delivery.known_wait_union > WAIT_BUDGET);
    assert!(!samples[0].delivery.breached());
    assert_eq!((active.created, active.active, active.completed), (1, 1, 0));
    assert_eq!(active.active_age_maximum, Duration::from_secs(6));
    assert_eq!(
        active.completed_maximum,
        Duration::ZERO,
        "active age is censored"
    );
    assert_eq!(
        (completed.created, completed.active, completed.completed),
        (1, 0, 1)
    );
    assert_eq!(completed.completed_p95, Duration::from_secs(6));
    assert_eq!(completed.completed_maximum, completed.completed_p95);
    assert!(!fixture.registry.has_breaches());
}

#[test]
fn optional_notice_wrong_kind_or_missing_enrollment_stays_required() {
    for enroll in [false, true] {
        let mut fixture = Fixture::new();
        let policy = fixture
            .state
            .frame_clock
            .capture_optional_notices(if enroll {
                vec![("wrong-rule".into(), Kind::CompleteReminder)]
            } else {
                Vec::new()
            });
        fixture.update("wrong-rule");
        fixture.advance(6);
        let waiting = fixture.row("wrong-rule");
        fixture.finish();
        assert!(
            waiting.breached() && !waiting.outstanding.is_empty(),
            "{waiting:?}"
        );
        assert_eq!(policy.summary().created, 0);
        assert!(fixture.registry.has_breaches());
    }
}

#[test]
fn optional_notice_phase_string_does_not_grant_the_semantic_rule() {
    let mut fixture = Fixture::new();
    let policy = fixture.enroll("not-a-tracker-notice", Kind::TrackerWatchNotice);
    let root = fixture.registry.begin(
        "not-a-tracker-notice",
        "thread.post",
        fixture.registry.now(),
    );
    {
        let _bound = root.context().bind_sync();
        fixture
            .state
            .post_from_build_and_wake(
                &fixture.watcher.0,
                &fixture.watcher.1,
                "task_notice",
                |thread, now| {
                    thread.post_user_from_build("ordinary required message".into(), now);
                },
            )
            .unwrap();
    }
    root.complete();
    fixture.advance(6);
    let waiting = fixture.row("not-a-tracker-notice");
    fixture.finish();
    assert!(
        waiting.breached(),
        "a phase label cannot authorize exclusion: {waiting:?}"
    );
    assert_eq!(policy.summary().created, 0);
}

#[test]
fn optional_notice_missing_publication_proof_stays_required() {
    let mut fixture = Fixture::new();
    let peer = coding_agent(&mut fixture.state, &fixture.project, "borrowed-peer");
    fixture.state.publish_all_snapshot_projection();
    let borrowed = fixture.state.take_run(&peer.0).unwrap();
    let policy = fixture.enroll("unpublished-notice", Kind::TrackerWatchNotice);
    let context = fixture.update("unpublished-notice");
    let premature_proof = fixture
        .state
        .origin_publication_complete(&context, &fixture.watcher.0);
    fixture.advance(6);
    let waiting = fixture.row("unpublished-notice");
    fixture.state.runs.insert(peer.0, borrowed);
    fixture
        .state
        .publish_owner_snapshot_projection(&fixture.watcher.0);
    fixture.state.flush_publication_metadata_for_test();
    fixture.finish();
    let drained = fixture.row("unpublished-notice");
    assert!(
        !premature_proof,
        "the borrowed peer really prevents local publication completion"
    );
    assert!(
        drained.outstanding.is_empty(),
        "cleanup completes real metadata debt: {drained:?}"
    );
    assert!(
        waiting.breached() && !waiting.outstanding.is_empty(),
        "{waiting:?}"
    );
    assert_eq!(
        policy.summary().created,
        0,
        "accepted publication is required at enqueue"
    );
    assert!(
        fixture.registry.has_breaches(),
        "late publication cannot erase the wait"
    );
}

#[test]
fn optional_notice_join_keeps_the_direct_post_required() {
    let mut fixture = Fixture::new();
    let policy = fixture.enroll("optional-side", Kind::TrackerWatchNotice);
    fixture.update("optional-side");
    let direct = fixture
        .registry
        .begin("required-side", "thread.post", fixture.registry.now());
    {
        let _bound = direct.context().bind_sync();
        let reply = fixture.state.handle(req("thread.post", json!({
            "entity_id":fixture.watcher.0, "agent_id":fixture.watcher.1, "body":"required direct post"
        })));
        assert_eq!(reply["ok"], true, "{reply}");
    }
    direct.complete();
    fixture.advance(6);
    let required = fixture.row("required-side");
    let mut ready = fixture.state.delivery_queue.take_ready(|_| false);
    let children = ready
        .iter()
        .flat_map(|turn| turn.logical_contexts())
        .count();
    for turn in &mut ready {
        turn.complete_logical_origins();
    }
    assert_eq!(ready.len(), 1, "native catch-up coalescing is unchanged");
    assert_eq!(children, 2, "the optional and required child both survive");
    assert!(required.breached());
    assert!(fixture.registry.has_breaches());
    assert_eq!(policy.summary().created, 1);
    assert!(!policy.samples()[0].delivery.breached());
}

#[test]
fn optional_complete_reminder_keeps_its_real_next_delivery_rule() {
    let mut fixture = Fixture::new();
    let (owner, agent) = reported(
        &mut fixture.state,
        &fixture.project,
        DoneStatus::Blocked,
        &[
            ("still owed", "in_progress"),
            ("another open task", "in_progress"),
        ],
    );
    fixture.state.delivery_queue.clear_queued();
    let policy = fixture.enroll("reminder-after-publication", Kind::CompleteReminder);
    let root = fixture.registry.begin(
        "reminder-after-publication",
        "mcp.control:done",
        fixture.registry.now(),
    );
    {
        let _bound = root.context().bind_sync();
        fixture.state.remind_of_open_tasks(&owner, &agent);
    }
    root.complete();
    assert_eq!(reminders(&mut fixture.state, &owner, &agent).len(), 1);
    fixture.state.delivery_queue.lapse_settle_windows();
    assert!(
        fixture
            .state
            .delivery_queue
            .take_ready(|_| false)
            .is_empty(),
        "the reminder does not gain its own wake from qualification"
    );
    fixture.advance(6);
    let active = policy.summary();
    let original = fixture.row("reminder-after-publication");
    let wake = fixture.state.handle(req(
        "thread.post",
        json!({
            "entity_id":owner,"agent_id":agent,"body":"real wake for reminder"
        }),
    ));
    let mut ready = fixture.state.delivery_queue.take_ready(|_| false);
    for turn in &mut ready {
        turn.complete_logical_origins();
    }
    assert_eq!(wake["ok"], true);
    assert_eq!(ready.len(), 1);
    assert!(
        original.root_finished && original.outstanding.is_empty(),
        "{original:?}"
    );
    assert_eq!((active.created, active.active, active.completed), (1, 1, 0));
    let samples = policy.samples();
    assert_eq!(samples[0].kind, Kind::CompleteReminder);
    assert_eq!(samples[0].original_ids, ["reminder-after-publication"]);
    assert!(samples[0].delivery.completed_at.is_some());
    assert_eq!(samples[0].elapsed, Duration::from_secs(6));
    assert!(!fixture.registry.has_breaches());
}

#[test]
fn optional_enrollment_never_exempts_a_comment_to_the_actual_or_pending_holder() {
    for pending in [false, true] {
        let mut fixture = Fixture::new();
        if pending {
            let blocker = task_id(&filed(
                &mut fixture.state,
                &fixture.project,
                "unresolved prerequisite",
            ));
            let edge = fixture.state.handle(req(
                "tasks.waiting_on.add",
                json!({
                    "task_id":fixture.task,"waiting_on":[blocker]
                }),
            ));
            assert_eq!(edge["ok"], true, "{edge}");
        }
        let assignment = fixture.state.handle(req(
            "tasks.assign",
            json!({
                "task_id":fixture.task, "assignee":{"kind":"agent","agent_id":fixture.watcher.1}
            }),
        ));
        assert_eq!(assignment["ok"], true, "{assignment}");
        super::super::tracker_activity::consume_assignment_turns(&mut fixture.state);
        fixture.state.delivery_queue.clear_queued();
        let before = notices(&mut fixture.state, &fixture.watcher.0, &fixture.watcher.1).len();
        let policy = fixture.enroll("holder-question", Kind::TrackerWatchNotice);
        let root =
            fixture
                .registry
                .begin("holder-question", "tasks.comment", fixture.registry.now());
        let context = root.context();
        {
            let _bound = context.bind_sync();
            let reply = fixture.state.handle(req(
                "tasks.comment",
                json!({
                    "task_id":fixture.task,"body":"Question for the task holder","notify_user":false
                }),
            ));
            assert_eq!(reply["ok"], true, "{reply}");
        }
        let local_publication = fixture
            .state
            .origin_publication_complete(&context, &fixture.watcher.0);
        root.complete();
        fixture.advance(6);
        let holder = fixture.row("holder-question");
        let visible = notices(&mut fixture.state, &fixture.watcher.0, &fixture.watcher.1);
        fixture.finish();
        assert!(
            local_publication,
            "the rule rejects despite genuine local completion"
        );
        assert_eq!(
            visible.len(),
            before + 1,
            "actual holder receives the question (pending={pending})"
        );
        assert!(
            holder.breached() && !holder.outstanding.is_empty(),
            "{holder:?}"
        );
        assert_eq!(
            policy.summary().created,
            0,
            "recipient role defeats enrollment"
        );
        assert!(fixture.registry.has_breaches());
    }
}

#[test]
fn optional_cleanup_refuses_required_children_and_retains_censored_evidence() {
    let mut mixed = Fixture::new();
    let policy = mixed.enroll("optional-mixed", Kind::TrackerWatchNotice);
    mixed.update("optional-mixed");
    let direct = mixed
        .registry
        .begin("required-mixed", "thread.post", mixed.registry.now());
    {
        let _bound = direct.context().bind_sync();
        let reply = mixed.state.handle(req(
            "thread.post",
            json!({
                "entity_id":mixed.watcher.0,"agent_id":mixed.watcher.1,"body":"required work"
            }),
        ));
        assert_eq!(reply["ok"], true, "{reply}");
    }
    direct.complete();
    let rejected = mixed.state.cancel_optional_notices_for_cleanup();
    let required_after = mixed.row("required-mixed");
    mixed.finish();
    assert!(
        rejected.is_err(),
        "cleanup must reject an outstanding required child"
    );
    assert!(
        !required_after.outstanding.is_empty() && !required_after.cancelled,
        "{required_after:?}"
    );
    assert_eq!(policy.summary().cancelled_for_cleanup, 0);

    let mut only = Fixture::new();
    let policy = only.enroll("optional-only", Kind::TrackerWatchNotice);
    only.update("optional-only");
    only.advance(6);
    let before_cleanup = policy.summary();
    let raw_before = policy.samples();
    let cancelled = only.state.cancel_optional_notices_for_cleanup();
    let after_cleanup = policy.summary();
    assert_eq!(cancelled.unwrap(), 1);
    assert_eq!(
        (
            before_cleanup.created,
            before_cleanup.completed,
            before_cleanup.active
        ),
        (1, 0, 1)
    );
    assert_eq!(before_cleanup.active_age_maximum, Duration::from_secs(6));
    assert_eq!(before_cleanup.completed_maximum, Duration::ZERO);
    assert_eq!(
        raw_before.len(),
        1,
        "freeze active raw evidence before cancellation"
    );
    assert!(raw_before[0].delivery.completed_at.is_none());
    assert_eq!(after_cleanup.cancelled_for_cleanup, 1);
    assert_eq!(
        after_cleanup.completed, 0,
        "cleanup is not delivery completion"
    );
    assert_eq!(after_cleanup.active, 0);
    assert!(policy.samples()[0].delivery.cancelled);
}

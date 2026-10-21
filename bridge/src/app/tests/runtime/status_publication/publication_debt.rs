//! Required local publication survives the originating frame's acknowledgment.
use super::*;
use crate::app::read_view::SnapshotDelta;
use crate::app::snapshot_projection::publication_debt::{
    observe_install_receipts, observe_metadata_tail, observe_prepared,
};
use crate::timing::write_wait::{current_context, WriteContext, WriteWaitRegistry};

const OTHER: &str = "run-publication-debt-peer";

fn fixture() -> Fixture {
    let fixture = Fixture::passive();
    {
        let mut app = fixture.state.lock().unwrap();
        let repo = app.project_at(0).repo_path.clone();
        insert_run(
            &mut app,
            &repo,
            fixture._directory.path(),
            OTHER,
            RunState::Building,
        );
        app.seed_anchors_for_records_without_one();
        for owner in [OWNER, OTHER] {
            let active = app.take_run(owner).unwrap();
            app.finish_run_mutation(owner.into(), active).unwrap();
        }
        app.delivery_queue.clear_queued();
        // Keep an unrelated narrow producer genuinely unrelated: no registered
        // target session or running clock may add this owner incidentally.
        app.session_registry.test_remove_tab(&fixture.key).unwrap();
        app.publish_all_snapshot_projection();
    }
    fixture
}

fn goal(fixture: &Fixture) -> Value {
    fixture
        .state
        .lock()
        .unwrap()
        .read_view
        .load()
        .get_owned("entity.get", OWNER)
        .unwrap()["goal"]
        .clone()
}
fn durable_goal(fixture: &Fixture) -> String {
    fixture
        .state
        .lock()
        .unwrap()
        .store
        .as_ref()
        .unwrap()
        .load_all_runs()
        .unwrap()
        .into_iter()
        .find(|run| run.id == OWNER)
        .unwrap()
        .goal
}
fn borrow(fixture: &Fixture) -> crate::orchestrator::ActiveRun {
    fixture.state.lock().unwrap().take_run(OTHER).unwrap()
}
fn restore(fixture: &Fixture, active: crate::orchestrator::ActiveRun) {
    fixture
        .state
        .lock()
        .unwrap()
        .runs
        .insert(OTHER.into(), active);
}
fn commit(
    fixture: &Fixture,
    registry: &Arc<WriteWaitRegistry>,
    original: &str,
    label: &str,
) -> WriteContext {
    let write = registry.begin(original, "thread.post", std::time::Instant::now());
    let context = write.context();
    {
        let _bound = context.bind_sync();
        let mut app = fixture.state.lock().unwrap();
        let mut active = app.take_run(OWNER).unwrap();
        active.run.goal = label.into();
        app.finish_run_mutation(OWNER.into(), active).unwrap();
    }
    assert_eq!(
        durable_goal(fixture),
        label,
        "the owner transaction really committed"
    );
    write.admitted();
    write.acknowledged();
    write.complete();
    context
}
fn pending(registry: &WriteWaitRegistry, original: &str) {
    let rows = registry.active_snapshot();
    let row = rows
        .iter()
        .find(|row| row.original_id == original)
        .unwrap_or_else(|| {
            panic!("required publication disappeared after acknowledgment: {rows:?}")
        });
    assert!(row.root_finished);
    assert!(!row.outstanding.is_empty());
}
fn prepare(fixture: &Fixture) -> SnapshotDelta {
    fixture
        .state
        .lock()
        .unwrap()
        .prepare_owner_snapshot_projection(OWNER)
        .unwrap()
}
fn publish(fixture: &Fixture, candidate: SnapshotDelta) {
    fixture
        .state
        .lock()
        .unwrap()
        .publish_prepared_snapshot_projection(candidate);
}
fn metadata(fixture: &Fixture) {
    fixture
        .state
        .lock()
        .unwrap()
        .flush_publication_metadata_for_test();
}
fn retry(fixture: &Fixture) {
    fixture
        .state
        .lock()
        .unwrap()
        .publish_owner_snapshot_projection(OWNER);
}
fn proof(fixture: &Fixture, context: &WriteContext) -> bool {
    fixture
        .state
        .lock()
        .unwrap()
        .origin_publication_complete(context, OWNER)
}

#[test]
fn successful_owner_publication_has_exact_local_completion_proof() {
    let fixture = fixture();
    let registry = WriteWaitRegistry::new();
    let context = commit(
        &fixture,
        &registry,
        "publication-normal",
        "committed normal goal",
    );
    assert_eq!(goal(&fixture), "committed normal goal");
    assert!(
        proof(&fixture, &context),
        "accepted owner plus metadata tail must establish proof"
    );
    assert!(registry.active_snapshot().is_empty());
    fixture.assert_full_oracle();
}

#[test]
fn borrowed_owner_keeps_publication_required_after_original_acknowledgment() {
    let fixture = fixture();
    let before = goal(&fixture);
    let held = borrow(&fixture);
    let registry = WriteWaitRegistry::new();
    let context = commit(
        &fixture,
        &registry,
        "publication-borrowed",
        "durable borrowed goal",
    );
    assert_eq!(
        goal(&fixture),
        before,
        "borrowed barrier must really postpone publication"
    );
    assert!(!proof(&fixture, &context));
    pending(&registry, "publication-borrowed");
    restore(&fixture, held);
    fixture.state.lock().unwrap().settle_failed_mutation(OTHER);
    assert_eq!(goal(&fixture), "durable borrowed goal");
    assert!(proof(&fixture, &context));
    assert!(registry.active_snapshot().is_empty());
    fixture.assert_full_oracle();
}

#[test]
fn rejected_candidate_retains_metadata_done_debt_until_accepted_retry() {
    let fixture = fixture();
    let held = borrow(&fixture);
    let registry = WriteWaitRegistry::new();
    let context = commit(
        &fixture,
        &registry,
        "publication-rejected",
        "rejected candidate goal",
    );
    restore(&fixture, held);
    let candidate = prepare(&fixture);
    let advanced = {
        let app = fixture.state.lock().unwrap();
        let previous = app.read_view.load();
        assert!(app.read_view.publish_delta(
            SnapshotDelta::new(app.read_view.next_version())
                .set_global(previous.global.clone().unwrap())
        ));
        app.read_view.load()
    };
    publish(&fixture, candidate);
    assert!(Arc::ptr_eq(
        &advanced,
        &fixture.state.lock().unwrap().read_view.load()
    ));
    metadata(&fixture);
    assert!(
        Arc::ptr_eq(&advanced, &fixture.state.lock().unwrap().read_view.load()),
        "metadata tail must not rescue the rejected projection"
    );
    pending(&registry, "publication-rejected");
    assert!(!proof(&fixture, &context));
    retry(&fixture);
    assert!(proof(&fixture, &context));
    assert!(registry.active_snapshot().is_empty());
}

#[test]
fn older_accepted_owner_revision_cannot_finish_a_newer_committed_origin() {
    let fixture = fixture();
    let registry = WriteWaitRegistry::new();
    let held = borrow(&fixture);
    let first = commit(
        &fixture,
        &registry,
        "publication-old",
        "first committed goal",
    );
    restore(&fixture, held);
    let candidate = prepare(&fixture);
    let held = borrow(&fixture);
    let second = commit(
        &fixture,
        &registry,
        "publication-new",
        "second committed goal",
    );
    restore(&fixture, held);
    publish(&fixture, candidate);
    assert_eq!(
        goal(&fixture),
        "first committed goal",
        "the older candidate was genuinely accepted"
    );
    metadata(&fixture);
    assert_eq!(
        goal(&fixture),
        "first committed goal",
        "tail did not publish the newer revision"
    );
    pending(&registry, "publication-new");
    assert!(proof(&fixture, &first));
    assert!(!proof(&fixture, &second));
    retry(&fixture);
    assert_eq!(goal(&fixture), "second committed goal");
    assert!(proof(&fixture, &second));
    assert!(registry.active_snapshot().is_empty());
}

#[test]
fn unrelated_accepted_candidate_cannot_consume_another_owners_required_publication() {
    let fixture = fixture();
    let before = goal(&fixture);
    let held = borrow(&fixture);
    let registry = WriteWaitRegistry::new();
    let context = commit(
        &fixture,
        &registry,
        "publication-unrelated",
        "unrelated acceptance goal",
    );
    restore(&fixture, held);
    let candidate = {
        let app = fixture.state.lock().unwrap();
        let _ownership = app.begin_snapshot_ownership();
        app.prepare_incremental_owner_snapshot(
            SnapshotDelta::new(app.read_view.next_version()),
            &app.read_view.load(),
            &[OTHER.to_owned()].into_iter().collect(),
        )
        .expect("unrelated inactive owner must retain a narrow candidate")
    };
    publish(&fixture, candidate);
    metadata(&fixture);
    assert_eq!(
        goal(&fixture),
        before,
        "unrelated candidate must really omit target owner rows"
    );
    pending(&registry, "publication-unrelated");
    assert!(!proof(&fixture, &context));
    retry(&fixture);
    assert_eq!(goal(&fixture), "unrelated acceptance goal");
    assert!(proof(&fixture, &context));
    assert!(registry.active_snapshot().is_empty());
}

#[test]
fn accepted_installer_and_metadata_tail_bind_only_live_owed_origin_children() {
    let fixture = fixture();
    let held = borrow(&fixture);
    let registry = WriteWaitRegistry::new();
    let context = commit(&fixture, &registry, "publication-tail", "tail-bound goal");
    restore(&fixture, held);
    let observations = Arc::new(Mutex::new(Vec::new()));
    let install = Arc::clone(&observations);
    let _install = observe_install_receipts(move |app| {
        install.lock().unwrap().push((
            "install",
            current_context()
                .map(|context| context.original_ids())
                .unwrap_or_default(),
            app.read_view.load().get_owned("entity.get", OWNER).unwrap()["goal"].clone(),
        ));
    });
    let tail = Arc::clone(&observations);
    let _tail = observe_metadata_tail(move |app, owner| {
        if owner == OWNER {
            tail.lock().unwrap().push((
                "metadata",
                current_context()
                    .map(|context| context.original_ids())
                    .unwrap_or_default(),
                app.read_view.load().get_owned("entity.get", OWNER).unwrap()["goal"].clone(),
            ));
        }
    });
    fixture.state.lock().unwrap().settle_failed_mutation(OTHER);
    let observed = observations.lock().unwrap();
    assert!(observed.iter().any(|(phase, _, _)| *phase == "install"));
    assert!(observed.iter().any(|(phase, _, _)| *phase == "metadata"));
    for (_, origins, published) in observed.iter() {
        assert_eq!(
            origins,
            &["publication-tail".to_owned()],
            "retry must bind the retained child, not a finished root or fresh origin"
        );
        assert_eq!(published, "tail-bound goal");
    }
    assert!(proof(&fixture, &context));
    assert!(registry.active_snapshot().is_empty());
}

#[test]
fn failed_save_creates_no_debt_and_never_recycles_its_prepared_revision() {
    let fixture = fixture();
    let before = durable_goal(&fixture);
    let registry = WriteWaitRegistry::new();
    let captures = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&captures);
    let _observer = observe_prepared(move |owner, receipts| {
        if owner == OWNER {
            captured.lock().unwrap().push(receipts.to_vec());
        }
    });
    fixture.sql("CREATE TRIGGER reject_publication_debt_save BEFORE INSERT ON implementations BEGIN SELECT RAISE(ABORT, 'injected publication debt save failure'); END;");
    let write = registry.begin(
        "publication-failed",
        "thread.post",
        std::time::Instant::now(),
    );
    let context = write.context();
    {
        let _bound = context.bind_sync();
        let mut app = fixture.state.lock().unwrap();
        let mut active = app.take_run(OWNER).unwrap();
        active.run.goal = "must not commit".into();
        let error = app.finish_run_mutation(OWNER.into(), active).unwrap_err();
        assert!(
            error.contains("injected publication debt save failure"),
            "{error}"
        );
    }
    write.complete();
    assert_eq!(durable_goal(&fixture), before);
    assert!(
        registry.active_snapshot().is_empty(),
        "staging cannot create a committed obligation"
    );
    assert!(!proof(&fixture, &context));
    fixture.sql("DROP TRIGGER reject_publication_debt_save;");
    let _later = commit(
        &fixture,
        &registry,
        "publication-after-failure",
        "later successful goal",
    );
    let captures = captures.lock().unwrap();
    assert_eq!(captures.len(), 2);
    let revision = |index: usize| {
        captures[index]
            .iter()
            .find(|receipt| receipt.owner == OWNER)
            .expect("actual prepared candidate needs owner revision coverage")
            .revision
    };
    assert!(
        revision(1) > revision(0),
        "aborted reservation must never be reused"
    );
}

#[test]
fn custom_reset_retains_owner_debt_until_its_borrowed_publication_can_finish() {
    let fixture = fixture();
    let params = {
        let app = fixture.state.lock().unwrap();
        let agent = app.entity_agents(OWNER).unwrap().primary().unwrap();
        json!({"project_id":app.projects.project_id_of(OWNER).unwrap(),"entity_id":OWNER,"agent_id":agent.id,"conversation_id":agent.conversation_id(),"expected_thread_id":agent.thread.id})
    };
    let old_thread = params["expected_thread_id"].clone();
    let held = borrow(&fixture);
    let registry = WriteWaitRegistry::new();
    let write = registry.begin(
        "publication-reset",
        "conversation.reset",
        std::time::Instant::now(),
    );
    let context = write.context();
    let reply = {
        let _bound = context.bind_sync();
        fixture
            .state
            .lock()
            .unwrap()
            .handle(req("conversation.reset", params))
    };
    assert_eq!(reply["ok"], true, "{reply}");
    assert_ne!(reply["result"]["thread_id"], old_thread);
    assert_eq!(
        fixture.durable_agent().thread.id,
        reply["result"]["thread_id"]
    );
    write.acknowledged();
    write.complete();
    pending(&registry, "publication-reset");
    assert!(!proof(&fixture, &context));
    restore(&fixture, held);
    fixture
        .state
        .lock()
        .unwrap()
        .publish_all_snapshot_projection();
    assert!(proof(&fixture, &context));
    assert!(registry.active_snapshot().is_empty());
    fixture.assert_full_oracle();
}

#[test]
fn borrowed_publication_wait_is_visible_to_independent_watchdog_after_ack() {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};
    let fixture = fixture();
    let held = borrow(&fixture);
    let base = Instant::now();
    let ticks = Arc::new(AtomicU64::new(0));
    let clock = Arc::clone(&ticks);
    let registry = WriteWaitRegistry::with_clock(Arc::new(move || {
        base + Duration::from_millis(clock.load(Ordering::Acquire))
    }));
    let (send, receive) = mpsc::channel();
    let watchdog = registry.watchdog(
        Duration::from_millis(1),
        Arc::new(move |row| {
            let _ = send.send(row);
        }),
    );
    let write = registry.begin("publication-queued-watchdog", "thread.post", base);
    let context = write.context();
    let committed = {
        let _bound = context.bind_sync();
        let mut app = fixture.state.lock().unwrap();
        let mut active = app.take_run(OWNER).unwrap();
        active.run.goal = "queued publication goal".into();
        app.finish_run_mutation(OWNER.into(), active)
    };
    write.acknowledged();
    write.complete();
    // Advance only after the real borrowed-owner publication barrier returned.
    // The fixture does not invent an interval; the actual debt hook must do so.
    ticks.store(6_000, Ordering::Release);
    let observed = receive.recv_timeout(Duration::from_secs(1));
    restore(&fixture, held);
    fixture.state.lock().unwrap().settle_failed_mutation(OTHER);
    drop(watchdog);
    // Restore and settle the real borrowed owner before any fault assertions.
    committed.unwrap();
    assert_eq!(durable_goal(&fixture), "queued publication goal");
    let row =
        observed.expect("owned-but-interval-free publication debt cannot satisfy the wait budget");
    assert_eq!(row.original_id, "publication-queued-watchdog");
    assert!(row.root_finished && row.breached());
    assert!(row.active.iter().any(|interval| matches!(
        interval.category,
        crate::timing::write_wait::IntervalCategory::KnownWait(_)
    )));
    assert!(row.known_wait_union >= Duration::from_secs(6));
    assert!(proof(&fixture, &context));
    assert!(registry.active_snapshot().is_empty());
    assert!(registry
        .snapshot()
        .iter()
        .find(|row| row.original_id == "publication-queued-watchdog")
        .unwrap()
        .breached());
}

#[test]
fn same_origin_outer_borrow_is_own_work_not_predecessor_wait() {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{Duration, Instant};
    let fixture = fixture();
    let base = Instant::now();
    let ticks = Arc::new(AtomicU64::new(0));
    let clock = Arc::clone(&ticks);
    let registry = WriteWaitRegistry::with_clock(Arc::new(move || {
        base + Duration::from_millis(clock.load(Ordering::Acquire))
    }));
    let write = registry.begin("publication-own-outer-borrow", "thread.post", base);
    let id = write.id();
    let context = write.context();
    let (held, committed) = {
        let _bound = context.bind_sync();
        let mut app = fixture.state.lock().unwrap();
        let held = app.take_run(OTHER).unwrap();
        let mut active = app.take_run(OWNER).unwrap();
        active.run.goal = "nested own-origin goal".into();
        let committed = app.finish_run_mutation(OWNER.into(), active);
        (held, committed)
    };
    let committed_goal = durable_goal(&fixture);
    let accepted_goal = goal(&fixture);
    write.acknowledged();
    write.complete();
    ticks.store(6_000, Ordering::Release);
    let delayed = registry
        .active_snapshot()
        .into_iter()
        .find(|row| row.id == id);
    restore(&fixture, held);
    fixture.state.lock().unwrap().settle_failed_mutation(OTHER);
    committed.unwrap();
    assert_eq!(committed_goal, "nested own-origin goal");
    assert_ne!(accepted_goal, "nested own-origin goal");
    let delayed = delayed.expect("publication still owes the nested owner");
    assert_eq!(delayed.known_wait_union, Duration::ZERO);
    assert_eq!(delayed.own_external_union, Duration::from_secs(6));
    assert!(
        !delayed.breached(),
        "the origin's own outer work is not lock contention"
    );
    assert!(proof(&fixture, &context));
    assert!(registry.active_snapshot().is_empty());
    fixture.assert_full_oracle();
}

#[test]
fn retry_resource_origins_are_not_authors_of_nested_owner_mutations() {
    let mut outcomes = Vec::new();
    for has_current_origin in [false, true] {
        let fixture = fixture();
        let registry = WriteWaitRegistry::new();
        let held = borrow(&fixture);
        let old = commit(&fixture, &registry, "old-debt-author", "old debt goal");
        restore(&fixture, held);
        let current = has_current_origin.then(|| {
            registry.begin(
                "current-mutation-author",
                "thread.post",
                std::time::Instant::now(),
            )
        });
        let current_context = current.as_ref().map(|write| write.context());
        let (nested_commit, old_is_author, current_is_author) = {
            let _current = current_context.as_ref().map(WriteContext::bind_sync);
            let mut app = fixture.state.lock().unwrap();
            let retry_work =
                app.begin_publication_owner_work(&[OWNER.to_owned()].into_iter().collect());
            let mut peer = app.take_run(OTHER).unwrap();
            peer.run.goal = "nested retry mutation".into();
            let committed = app.finish_run_mutation(OTHER.into(), peer);
            let old_is_author = app.origin_publication_complete(&old, OTHER);
            let current_is_author = current_context
                .as_ref()
                .map(|context| app.origin_publication_complete(context, OTHER));
            retry_work.complete();
            app.settle_failed_mutation(OWNER);
            (committed, old_is_author, current_is_author)
        };
        if let Some(current) = current {
            current.complete();
        }
        nested_commit.unwrap();
        {
            let app = fixture.state.lock().unwrap();
            assert_eq!(
                app.store
                    .as_ref()
                    .unwrap()
                    .load_all_runs()
                    .unwrap()
                    .into_iter()
                    .find(|run| run.id == OTHER)
                    .unwrap()
                    .goal,
                "nested retry mutation"
            );
            assert_eq!(
                app.read_view.load().get_owned("entity.get", OTHER).unwrap()["goal"],
                "nested retry mutation"
            );
        }
        assert!(proof(&fixture, &old));
        assert!(registry.active_snapshot().is_empty());
        fixture.assert_full_oracle();
        outcomes.push((has_current_origin, old_is_author, current_is_author));
    }
    for (has_current_origin, old_is_author, current_is_author) in outcomes {
        assert!(
            !old_is_author,
            "resource attribution cannot invent mutation intent"
        );
        assert_eq!(current_is_author, has_current_origin.then_some(true));
    }
}

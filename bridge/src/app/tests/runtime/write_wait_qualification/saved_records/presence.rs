//! OFFSOURCE TESTFIRST child of saved_records. First two are real encrypted
//! user.present requests; last is explicitly a deterministic native-source unit.
use super::effects_test_helpers::finish_caught;
use super::fixture::SavedFixture;
use crate::app::accepted_records::{
    AcceptedRecord, AcceptedRecords, PresencePersistence, PresenceRecordWitness,
};
use crate::app::tests::*;
use crate::session_summary::UserSession;
use crate::timing::write_wait::WriteWaitRegistry;
use futures_util::FutureExt;
use std::panic::AssertUnwindSafe;
use std::time::Instant;

fn only_presence(records: Vec<AcceptedRecord>) -> Vec<PresenceRecordWitness> {
    records
        .into_iter()
        .map(|record| match record {
            AcceptedRecord::Presence(presence) => presence,
            _ => panic!("unexpected sibling kind in explicit Presence-only control"),
        })
        .collect()
}
fn seed_known_durable(records: &AcceptedRecords, store: &Store) {
    // Actual setup SQL, no AppState guard, no later request can mask a failure.
    records.seed_known_durable_user_session(store.load_user_session().unwrap());
}
async fn later_than(ts: Option<i64>) {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while ts.is_some_and(|before| i64::try_from(crate::agent::now_ms()).unwrap() <= before) {
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("native wall clock did not advance within fixture budget");
}
fn native_session(fixture: &SavedFixture) -> (UserSession, UserSession) {
    let app = fixture.state.lock().unwrap();
    (
        app.user_session(),
        app.test_published_user_session()
            .expect("actual initialized read publication"),
    )
}
fn session(reply: &Value) -> UserSession {
    serde_json::from_value(reply["result"]["user_session"].clone()).unwrap()
}
struct RejectUserSession {
    store: Store,
}
impl RejectUserSession {
    fn new(store: &Store) -> Self {
        // fail_next_write is NOT used: save_user_session bypasses that path.
        store.writer_for_test().execute_batch("CREATE TRIGGER qualification_reject_user_session BEFORE INSERT ON meta WHEN NEW.key='user_session' BEGIN SELECT RAISE(FAIL,'qualification actual user_session save refusal'); END;").unwrap();
        Self {
            store: store.clone(),
        }
    }
}
impl Drop for RejectUserSession {
    fn drop(&mut self) {
        self.store
            .writer_for_test()
            .execute_batch("DROP TRIGGER IF EXISTS qualification_reject_user_session")
            .unwrap();
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn presence_keeps_original_success_after_later_native_presence() {
    crate::git_fixture::environment::isolated_git_test!();
    let records = AcceptedRecords::new();
    let fixture = SavedFixture::new(records.clone());
    let exercise = AssertUnwindSafe(async {
        let initial = fixture
            .store
            .load_user_session()
            .unwrap()
            .unwrap_or_default();
        later_than(initial.last_activity_ms).await;
        let first = fixture
            .rpc_exact(0, "qual/presence/first", "user.present", json!({}))
            .await;
        let first_saved = fixture.store.load_user_session().unwrap().unwrap();
        let first_native = native_session(&fixture);
        later_than(first_saved.last_activity_ms).await;
        let later = fixture
            .rpc_exact(1, "qual/presence/later", "user.present", json!({}))
            .await;
        let later_saved = fixture.store.load_user_session().unwrap().unwrap();
        let later_native = native_session(&fixture);
        let first_records = records.take("qual/presence/first");
        let later_records = records.take("qual/presence/later");
        (
            first,
            later,
            first_saved,
            later_saved,
            first_native,
            later_native,
            first_records,
            later_records,
        )
    })
    .catch_unwind()
    .await;
    let (
        first,
        later,
        first_saved,
        later_saved,
        first_native,
        later_native,
        first_records,
        later_records,
    ) = finish_caught(fixture, exercise).await;
    let first = first.unwrap();
    let later = later.unwrap();
    assert_eq!(first.reply["ok"], true);
    assert_eq!(later.reply["ok"], true);
    assert_eq!(session(&first.reply), first_saved);
    assert_eq!(session(&later.reply), later_saved);
    assert_eq!(first_native, (first_saved, first_saved));
    assert_eq!(later_native, (later_saved, later_saved));
    assert!(later_saved.last_activity_ms > first_saved.last_activity_ms);
    // Intended missing producer RED only after exact actual save checks.
    let first = only_presence(first_records);
    let later = only_presence(later_records);
    assert_eq!(
        first.len(),
        1,
        "actual successful Presence original lacks witness"
    );
    assert_eq!(later.len(), 1);
    assert_eq!(first[0].persistence, PresencePersistence::Saved);
    assert_eq!(first[0].local, first_saved);
    assert_eq!(first[0].published, first_saved);
    assert_eq!(first[0].durable_at_execution, Some(first_saved));
    assert_eq!(first[0].original_ids, vec!["qual/presence/first"]);
    assert_eq!(later[0].local, later_saved);
    assert_eq!(later[0].original_ids, vec!["qual/presence/later"]);
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failed_presence_save_stays_failed_after_later_real_success() {
    crate::git_fixture::environment::isolated_git_test!();
    let records = AcceptedRecords::new();
    let fixture = SavedFixture::new(records.clone());
    let exercise = AssertUnwindSafe(async {
        let before = fixture.store.load_user_session().unwrap();
        later_than(before.and_then(|s| s.last_activity_ms)).await;
        let failure = RejectUserSession::new(&fixture.store);
        let failed = fixture
            .rpc_exact(0, "qual/presence/failed", "user.present", json!({}))
            .await;
        let after_failure = fixture.store.load_user_session().unwrap();
        drop(failure); // All original failures collected before restoring native SQL.
        let (failed_local, failed_published) = native_session(&fixture);
        later_than(failed_local.last_activity_ms).await;
        let later = fixture
            .rpc_exact(1, "qual/presence/recovered", "user.present", json!({}))
            .await;
        let after_success = fixture.store.load_user_session().unwrap();
        let failed_records = records.take("qual/presence/failed");
        let later_records = records.take("qual/presence/recovered");
        (
            failed,
            later,
            before,
            after_failure,
            after_success,
            failed_local,
            failed_published,
            failed_records,
            later_records,
        )
    })
    .catch_unwind()
    .await;
    let (
        failed,
        later,
        before,
        after_failure,
        after_success,
        failed_local,
        failed_published,
        failed_records,
        later_records,
    ) = finish_caught(fixture, exercise).await;
    let failed = failed.unwrap();
    let later = later.unwrap();
    assert_eq!(
        failed.reply["ok"], true,
        "best-effort product wire unchanged"
    );
    assert_eq!(later.reply["ok"], true);
    assert_eq!(
        after_failure, before,
        "temporary real trigger refused original save"
    );
    assert_eq!(session(&failed.reply), failed_local);
    assert_ne!(Some(failed_local), before);
    assert_eq!(
        failed_published, failed_local,
        "failed native save preserves actual published memory"
    );
    assert_eq!(after_success, Some(session(&later.reply)));
    let failed = only_presence(failed_records);
    let later = only_presence(later_records);
    assert_eq!(
        failed.len(),
        1,
        "native failed save outcome must remain visible"
    );
    assert_eq!(later.len(), 1);
    assert_eq!(failed[0].persistence, PresencePersistence::SaveFailed);
    assert_eq!(failed[0].original_ids, vec!["qual/presence/failed"]);
    assert_eq!(later[0].original_ids, vec!["qual/presence/recovered"]);
    assert_eq!(failed[0].local, failed_local);
    assert_eq!(failed[0].published, failed_local);
    assert_eq!(
        failed[0].durable_at_execution, before,
        "later save must not rewrite original evidence"
    );
    assert_eq!(later[0].persistence, PresencePersistence::Saved);
}

struct NoopCase {
    label: &'static str,
    baseline: UserSession,
    changed: UserSession,
    stored: Option<UserSession>,
    local: UserSession,
    published: UserSession,
    covered_id: String,
    failed_id: String,
    gap_id: String,
    covered: Vec<AcceptedRecord>,
    failed: Vec<AcceptedRecord>,
    gap: Vec<AcceptedRecord>,
}

fn native_noop_case(label: &'static str, baseline: UserSession, changed_at: i64) -> NoopCase {
    let records = AcceptedRecords::new();
    let (directory, repo) = init_repo();
    let mut app = qa_state(&repo, directory.path());
    app.initialize_read_context();
    app.initialize_snapshot_projection();
    let store = app.store.as_ref().unwrap().clone();
    store.save_user_session(&baseline).unwrap();
    app.user_session = baseline;
    app.publish_read_user_session();
    app.test_capture_accepted_records(records.clone());
    seed_known_durable(&records, &store);
    let registry = WriteWaitRegistry::new();
    let covered_id = format!("qual/presence/{label}/covered");
    let failed_id = format!("qual/presence/{label}/failed");
    let gap_id = format!("qual/presence/{label}/uncovered");
    source_activity(
        &mut app,
        &registry,
        &covered_id,
        baseline.last_activity_ms.unwrap(),
    );
    let failure = RejectUserSession::new(&store);
    source_activity(&mut app, &registry, &failed_id, changed_at);
    drop(failure);
    source_activity(&mut app, &registry, &gap_id, changed_at);
    let evidence = NoopCase {
        label,
        baseline,
        changed: baseline.updated(changed_at),
        stored: store.load_user_session().unwrap(),
        local: app.user_session(),
        published: app
            .test_published_user_session()
            .expect("actual initialized read publication"),
        covered: records.take(&covered_id),
        failed: records.take(&failed_id),
        gap: records.take(&gap_id),
        covered_id,
        failed_id,
        gap_id,
    };
    drop(app);
    evidence
}

fn source_activity(
    app: &mut AppState,
    registry: &std::sync::Arc<WriteWaitRegistry>,
    original_id: &str,
    timestamp: i64,
) {
    let original = registry.begin(original_id, "user.present", Instant::now());
    original.admitted();
    {
        let _scope = original.context().bind_sync();
        app.test_note_presence_at(timestamp);
    }
    original.complete();
}

#[test]
fn native_presence_noop_uses_only_prior_exact_durable_coverage() {
    crate::git_fixture::environment::isolated_git_test!();
    // Deterministic native SOURCE units; these are not native-wire coverage.
    // Late activity changes each boundary field without advancing last_activity.
    let timestamp = 3 * crate::session_summary::USER_SESSION_GAP_MS;
    let baseline = UserSession::default().updated(timestamp);
    let cases = [
        native_noop_case("last_activity", baseline, timestamp + 1),
        native_noop_case("session_started", baseline, timestamp - 1),
        native_noop_case(
            "previous_session_ended",
            baseline,
            timestamp - crate::session_summary::USER_SESSION_GAP_MS,
        ),
    ];
    // All native preconditions are checked before the intended missing record RED.
    for case in &cases {
        assert_eq!(
            case.stored,
            Some(case.baseline),
            "{} actual trigger refusal",
            case.label
        );
        assert_eq!(case.local, case.changed);
        assert_eq!(case.published, case.local);
        let differences = [
            case.local.last_activity_ms != baseline.last_activity_ms,
            case.local.session_started_ms != baseline.session_started_ms,
            case.local.previous_session_ended_ms != baseline.previous_session_ended_ms,
        ];
        assert_eq!(
            differences.iter().filter(|changed| **changed).count(),
            1,
            "one independent session field must differ"
        );
        match case.label {
            "last_activity" => assert!(differences[0]),
            "session_started" => assert!(differences[1]),
            "previous_session_ended" => assert!(differences[2]),
            _ => unreachable!(),
        }
    }
    for case in cases {
        let covered = only_presence(case.covered);
        let failed = only_presence(case.failed);
        let gap = only_presence(case.gap);
        assert_eq!(covered.len(), 1, "native no-op lacks original observation");
        assert_eq!(failed.len(), 1);
        assert_eq!(gap.len(), 1);
        assert_eq!(covered[0].original_ids, vec![case.covered_id]);
        assert_eq!(failed[0].original_ids, vec![case.failed_id]);
        assert_eq!(gap[0].original_ids, vec![case.gap_id]);
        assert_eq!(covered[0].persistence, PresencePersistence::UnchangedNoSave);
        assert_eq!(covered[0].local, baseline);
        assert_eq!(covered[0].published, baseline);
        assert_eq!(covered[0].durable_at_execution, Some(baseline));
        assert_eq!(failed[0].persistence, PresencePersistence::SaveFailed);
        assert_eq!(failed[0].local, case.changed);
        assert_eq!(failed[0].published, case.changed);
        assert_eq!(failed[0].durable_at_execution, Some(baseline));
        assert_eq!(gap[0].persistence, PresencePersistence::UnchangedNoSave);
        assert_eq!(gap[0].local, case.changed);
        assert_eq!(gap[0].published, case.changed);
        assert_eq!(gap[0].durable_at_execution, Some(baseline));
        assert_ne!(
            gap[0].durable_at_execution,
            Some(gap[0].local),
            "all three native session fields must match prior durable coverage"
        );
    }
}

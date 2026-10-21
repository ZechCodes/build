//! Native-source accounting controls, not wire or predurable-refusal proof.
use super::*;
use crate::app::tests::runtime::write_wait_qualification::common_requests::ActualRequest;
use crate::app::tests::runtime::write_wait_qualification::trial_complete::{
    Envelope, JsonArtifacts,
};
use crate::app::tests::*;
use crate::timing::write_wait::WriteWaitRegistry;

/// Caller must enter isolated_git_test! at its unit-returning test entrypoint.
/// Construct through actual Store/collector/export; never forge token fields.
pub(in super::super) fn empty_rejections_control(originals: &[&str]) -> AcceptedRefusals {
    let (directory, repo) = init_repo();
    let mut app = qa_state(&repo, directory.path());
    app.initialize_read_context();
    let store = app.store.as_ref().unwrap().clone();
    let records = AcceptedRecords::new();
    records.seed_known_durable_user_session(store.load_user_session().unwrap());
    app.test_capture_accepted_records(Arc::clone(&records));
    let archive = AcceptedArchive::new(
        Arc::clone(&records),
        originals.iter().map(|id| (*id).to_owned()).collect(),
    );
    for original in originals {
        assert!(archive.take_rejected(original).unwrap().is_empty());
    }
    let artifact_root = directory.path().join("empty-rejections-control");
    let sink = JsonArtifacts::new(&artifact_root).unwrap();
    let name = trial_name();
    let result = archive.finish(name, &sink, true);
    assert!(result.persisted && result.errors.is_empty());
    let raw: Value = serde_json::from_slice(
        &std::fs::read(
            artifact_root
                .join(name.label())
                .join("accepted-records-final.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(raw["histories"].as_object().unwrap().len(), originals.len());
    for original in originals {
        assert_eq!(raw["histories"][*original]["attempts"], json!([[]]));
    }
    assert!(records.take_all().is_empty());
    result
        .refused
        .expect("quiet persisted empty acceptance audit")
}

#[test]
fn native_rejected_acceptance_requires_quiet_persisted_final_audit() {
    crate::git_fixture::environment::isolated_git_test!();
    let original = "qual/rejected/empty";
    let (passed, raw) = finish_empty_candidate(original, true, false);
    assert!(passed.persisted && passed.errors.is_empty());
    assert_eq!(raw["histories"][original]["attempts"], json!([[]]));
    let token = passed
        .refused
        .expect("quiet persisted empty acceptance audit");
    assert_eq!(token.originals().collect::<Vec<_>>(), [original]);
    assert!(token.contains(original));
    assert!(!token.contains(&format!("{original}/accepted-control")));

    let (not_quiet, raw) = finish_empty_candidate("qual/rejected/not-quiet", false, false);
    assert!(not_quiet.persisted);
    assert!(not_quiet.refused.is_none());
    assert!(not_quiet
        .errors
        .iter()
        .any(|error| error.contains("not quiet")));
    assert_eq!(raw["native_quiet"], false);

    let (failed_sink, raw) = finish_empty_candidate("qual/rejected/failed-sink", true, true);
    assert!(!failed_sink.persisted);
    assert!(failed_sink.refused.is_none());
    assert!(failed_sink
        .errors
        .iter()
        .any(|error| error.contains("accepted-record artifact")));
    assert_eq!(
        raw["histories"]["qual/rejected/failed-sink"]["attempts"],
        json!([[]])
    );
}

#[test]
fn native_rejected_acceptance_preserves_nonempty_duplicate_late_and_unknown_records() {
    crate::git_fixture::environment::isolated_git_test!();
    let (directory, repo) = init_repo();
    let mut app = qa_state(&repo, directory.path());
    app.initialize_read_context();
    let store = app.store.as_ref().unwrap().clone();
    let records = AcceptedRecords::new();
    records.seed_known_durable_user_session(store.load_user_session().unwrap());
    app.test_capture_accepted_records(Arc::clone(&records));
    let registry = WriteWaitRegistry::new();
    let saved = "qual/rejected/has-native-saves";
    let duplicate = "qual/rejected/duplicate-take";
    let late = "qual/rejected/late-save";
    let unknown = "qual/rejected/unknown-save";
    let archive = AcceptedArchive::new(
        Arc::clone(&records),
        [saved, duplicate, late]
            .into_iter()
            .map(str::to_owned)
            .collect(),
    );
    let initial = app.user_session().last_activity_ms.unwrap_or(0) + 1;
    native_activity(&mut app, &registry, saved, initial);
    native_activity(&mut app, &registry, saved, initial + 1);
    assert_eq!(store.load_user_session().unwrap(), Some(app.user_session()));
    assert!(archive
        .take_rejected(saved)
        .unwrap_err()
        .contains("2 accepted records"));
    assert!(archive.take_rejected(duplicate).unwrap().is_empty());
    assert!(archive
        .take_rejected(duplicate)
        .unwrap_err()
        .contains("more than once"));
    assert!(archive.take_rejected(late).unwrap().is_empty());
    assert!(archive
        .take_rejected("qual/rejected/unplanned-take")
        .unwrap_err()
        .contains("unplanned"));
    native_activity(&mut app, &registry, late, initial + 2);
    native_activity(&mut app, &registry, unknown, initial + 3);
    assert_eq!(store.load_user_session().unwrap(), Some(app.user_session()));

    let artifact_root = directory.path().join("retained-rejected-records");
    let sink = JsonArtifacts::new(&artifact_root).unwrap();
    let name = trial_name();
    let result = archive.finish(name, &sink, true);
    assert!(result.persisted);
    assert!(
        result.refused.is_none(),
        "any retained audit error blocks the token"
    );
    for original in [
        saved,
        duplicate,
        late,
        unknown,
        "qual/rejected/unplanned-take",
    ] {
        assert!(
            result.errors.iter().any(|error| error.contains(original)),
            "missing audit error for {original}"
        );
    }
    let raw: Value = serde_json::from_slice(
        &std::fs::read(
            artifact_root
                .join(name.label())
                .join("accepted-records-final.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        raw["histories"][saved]["attempts"][0]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(raw["histories"][duplicate]["attempts"], json!([[], []]));
    assert_eq!(raw["late_or_unknown"][late].as_array().unwrap().len(), 1);
    assert_eq!(raw["late_or_unknown"][unknown].as_array().unwrap().len(), 1);
    assert_eq!(raw["histories"][saved]["classifications"], json!([]));
    assert!(!serde_json::to_string(&raw["histories"])
        .unwrap()
        .contains("native saved effect and completion verified"));
    assert!(records.take_all().is_empty());
}

fn finish_empty_candidate(
    original: &str,
    native_quiet: bool,
    block_sink: bool,
) -> (AcceptedExport, Value) {
    let (directory, repo) = init_repo();
    let mut app = qa_state(&repo, directory.path());
    app.initialize_read_context();
    let store = app.store.as_ref().unwrap().clone();
    let records = AcceptedRecords::new();
    records.seed_known_durable_user_session(store.load_user_session().unwrap());
    app.test_capture_accepted_records(Arc::clone(&records));
    let accepted = format!("{original}/accepted-control");
    let archive = AcceptedArchive::new(
        Arc::clone(&records),
        [original.to_owned(), accepted.clone()].into(),
    );
    let registry = WriteWaitRegistry::new();
    let initial = app.user_session().last_activity_ms.unwrap_or(0) + 1;
    native_activity(&mut app, &registry, &accepted, initial);
    assert_eq!(store.load_user_session().unwrap(), Some(app.user_session()));
    let captured = archive.take(&presence_request(&accepted)).unwrap();
    assert_eq!(
        captured.len(),
        1,
        "actual native save must be captured normally"
    );
    assert!(archive.take_rejected(original).unwrap().is_empty());

    let artifact_root = directory.path().join("empty-rejection-artifacts");
    let sink = JsonArtifacts::new(&artifact_root).unwrap();
    let name = trial_name();
    let trial_dir = artifact_root.join(name.label());
    let destination = trial_dir.join("accepted-records-final.json");
    if block_sink {
        std::fs::create_dir(&trial_dir).unwrap();
        std::fs::write(&destination, b"existing artifact must survive").unwrap();
    }
    let result = archive.finish(name, &sink, native_quiet);
    let raw = if result.persisted {
        serde_json::from_slice(&std::fs::read(&destination).unwrap()).unwrap()
    } else {
        assert_eq!(
            std::fs::read(&destination).unwrap(),
            b"existing artifact must survive"
        );
        archive
            .retained
            .lock()
            .unwrap()
            .failed_export
            .clone()
            .expect("failed export remains owned")
    };
    assert_eq!(
        raw["histories"][accepted]["attempts"][0],
        serde_json::to_value(captured).unwrap()
    );
    assert!(records.take_all().is_empty());
    (result, raw)
}

fn presence_request(original: &str) -> CommonRequest {
    CommonRequest {
        original_id: original.into(),
        caller: 0,
        owner: 0,
        request: ActualRequest::Rpc {
            method: "user.present",
            params: json!({}),
        },
        effect: ExpectedEffect::UserPresent,
        durable_operation_id: None,
        predecessor: None,
    }
}

fn native_activity(app: &mut AppState, registry: &Arc<WriteWaitRegistry>, original: &str, ts: i64) {
    let root = registry.begin(original, "user.present", registry.now());
    root.admitted();
    {
        let _binding = root.context().bind_sync();
        app.test_note_presence_at(ts);
    }
    root.complete();
}

fn trial_name() -> TrialName {
    TrialName {
        envelope: Envelope::Small,
        number: 1,
    }
}

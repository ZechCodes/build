//! Existing common-write evidence ownership; no AppState access after setup.
use super::super::common_requests::{CommonRequest, ExpectedEffect};
use super::super::fixture::QualificationFixture;
use super::super::rpc::ReplyProof;
use super::super::trial_complete::{ArtifactSink, OracleWitness, TrialName};
use super::completed_origins::NativeCompletion;
use super::saved_effects::{self, Handles, InvocationWindow};
use crate::app::accepted_records::{AcceptedRecord, AcceptedRecords, PresencePersistence};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

pub(super) struct SavedEvidence {
    handles: Handles,
    pub archive: AcceptedArchive,
}
impl SavedEvidence {
    pub fn capture(
        fixture: &QualificationFixture,
        expected: BTreeSet<String>,
    ) -> Result<Arc<Self>, String> {
        let (store, service, addresses) = {
            let app = fixture
                .state
                .lock()
                .map_err(|_| "fixture app lock poisoned")?;
            let addresses = fixture
                .active_workspaces
                .iter()
                .map(|workspace| {
                    Ok((
                        workspace.agent_id.clone(),
                        app.live_mcp_issued_address(&workspace.owner, &workspace.agent_id)?,
                    ))
                })
                .collect::<Result<BTreeMap<_, _>, String>>()?;
            (
                app.store.as_ref().ok_or("actual Store missing")?.clone(),
                app.read_service(),
                addresses,
            )
        };
        if fixture.logs.len() != fixture.active_workspaces.len()
            || addresses.len() != fixture.logs.len()
        {
            return Err("actual SessionLog/address cardinality differs".into());
        }
        let logs = fixture
            .active_workspaces
            .iter()
            .zip(&fixture.logs)
            .map(|(workspace, log)| (workspace.agent_id.clone(), log.clone()))
            .collect();
        let records = AcceptedRecords::new();
        // No AppState guard while loading the authoritative setup row.
        records.seed_known_durable_user_session(
            store
                .load_user_session()
                .map_err(|error| error.to_string())?,
        );
        fixture
            .state
            .lock()
            .map_err(|_| "fixture app lock poisoned")?
            .test_capture_accepted_records(Arc::clone(&records));
        let pool = store.read_pool();
        Ok(Arc::new(Self {
            handles: Handles {
                store,
                pool,
                service,
                addresses,
                logs,
            },
            archive: AcceptedArchive::new(records, expected),
        }))
    }

    pub async fn verify(
        &self,
        request: &CommonRequest,
        reply: &ReplyProof,
        window: InvocationWindow,
        completion: &NativeCompletion,
        records: &[AcceptedRecord],
    ) -> Result<OracleWitness, String> {
        // Required for every effect, including Agent/Task and finite writes.
        saved_effects::check_original_complete(request, reply, completion)?;
        match &request.effect {
            ExpectedEffect::Posted { .. }
            | ExpectedEffect::Handoff { .. }
            | ExpectedEffect::Seen { .. }
            | ExpectedEffect::UserPresent => {
                saved_effects::verify(request, reply, &self.handles, window, completion, records)
                    .await
            }
            ExpectedEffect::Topic { .. }
            | ExpectedEffect::Name { .. }
            | ExpectedEffect::TaskStatus { .. } => {
                request.verify_accepted_record(&reply.reply, selected_record(request, records)?)
            }
            ExpectedEffect::Comment { .. } => {
                let record = selected_record(request, records)?;
                let id = reply.reply["result"]["comment"]["id"]
                    .as_str()
                    .ok_or("actual comment reply lacks immutable ID")?
                    .to_owned();
                let store = self.handles.store.clone();
                let comment = tokio::task::spawn_blocking(move || {
                    store
                        .load_tracker_comment(&id)
                        .map_err(|error| error.to_string())?
                        .ok_or_else(|| "acknowledged native comment missing".to_owned())
                })
                .await
                .map_err(|error| error.to_string())??;
                request.verify_accepted_comment(&reply.reply, record, comment)
            }
            ExpectedEffect::File { .. } | ExpectedEffect::GitHead { .. } => {
                Err("finite effect uses existing exact file/ref oracle".into())
            }
        }
    }
}

#[derive(Default, Serialize)]
struct RecordHistory {
    attempts: Vec<Vec<AcceptedRecord>>,
    classifications: Vec<Vec<String>>,
    outcomes: Vec<String>,
}
#[derive(Default)]
struct Retained {
    histories: BTreeMap<String, RecordHistory>,
    late: BTreeMap<String, Vec<AcceptedRecord>>,
    rejected: BTreeSet<String>,
    errors: Vec<String>,
    final_attempted: bool,
    failed_export: Option<Value>,
}
pub(super) struct AcceptedArchive {
    records: Arc<AcceptedRecords>,
    expected: BTreeSet<String>,
    retained: Mutex<Retained>,
}
/// Only this module can certify persisted empty accepted-record evidence.
/// Native cancellation, exact busy protocol and zero execution remain separate
/// prerequisites; this token alone is not a verified frontdoor refusal.
pub(super) struct AcceptedRefusals {
    originals: BTreeSet<String>,
}
impl AcceptedRefusals {
    pub(super) fn contains(&self, original: &str) -> bool {
        self.originals.contains(original)
    }
    pub(super) fn originals(&self) -> impl Iterator<Item = &str> {
        self.originals.iter().map(String::as_str)
    }
}
pub(super) struct AcceptedExport {
    pub persisted: bool,
    pub errors: Vec<String>,
    pub refused: Option<AcceptedRefusals>,
}
impl AcceptedArchive {
    fn new(records: Arc<AcceptedRecords>, expected: BTreeSet<String>) -> Self {
        Self {
            records,
            expected,
            retained: Mutex::new(Retained::default()),
        }
    }
    /// Called exactly once for each dispatched original, even when reply or
    /// completion failed. Preserve raw before any selection/oracle can fail.
    pub fn take(&self, request: &CommonRequest) -> Result<Vec<AcceptedRecord>, String> {
        self.take_classified(&request.original_id, false, |rows| classify(request, rows))
    }
    /// Only the overload path calls this for an actual typed busy candidate.
    /// Empty evidence is provisional until the final native-quiet audit and
    /// persisted archive; preserve nonempty vectors before returning any error.
    pub fn take_rejected(&self, original: &str) -> Result<Vec<AcceptedRecord>, String> {
        self.take_classified(original, true, |rows| {
            if rows.is_empty() {
                Ok(vec![
                    "refusal-candidate/accepted-records-empty/final-audit-pending".into(),
                ])
            } else {
                Err(format!(
                    "refusal candidate has {} accepted records",
                    rows.len()
                ))
            }
        })
    }
    fn take_classified(
        &self,
        original: &str,
        rejected: bool,
        classify: impl FnOnce(&[AcceptedRecord]) -> Result<Vec<String>, String>,
    ) -> Result<Vec<AcceptedRecord>, String> {
        let rows = self.records.take(original);
        let mut retained = self.retained.lock().unwrap();
        let history = retained.histories.entry(original.to_owned()).or_default();
        history.attempts.push(rows.clone());
        let result = if history.attempts.len() != 1 {
            Err("accepted evidence taken more than once for original".into())
        } else if !self.expected.contains(original) {
            Err("accepted evidence taken for unplanned original".into())
        } else {
            classify(&rows)
        };
        match result {
            Ok(classifications) => {
                history.classifications.push(classifications);
                if rejected {
                    history.outcomes.push("overload refusal candidate: actual acceptance vector empty; final audit pending".into());
                    retained.rejected.insert(original.to_owned());
                }
                Ok(rows)
            }
            Err(error) => {
                history.outcomes.push(error.clone());
                retained.errors.push(format!("{original}: {error}"));
                Err(error)
            }
        }
    }
    pub fn outcome(&self, original: &str, result: Result<(), String>) {
        let mut retained = self.retained.lock().unwrap();
        let history = retained.histories.entry(original.to_owned()).or_default();
        history.outcomes.push(match result {
            Ok(()) => "native saved effect and completion verified".into(),
            Err(error) => error,
        });
    }
    pub fn finish(
        &self,
        name: TrialName,
        sink: &dyn ArtifactSink,
        native_quiet: bool,
    ) -> AcceptedExport {
        self.export(native_quiet, |raw| sink.accepted_records_archive(name, raw))
    }
    fn export(
        &self,
        native_quiet: bool,
        write: impl FnOnce(&Value) -> Result<(), String>,
    ) -> AcceptedExport {
        let late = self.records.take_all();
        let mut retained = self.retained.lock().unwrap();
        for (original, records) in late {
            retained.errors.push(format!(
                "late or unknown accepted records for {original}: {}",
                records.len()
            ));
            retained.late.entry(original).or_default().extend(records);
        }
        for original in &self.expected {
            if retained
                .histories
                .get(original)
                .is_none_or(|row| row.attempts.len() != 1)
            {
                retained.errors.push(format!(
                    "expected exactly one accepted evidence take for {original}"
                ));
            }
        }
        if !native_quiet {
            retained
                .errors
                .push("native close not quiet at accepted-record audit".into());
        }
        if retained.final_attempted {
            retained
                .errors
                .push("accepted final export attempted twice".into());
        }
        retained.final_attempted = true;
        let raw = json!({"schema":"build.native-accepted-records-final.v1", "qualification":false,
            "native_quiet":native_quiet, "expected_originals":self.expected,
            "histories":retained.histories, "late_or_unknown":retained.late,
            "refusal_candidates":retained.rejected, "errors":retained.errors});
        let persisted = match write(&raw) {
            Ok(()) => true,
            Err(error) => {
                retained.failed_export = Some(raw);
                retained
                    .errors
                    .push(format!("accepted-record artifact: {error}"));
                false
            }
        };
        let refused =
            (persisted && native_quiet && retained.errors.is_empty()).then(|| AcceptedRefusals {
                originals: retained.rejected.clone(),
            });
        AcceptedExport {
            persisted,
            errors: retained.errors.clone(),
            refused,
        }
    }
}

fn requested_kind(request: &CommonRequest) -> Option<&'static str> {
    match request.effect {
        ExpectedEffect::Topic { .. } | ExpectedEffect::Name { .. } => Some("Agent"),
        ExpectedEffect::TaskStatus { .. } | ExpectedEffect::Comment { .. } => Some("Task"),
        ExpectedEffect::Seen { .. } => Some("Seen"),
        ExpectedEffect::UserPresent => Some("Presence"),
        _ => None,
    }
}
fn record_kind(record: &AcceptedRecord) -> (&'static str, &[String]) {
    match record {
        AcceptedRecord::Agent(row) => ("Agent", &row.original_ids),
        AcceptedRecord::Task(row) => ("Task", &row.original_ids),
        AcceptedRecord::Seen(row) => ("Seen", &row.original_ids),
        AcceptedRecord::Presence(row) => ("Presence", &row.original_ids),
    }
}
fn classify(request: &CommonRequest, records: &[AcceptedRecord]) -> Result<Vec<String>, String> {
    let expected = requested_kind(request);
    let mut requested = 0;
    let mut labels = Vec::new();
    for record in records {
        let (kind, originals) = record_kind(record);
        if originals.len() != 1 || originals[0] != request.original_id {
            return Err("accepted record must name exact singleton original".into());
        }
        if Some(kind) == expected {
            requested += 1;
            labels.push(format!("requested/{kind}"));
        } else if let AcceptedRecord::Presence(row) = record {
            // Incidental best-effort activity never establishes primary proof.
            // A failed/no-op coverage gap is retained as that exact disposition.
            if row.local != row.published
                || (row.persistence == PresencePersistence::Saved
                    && row.durable_at_execution != Some(row.local))
            {
                return Err(
                    "incidental Presence disagrees with its actual native disposition".into(),
                );
            }
            labels.push(format!(
                "incidental/Presence/{:?}/durable_coverage={}/primary_credit=false",
                row.persistence,
                row.durable_at_execution == Some(row.local)
            ));
        } else {
            return Err(format!("unexpected unrelated {kind} under common original"));
        }
    }
    if requested != usize::from(expected.is_some()) {
        return Err(format!(
            "requested {expected:?} record count {requested} differs"
        ));
    }
    Ok(labels)
}
fn selected_record(
    request: &CommonRequest,
    records: &[AcceptedRecord],
) -> Result<AcceptedRecord, String> {
    classify(request, records)?;
    let expected = requested_kind(request);
    records
        .iter()
        .find(|record| Some(record_kind(record).0) == expected)
        .cloned()
        .ok_or_else(|| "requested native record missing".into())
}

#[cfg(test)]
mod tests {
    use super::super::super::common_requests::ActualRequest;
    use super::super::super::trial_complete::{Envelope, JsonArtifacts};
    use super::*;
    use crate::app::tests::*;
    use crate::timing::write_wait::WriteWaitRegistry;

    #[test]
    fn native_accepted_final_audit_retains_late_unknown_and_failed_export() {
        crate::git_fixture::environment::isolated_git_test!();
        // Native-source retention control; does not claim socket proof.
        let (directory, repo) = init_repo();
        let mut app = qa_state(&repo, directory.path());
        app.initialize_read_context();
        let store = app.store.as_ref().unwrap().clone();
        let records = AcceptedRecords::new();
        records.seed_known_durable_user_session(store.load_user_session().unwrap());
        app.test_capture_accepted_records(Arc::clone(&records));
        let registry = WriteWaitRegistry::new();
        let id = "qual/accepted/native-initial";
        let request = CommonRequest {
            original_id: id.into(),
            caller: 0,
            owner: 0,
            request: ActualRequest::Rpc {
                method: "user.present",
                params: json!({}),
            },
            effect: ExpectedEffect::UserPresent,
            durable_operation_id: None,
            predecessor: None,
        };
        let archive = AcceptedArchive::new(Arc::clone(&records), [id.to_owned()].into());
        let initial = app.user_session().last_activity_ms.unwrap_or(0) + 1;
        native_activity(&mut app, &registry, id, initial);
        assert_eq!(store.load_user_session().unwrap(), Some(app.user_session()));
        let first = archive.take(&request).unwrap();
        assert_eq!(first.len(), 1);
        archive.outcome(id, Err("deliberate earlier proof failure".into()));
        native_activity(&mut app, &registry, id, initial + 1);
        native_activity(
            &mut app,
            &registry,
            "qual/accepted/unplanned-native",
            initial + 2,
        );
        assert_eq!(store.load_user_session().unwrap(), Some(app.user_session()));

        // Real ArtifactSink create_new refusal: an existing artifact must survive.
        let artifact_root = directory.path().join("accepted-artifacts");
        let sink = JsonArtifacts::new(&artifact_root).unwrap();
        let name = TrialName {
            envelope: Envelope::Small,
            number: 1,
        };
        let trial_dir = artifact_root.join(name.label());
        std::fs::create_dir(&trial_dir).unwrap();
        let blocked = trial_dir.join("accepted-records-final.json");
        std::fs::write(&blocked, b"existing artifact must survive").unwrap();
        let result = archive.finish(name, &sink, true);
        assert!(!result.persisted);
        assert!(result
            .errors
            .iter()
            .any(|error| error.contains("accepted-record artifact")));
        let retained = archive.retained.lock().unwrap();
        let raw = retained
            .failed_export
            .as_ref()
            .expect("failed export must stay owned");
        assert_eq!(
            raw["histories"][id]["attempts"][0],
            serde_json::to_value(&first).unwrap()
        );
        assert_eq!(
            raw["late_or_unknown"][id].as_array().map_or(0, Vec::len),
            1,
            "actual final audit lost late native record after earlier proof failure"
        );
        assert_eq!(
            raw["late_or_unknown"]["qual/accepted/unplanned-native"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(retained.histories[id].attempts.len(), 1);
        assert!(retained.histories[id]
            .outcomes
            .iter()
            .any(|outcome| outcome == "deliberate earlier proof failure"));
        assert_eq!(
            std::fs::read(&blocked).unwrap(),
            b"existing artifact must survive"
        );
        assert!(records.take_all().is_empty());
    }
    fn native_activity(app: &mut AppState, registry: &Arc<WriteWaitRegistry>, id: &str, ts: i64) {
        let root = registry.begin(id, "user.present", registry.now());
        root.admitted();
        {
            let _binding = root.context().bind_sync();
            app.test_note_presence_at(ts);
        }
        root.complete();
    }
}

#[cfg(test)]
#[path = "saved_evidence/rejection_tests.rs"]
mod rejection_tests;
#[cfg(test)]
pub(super) use rejection_tests::empty_rejections_control;

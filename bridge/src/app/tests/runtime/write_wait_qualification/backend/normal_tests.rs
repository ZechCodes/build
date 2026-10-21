//! Native setup controls, never a replacement or shortened normal original.
use super::super::read_recipes::{mcp_cases, mcp_read_method, mcp_recipe_form, FullReadForm};
use super::super::trial_complete::{ReadCohort, Route, Traffic};
use super::*;
use futures_util::FutureExt;
use serde_json::{json, Value};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn small_native_pre_t0_counts_bind_measured_state_before_any_enrollment() {
    crate::git_fixture::environment::isolated_git_test!();
    counts_control(Envelope::Small).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "native large-envelope setup count control; run serial at NI10 before six originals"]
async fn large_native_pre_t0_counts_bind_measured_state_before_any_enrollment() {
    crate::git_fixture::environment::isolated_git_test!();
    counts_control(Envelope::Large).await;
}

async fn counts_control(envelope: Envelope) {
    let artifacts = tempfile::Builder::new()
        .prefix("qualification-native-counts-NoQualification-")
        .tempdir()
        .unwrap()
        .keep();
    eprintln!("native_count_control_artifacts={}", artifacts.display());
    let mut prepared = PreparedBackend::unassembled(
        TrialName {
            envelope,
            number: 1,
        },
        PipelineMode::Qualification,
        provenance(),
    )
    .await;
    let measured = std::panic::AssertUnwindSafe(async {
        prepared.assemble_support().await?;
        if prepared.watch.is_some() || prepared.lifecycle.is_some() {
            return Err("counts must be measured before any caller enrollment".into());
        }
        let actual = prepared
            .fixture
            .as_ref()
            .ok_or("owned native fixture missing")?
            .qualification_count_receipt();
        let declarations = prepared.provenance.fixture_counts.clone();
        let binding = prepared.bind_native_fixture_counts();
        let bound = prepared.provenance.fixture_counts.clone();
        let index = usize::from(envelope == Envelope::Large);
        prepared.provenance.fixture_counts = declarations.clone();
        prepared.provenance.fixture_counts["expected_envelopes"][index]["thread_items"] = json!(0);
        let mismatch = prepared.bind_native_fixture_counts();
        save(
            &artifacts.join("measured-binding.json"),
            &json!({
                "qualification":false,"actual":actual,"declared":declarations,
                "bound":bound,"binding":binding,"intentional_mismatch":mismatch,
            }),
        )?;
        Ok::<_, String>((actual, bound, binding, mismatch))
    })
    .catch_unwind()
    .await;
    // Measurements and binding outcomes above are immutable before this
    // explicit fixture-service proof; it is not a normal scheduled original.
    let service = exercise_github_cleanup_prerequisite(&prepared, &artifacts).await;
    let cleanup = close_retained(prepared).await;
    save(
        &artifacts.join("cleanup.json"),
        &json!({"result":cleanup,"qualification":false}),
    )
    .unwrap();
    cleanup.expect("actual native setup resources must close before count assertions");
    service.expect("actual full-read fixture service proof before cleanup");
    let (actual, bound, binding, mismatch) =
        measured.expect("native count collection panicked").unwrap();
    binding.expect("normal provenance must bind measured native pre-T0 counts");
    assert_eq!(bound["actual_pre_t0"], actual);
    assert!(bound["supplied_contract"].is_object());
    assert!(
        mismatch.is_err(),
        "wrong declaration must fail against real measurements"
    );
    assert_eq!(actual["projects"], 4);
    assert_eq!(actual["active_workspaces"], 8);
    assert_eq!(actual["live_sessions"], 8);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_normal_native_read_recipe_has_exact_typed_cohort_before_t0() {
    crate::git_fixture::environment::isolated_git_test!();
    let artifacts = tempfile::Builder::new()
        .prefix("qualification-native-forms-NoQualification-")
        .tempdir()
        .unwrap()
        .keep();
    eprintln!("native_form_control_artifacts={}", artifacts.display());
    let mut prepared = PreparedBackend::unassembled(
        TrialName {
            envelope: Envelope::Small,
            number: 1,
        },
        PipelineMode::Qualification,
        provenance(),
    )
    .await;
    let observed = std::panic::AssertUnwindSafe(async {
        prepared.assemble_support().await?;
        let mut outcomes = Vec::new();
        for case in &prepared.rpc_cases {
            let slot = planned_read(&prepared, Route::SpaRpc, case.method)?;
            outcomes.push((
                case.method.to_owned(),
                case.form,
                control::expected_read_cohort(&prepared, slot),
            ));
        }
        for (_, action) in mcp_cases(&prepared.mcp_inputs[0].0) {
            let method = mcp_read_method(&action);
            let slot = planned_read(&prepared, Route::AgentMcp, &method)?;
            let form = mcp_recipe_form(&action).ok_or("native MCP recipe form missing")?;
            outcomes.push((method, form, control::expected_read_cohort(&prepared, slot)));
        }
        let counts = [Traffic::Write, Traffic::Read, Traffic::Exempt].map(|traffic| {
            prepared
                .plan
                .iter()
                .filter(|slot| slot.traffic == traffic)
                .count()
        });
        let mut unknown = prepared
            .plan
            .iter()
            .find(|slot| slot.traffic == Traffic::Read)
            .unwrap()
            .clone();
        unknown.method = Some("unknown.native.read".into());
        let rejected = control::expected_read_cohort(&prepared, &unknown).is_err();
        save(
            &artifacts.join("measured-forms.json"),
            &json!({
                "qualification":false,"counts":counts,"unknown_rejected":rejected,
                "outcomes":outcomes.iter().map(|(method, form, actual)| json!({
                    "method":method,"form":format!("{form:?}"),"actual":actual
                })).collect::<Vec<_>>()
            }),
        )?;
        Ok::<_, String>((outcomes, counts, rejected))
    })
    .catch_unwind()
    .await;
    let service = exercise_github_cleanup_prerequisite(&prepared, &artifacts).await;
    let cleanup = close_retained(prepared).await;
    save(
        &artifacts.join("cleanup.json"),
        &json!({"result":cleanup,"qualification":false}),
    )
    .unwrap();
    cleanup.expect("actual native setup cleanup before typed-form assertions");
    service.expect("actual full-read fixture service proof before cleanup");
    let (outcomes, counts, rejected) = observed
        .expect("native read classification collection panicked")
        .unwrap();
    assert_eq!(counts, [24_000, 59_000, 60_000]);
    assert!(rejected, "an unknown form cannot earn Ready credit");
    assert_eq!(outcomes.len(), 59);
    let errors = outcomes
        .into_iter()
        .filter_map(|(method, form, actual)| {
            let expected = match form {
                FullReadForm::Ready => ReadCohort::ReadyBounded,
                FullReadForm::LegacyFull => ReadCohort::LegacyUnbounded,
            };
            (actual.as_ref().ok() != Some(&Some(expected.clone())))
                .then(|| format!("{method}: expected {expected:?}, actual {actual:?}"))
        })
        .collect::<Vec<_>>();
    assert!(
        errors.is_empty(),
        "native typed read recipe gaps: {errors:?}"
    );
}

fn planned_read<'a>(
    prepared: &'a PreparedBackend,
    route: Route,
    method: &str,
) -> Result<&'a Slot, String> {
    prepared
        .plan
        .iter()
        .find(|slot| {
            slot.traffic == Traffic::Read
                && slot.route == route
                && slot.method.as_deref() == Some(method)
        })
        .ok_or_else(|| format!("native read slot missing: {method}"))
}

// Full ReadyReadInputs requires evidence that its isolated GitHub stand-in
// was exercised. Setup-only controls must obtain that evidence from the actual
// native endpoint, after their pre-T0 measurement/classification boundary.
// This is fixture-prerequisite evidence, never a normal original or timing pass.
async fn exercise_github_cleanup_prerequisite(
    prepared: &PreparedBackend,
    artifacts: &std::path::Path,
) -> Result<(), String> {
    let result = std::panic::AssertUnwindSafe(async {
        let case = prepared
            .rpc_cases
            .iter()
            .find(|case| case.method == "github.repos")
            .ok_or("native GitHub read prerequisite missing")?;
        let fixture = prepared.fixture.as_ref().ok_or("native fixture missing")?;
        let reads = prepared.reads.as_deref().ok_or("native read witnesses missing")?;
        let id = "fixture-prerequisite/after-pre-t0/github.repos";
        let reply = fixture.rpc.request(0, id, case.method, case.params.clone()).await;
        save(
            &artifacts.join("fixture-service-reply.json"),
            &json!({
                "qualification":false,"normal_original":false,
                "boundary":"after pre-T0 measurement/classification; before native cleanup",
                "original_id":reply.original_id,"method":reply.method,"reply":reply.reply,
                "elapsed_us":reply.final_reply_at.saturating_duration_since(reply.sent_at).as_micros()
            }),
        )?;
        if reply.original_id != id || reply.reply["id"] != id || reply.method != case.method {
            return Err("native fixture-service reply identity mismatch".into());
        }
        let outcome = reads.assert_rpc_reply(case, &reply.reply);
        if outcome != super::super::read_recipes::FullReadOutcome::Ready {
            return Err(format!("native GitHub prerequisite was not Ready: {outcome:?}"));
        }
        Ok::<(), String>(())
    })
    .catch_unwind()
    .await
    .unwrap_or_else(|panic| {
        let detail = panic
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| panic.downcast_ref::<&str>().map(|text| (*text).to_owned()))
            .unwrap_or_else(|| "non-string native fixture-service panic".into());
        Err(detail)
    });
    save(
        &artifacts.join("fixture-service-result.json"),
        &json!({"qualification":false,"result":result}),
    )?;
    result
}

async fn close_retained(prepared: PreparedBackend) -> Result<(), String> {
    match prepared.shutdown().await {
        Ok(evidence)
            if evidence.producers_joined
                && evidence.mcp_quiet
                && evidence.client_watch_joined
                && evidence.native_cleanup_completed =>
        {
            Ok(())
        }
        Ok(_) => Err("native setup cleanup evidence incomplete".into()),
        Err(failure) => {
            let reason = failure.reason;
            std::mem::forget(failure.retained);
            Err(reason)
        }
    }
}

fn save(path: &std::path::Path, value: &Value) -> Result<(), String> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| error.to_string())?;
    serde_json::to_writer_pretty(&mut file, value).map_err(|error| error.to_string())?;
    file.write_all(b"\n").map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())
}

fn provenance() -> Provenance {
    let mut provenance = super::preparation_tests::marker();
    provenance.fixture_counts = json!({"qualification":true,"expected_envelopes":[
        {"label":"small","projects":4,"tracker_tasks":100,"registry_workspaces":10,"active_workspaces":8,"canonical_histories":16,"thread_items":1000},
        {"label":"large","projects":4,"tracker_tasks":10000,"registry_workspaces":200,"active_workspaces":8,"canonical_histories":206,"thread_items":100000}
    ]});
    provenance
}

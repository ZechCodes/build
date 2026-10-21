//! Explicit native adapter control, not a shortened qualification original.
//! Only 59 derived read IDs are enrolled; their full normal source slots remain
//! in the artifact. The actual backend proof helper runs unchanged.
use super::super::client_watch::{ClientWatch, FinalOutcome};
use super::super::read_recipes::{FullReadForm, mcp_cases, mcp_read_method, mcp_recipe_form};
use super::super::trial_complete::{
    ControlExecution, ExecutionProof, JsonArtifacts, Observation, ProducerHandle, ReadCohort,
    Recipe, Route, Stage, Traffic,
};
use super::*;
use futures_util::FutureExt;
use serde_json::json;
use std::collections::BTreeSet;
use std::path::Path;
use std::time::{Duration, Instant};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "native59 proof adapter readiness control; explicit quiet release NI10 only, never debug-parallel timing"]
async fn actual_native59_read_proofs_keep_typed_forms_identity_and_100ms_deadlines() {
    assert!(
        !cfg!(debug_assertions),
        "native59 timing requires release-opt3"
    );
    crate::git_fixture::environment::isolated_git_test!();
    assert_eq!(crate::priority::own_nice(), 10);
    let artifacts = tempfile::Builder::new()
        .prefix("qualification-native59-proof-NoQualification-")
        .tempdir()
        .unwrap()
        .keep();
    eprintln!("native59_proof_control_artifacts={}", artifacts.display());
    let mut prepared = PreparedBackend::unassembled(
        TrialName {
            envelope: Envelope::Small,
            number: 1,
        },
        PipelineMode::Qualification,
        super::preparation_tests::marker(),
    )
    .await;
    let collected = std::panic::AssertUnwindSafe(collect(&mut prepared, &artifacts))
        .catch_unwind()
        .await;
    let cleanup = close_attached(prepared, &artifacts).await;
    save(
        &artifacts.join("cleanup.json"),
        &json!({"qualification":false,"result":cleanup}),
    )
    .unwrap();
    let (proofs, rows, clients) = collected
        .expect("actual native read proof helper panicked")
        .unwrap();
    assert_eq!(proofs.len(), 59);
    assert_eq!(clients.len(), 59);
    let mut failures = Vec::new();
    for (slot, expected, proof) in proofs {
        match proof {
            Ok(proof) => {
                if proof.original_id != slot.original_id
                    || proof.method != slot.method.unwrap()
                    || proof.read_cohort != Some(expected)
                    || proof.effect.is_none()
                {
                    failures.push(format!(
                        "{}: native identity/form/effect proof mismatch",
                        slot.original_id
                    ));
                }
            }
            Err(error) => failures.push(format!("{}: {error}", slot.original_id)),
        }
    }
    for row in clients {
        let elapsed = row
            .final_at
            .map(|at| at.saturating_duration_since(row.intended_at));
        if row.final_outcome != Some(FinalOutcome::Reply)
            || elapsed.is_none_or(|elapsed| elapsed > Duration::from_millis(100))
        {
            failures.push(format!(
                "{}: response from immutable due {elapsed:?}, outcome {:?}",
                row.original_id, row.final_outcome
            ));
        }
    }
    for row in rows {
        if row.frames.is_empty()
            || row
                .frames
                .iter()
                .any(|frame| frame.lock_wait_us != 0 || frame.held_us != 0)
        {
            failures.push(format!(
                "{}: missing read frame or actual app-lock wait/hold {:?}",
                row.slot.original_id, row.frames
            ));
        }
    }
    if let Err(error) = cleanup {
        failures.push(format!(
            "physical cleanup / retained watch evidence: {error}"
        ));
    }
    save(
        &artifacts.join("assertion-failures.json"),
        &json!({"qualification":false,"errors":failures}),
    )
    .unwrap();
    assert!(
        failures.is_empty(),
        "native59 proof adapter failures: {failures:?}"
    );
}

type CollectedProofs = Vec<(Slot, ReadCohort, Result<ExecutionProof, String>)>;
type Collected = (
    CollectedProofs,
    Vec<super::super::trial_complete::Row>,
    Vec<super::super::client_watch::ClientRow>,
);

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "native read decoded-error retention; explicit quiet release NI10 readiness control"]
async fn actual_read_stage_failure_preserves_its_decoded_response_before_error() {
    assert!(
        !cfg!(debug_assertions),
        "native read error control requires release-opt3"
    );
    crate::git_fixture::environment::isolated_git_test!();
    assert_eq!(crate::priority::own_nice(), 10);
    let artifacts = tempfile::Builder::new()
        .prefix("qualification-read-raw-failure-NoQualification-")
        .tempdir()
        .unwrap()
        .keep();
    eprintln!(
        "native_read_error_control_artifacts={}",
        artifacts.display()
    );
    let mut prepared = PreparedBackend::unassembled(
        TrialName {
            envelope: Envelope::Small,
            number: 1,
        },
        PipelineMode::TwoSecondControl,
        super::preparation_tests::marker(),
    )
    .await;
    let collected = std::panic::AssertUnwindSafe(collect_stage_failure(&mut prepared, &artifacts))
        .catch_unwind()
        .await;
    let cleanup = close_attached(prepared, &artifacts).await;
    save(
        &artifacts.join("cleanup.json"),
        &json!({"qualification":false,"result":cleanup}),
    )
    .unwrap();
    let (slot, error, raw) = collected
        .expect("native read stage control panicked")
        .unwrap();
    assert!(
        error.contains("duplicate WireSent"),
        "injected accounting failure must remain: {error}"
    );
    let raw =
        raw.expect("actual decoded reply must be archived even when recording its stage fails");
    assert_eq!(raw["original_id"], slot.original_id);
    assert_eq!(raw["case"], "tasks.get");
    assert_eq!(raw["failure"], true);
    assert_eq!(raw["decoded_response"]["id"], slot.original_id);
    assert_eq!(raw["decoded_response"]["ok"], true);
    assert!(raw["decoded_response"]["result"]["task"]["id"].is_string());
    cleanup.expect("actual native read failure control cleanup");
}

async fn collect_stage_failure(
    prepared: &mut PreparedBackend,
    artifacts: &Path,
) -> Result<(Slot, String, Option<serde_json::Value>), String> {
    prepared.assemble_support().await?;
    let mut slot = prepared
        .plan
        .iter()
        .find(|slot| slot.traffic == Traffic::Read && slot.method.as_deref() == Some("tasks.get"))
        .ok_or("native bounded task read missing")?
        .clone();
    save(
        &artifacts.join("source-slot.json"),
        &json!({"qualification":false,"source_slot":slot}),
    )?;
    slot.original_id = "native-read-stage-failure/tasks.get".into();
    slot.due_us = 0;
    slot.sequence = 0;
    prepared.plan = vec![slot.clone()];
    prepared.prepare_collectors()?;
    let watch = prepared.watch.as_ref().ok_or("watch missing")?.clone();
    prepared
        .fixture
        .as_mut()
        .ok_or("fixture missing")?
        .rpc
        .attach_client_watch(watch.clone());
    let start = Instant::now();
    let observe = Observation::for_native_control_at(&[slot.clone()], start);
    watch.start_at(start);
    watch.offer(&slot.original_id);
    observe.stage(&slot.original_id, Stage::WireSent, start)?;
    let recipe = Recipe {
        method: "tasks.get".into(),
        scenario: "native-read-stage-failure-control".into(),
        recipe_key: slot.original_id.clone(),
        predecessor: None,
        needs_admission: false,
        needs_durable_ack: false,
        needs_registry_root: false,
        durable_operation_id: None,
        expected_read_cohort: Some(ReadCohort::ReadyBounded),
    };
    let sink = JsonArtifacts::new(artifacts.join("responses"))?;
    let result = control::prove_native_read(prepared, &slot, &recipe, &observe, &sink).await;
    let error = match result {
        Err(error) => error,
        Ok(_) => return Err("injected duplicate stage unexpectedly produced success".into()),
    };
    let directory = artifacts
        .join("responses")
        .join(prepared.name.label())
        .join("raw-failures");
    let raw = if directory.exists() {
        let files = std::fs::read_dir(directory)
            .map_err(|error| error.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        if files.len() != 1 {
            return Err(format!(
                "expected one exact raw failure, found {}",
                files.len()
            ));
        }
        Some(
            serde_json::from_slice(
                &std::fs::read(files[0].path()).map_err(|error| error.to_string())?,
            )
            .map_err(|error| error.to_string())?,
        )
    } else {
        None
    };
    save(
        &artifacts.join("before-cleanup.json"),
        &json!({
            "qualification":false,"original_id":slot.original_id,"error":error,"raw":raw
        }),
    )?;
    Ok((slot, error, raw))
}

async fn collect(prepared: &mut PreparedBackend, artifacts: &Path) -> Result<Collected, String> {
    prepared.assemble_support().await?;
    if prepared.watch.is_some() || prepared.lifecycle.is_some() {
        return Err("normal schedule was enrolled before selecting explicit control IDs".into());
    }
    let source = select_first_native_reads(&prepared.plan)?;
    let mut recipes = Vec::new();
    prepared.plan = source
        .iter()
        .enumerate()
        .map(|(index, original)| {
            let mut slot = original.clone();
            slot.original_id =
                format!("native-read-proof-control/{index}/{}", original.original_id);
            slot.sequence = index;
            slot.due_us = index as u64 * 150_000;
            slot
        })
        .collect();
    for slot in &prepared.plan {
        recipes.push(exact_recipe(prepared, slot)?);
    }
    save(
        &artifacts.join("schedule.json"),
        &json!({
            "qualification":false,"normal_trials_started":0,"source_slots":source,
            "control_slots":prepared.plan,"spacing_us":150000,"unchanged_read_limit_us":100000,
            "normal_schedule_enrolled":false
        }),
    )?;
    prepared.prepare_collectors()?;
    let watch = prepared
        .watch
        .as_ref()
        .ok_or("owned watch missing")?
        .clone();
    prepared
        .fixture
        .as_mut()
        .ok_or("owned fixture missing")?
        .rpc
        .attach_client_watch(watch.clone());
    let start = Instant::now() + Duration::from_millis(10);
    let observe = Observation::for_native_control_at(&prepared.plan, start);
    watch.start_at(start);
    let sink = JsonArtifacts::new(artifacts.join("responses"))?;
    let mut proofs = Vec::new();
    for (slot, recipe) in prepared.plan.iter().zip(&recipes) {
        tokio::time::sleep_until((start + Duration::from_micros(slot.due_us)).into()).await;
        watch.offer(&slot.original_id);
        observe.stage(&slot.original_id, Stage::DispatchStarted, Instant::now())?;
        let proof = control::prove_native_read(prepared, slot, recipe, &observe, &sink)
            .await
            .and_then(|result| match result {
                ControlExecution::Verified(proof) => Ok(proof),
                ControlExecution::ProofGap(gap) => {
                    Err(format!("native read proof gap: {}", gap.detail))
                }
                ControlExecution::Refused(refusal) => Err(format!(
                    "unexpected refusal on native read {}: {}", refusal.original_id, refusal.method
                )),
            });
        settle_unreturned_failure(&watch, &slot.original_id, &proof);
        save(
            &artifacts.join(format!("proof-{}.json", slot.sequence)),
            &json!({
                "qualification":false,"slot":slot,"recipe":recipe,"result":proof
            }),
        )?;
        proofs.push((
            slot.clone(),
            recipe
                .expected_read_cohort
                .clone()
                .ok_or("exact cohort missing")?,
            proof,
        ));
    }
    prepared.frames.drain_into(&observe)?;
    let rows = observe.native_control_rows();
    save(
        &artifacts.join("frame-observations.json"),
        &json!({"qualification":false,"rows":rows}),
    )?;
    let clients = watch.snapshot();
    let timing = clients.iter().map(|row| json!({
        "original_id":row.original_id,"due_us":row.intended_at.saturating_duration_since(start).as_micros(),
        "final_us":row.final_at.map(|at| at.saturating_duration_since(start).as_micros()),
        "elapsed_us":row.final_at.map(|at| at.saturating_duration_since(row.intended_at).as_micros()),
        "outcome":format!("{:?}", row.final_outcome),"cancelled":row.cancelled,
        "implicit_schedule":row.implicit_schedule
    })).collect::<Vec<_>>();
    save(
        &artifacts.join("actual-client-timing.json"),
        &json!({"qualification":false,"rows":timing}),
    )?;
    Ok((proofs, rows, clients))
}

fn select_first_native_reads(plan: &[Slot]) -> Result<Vec<Slot>, String> {
    let mut seen = BTreeSet::new();
    let selected = plan
        .iter()
        .filter(|slot| slot.traffic == Traffic::Read)
        .filter(|slot| seen.insert((slot.route == Route::AgentMcp, slot.method.clone())))
        .cloned()
        .collect::<Vec<_>>();
    let rpc = selected
        .iter()
        .filter(|slot| slot.route == Route::SpaRpc)
        .count();
    let mcp = selected.len() - rpc;
    if (rpc, mcp) != (45, 14) {
        return Err(format!(
            "actual native read catalog changed: RPC={rpc}, MCP={mcp}"
        ));
    }
    Ok(selected)
}

fn exact_recipe(prepared: &PreparedBackend, slot: &Slot) -> Result<Recipe, String> {
    let method = slot.method.as_deref().ok_or("native read method missing")?;
    let form = match slot.route {
        Route::SpaRpc => {
            prepared
                .rpc_cases
                .iter()
                .find(|case| case.method == method)
                .ok_or("native RPC read case missing")?
                .form
        }
        Route::AgentMcp => {
            let (inputs, _) = prepared
                .mcp_inputs
                .get(slot.owner)
                .ok_or("native read owner missing")?;
            let (_, action) = mcp_cases(inputs)
                .into_iter()
                .find(|(_, action)| mcp_read_method(action) == method)
                .ok_or("native MCP action missing")?;
            mcp_recipe_form(&action).ok_or("native MCP form missing")?
        }
    };
    Ok(Recipe {
        method: method.into(),
        scenario: "actual-native-read-adapter-control".into(),
        recipe_key: slot.original_id.clone(),
        predecessor: None,
        needs_admission: false,
        needs_durable_ack: false,
        needs_registry_root: false,
        durable_operation_id: None,
        expected_read_cohort: Some(match form {
            FullReadForm::Ready => ReadCohort::ReadyBounded,
            FullReadForm::LegacyFull => ReadCohort::LegacyUnbounded,
        }),
    })
}

fn settle_unreturned_failure(
    watch: &ClientWatch,
    id: &str,
    proof: &Result<ExecutionProof, String>,
) {
    if proof.is_err()
        && watch
            .snapshot()
            .iter()
            .any(|row| row.original_id == id && row.finished_at.is_none())
    {
        // A test-only pre-dispatch failure is cancellation, never a decoded reply.
        watch.cancel(id);
    }
}

pub(super) async fn close_attached(
    mut prepared: PreparedBackend,
    artifacts: &Path,
) -> Result<(), String> {
    // These explicit controls still own every real setup worker. Failure of a
    // proof never fabricates a final receipt or permits an unsafe watch detach.
    if let Some(producers) = &prepared.producers {
        for producer in &producers.handles {
            producer.stop_offering();
        }
    }
    if let Some(mcp) = &prepared.mcp {
        mcp.stop_offers();
    }
    if let Err(error) = prepared.join_setup_workers().await {
        std::mem::forget(prepared);
        return Err(format!(
            "native control workers unfinished; resources retained: {error}"
        ));
    }
    if let Some(owner) = &prepared.lifecycle {
        owner.request_finish();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while !owner.is_finished() && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        if !owner.is_finished() || owner.report().is_none_or(|report| report.panicked) {
            std::mem::forget(prepared);
            return Err("actual59 watch join unconfirmed; all resources retained".into());
        }
    }
    let mut errors = Vec::new();
    let watch_report = prepared.lifecycle.as_ref().and_then(|owner| owner.report());
    if watch_report.as_ref().is_some_and(|report| report.failed) {
        errors.push(
            "actual joined watchdog failed; report retained independently of teardown".into(),
        );
    }
    let watch_evidence = watch_report.as_ref().map(|report| {
        json!({
            "failed":report.failed,"panicked":report.panicked,"polls":report.polls,
            "inspected_deadlines":report.inspected_deadlines,"still_indexed":report.still_indexed,
            "breaches":format!("{:?}",report.breaches)
        })
    });
    let detached = prepared
        .fixture
        .as_ref()
        .ok_or("owned fixture missing")?
        .rpc
        .detach_client_watch();
    if let Err(error) = save(
        &artifacts.join("detached-watch.json"),
        &json!({"qualification":false,"result":format!("{detached:?}"),"watch_report":watch_evidence}),
    ) {
        errors.push(error);
    }
    if let Err(refusal) = detached {
        errors.push(format!(
            "actual watch detach refused, unchanged: {refusal:?}"
        ));
    }
    // The pre-dispatch TESTFIRST gap deliberately leaves cancelled originals
    // without finals. Keep the refused attachment; perform the native failure
    // teardown instead of forgetting live fixture resources or inventing finals.
    // All actual shutdown diagnostics survive independently of target failures.
    prepared.probe.take();
    prepared.mcp.take();
    prepared.lifecycle.take();
    prepared.watch.take();
    prepared.producers.take();
    prepared.services.take();
    prepared.reads.take();
    if let Some(terminal) = prepared.terminal.take() {
        let cleanup = terminal.shutdown(&prepared.fixture.as_ref().expect("owned fixture").rpc);
        if std::panic::AssertUnwindSafe(cleanup)
            .catch_unwind()
            .await
            .is_err()
        {
            errors.push(
                "actual exempt terminal shutdown failed; native all-tab retirement follows".into(),
            );
        }
    }
    if let Some(fixture) = prepared.fixture.take() {
        // This existing helper catches ReadyReadInputs shutdown failure, retires
        // every native tab, joins encrypted receivers, checks AppState release,
        // and preserves private roots if any cleanup remains unconfirmed.
        errors.extend(fixture.cleanup_owned_resources().await);
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

fn save(path: &Path, value: &serde_json::Value) -> Result<(), String> {
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

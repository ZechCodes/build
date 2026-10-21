//! Actual400-intent overload; a valid busy is not a product defect or success.
use super::*;
use serde::Serialize;
use trial_complete::PipelineControlRun;
#[derive(Serialize)]
struct OverloadSummary<'a> {
    qualification: bool,
    engine: &'a trial_complete::PipelineControlReport,
    diagnostics: Value,
    validation_errors: &'a [String],
}
pub(in crate::app::tests::runtime::write_wait_qualification) struct OverloadRun {
    pub engine: PipelineControlRun,
    pub artifact: PathBuf,
    pub diagnostics: Value,
    pub validation_errors: Vec<String>,
}

pub(in crate::app::tests::runtime::write_wait_qualification) async fn run_overload(
    name: TrialName,
    bound: &BoundFault,
    artifacts: &FaultArtifacts,
) -> Result<OverloadRun, String> {
    let provenance = bound.provenance();
    let plan = super::plan::overload_plan(name)?;
    let engine_root = artifacts.engine_path();
    let sink: Arc<dyn ArtifactSink> = Arc::new(trial_complete::JsonArtifacts::new(&engine_root)?);
    let prepared = match super::preparation::prepare_overload(name, provenance, plan.clone()).await
    {
        Ok(prepared) => prepared,
        Err(failure) => return Err(cleanup_setup(failure).await),
    };
    let backend = match ControlBackend::new_overload(prepared, Arc::clone(&sink)) {
        Ok(backend) => Arc::new(backend) as Arc<dyn trial_complete::TrialBackend>,
        Err(failure) => return Err(cleanup_setup(failure).await),
    };
    let run = match trial_complete::run_fault_plan(
        name,
        Arc::clone(&backend),
        &*sink,
        plan.clone(),
        Duration::from_secs(1),
    )
    .await
    {
        Ok(run) => run,
        Err(error) => {
            let closed = close_failed_backend(backend).await;
            return Err(format!("actual overload engine failure: {error}; actual bounded cleanup confirmed={closed}"));
        }
    };
    let (diagnostics, mut validation_errors) =
        match archive_diagnostics(&engine_root, name, &plan, &run.report) {
            Ok(value) => value,
            Err(error) => (
                json!({"archive_validation_error":error,"qualification":false}),
                vec![format!("overload final archive validation failed: {error}")],
            ),
        };
    // This runs after the engine's actual native cleanup attempt. The raw
    // report remains intact; an invalid refusal/archive can never be a pass.
    validate_engine(&run, &mut validation_errors);
    let artifact = match artifacts.write(
        "overload400",
        &OverloadSummary {
            qualification: false,
            engine: &run.report,
            diagnostics: diagnostics.clone(),
            validation_errors: &validation_errors,
        },
    ) {
        Ok(path) => path,
        Err(error) => {
            if run.unfinished.is_some() {
                std::mem::forget(run);
            }
            return Err(format!("overload summary persistence failed; any unfinished run ownership retained: {error}"));
        }
    };
    Ok(OverloadRun {
        engine: run,
        artifact,
        diagnostics,
        validation_errors,
    })
}
async fn cleanup_setup(failure: PreparationFailure) -> String {
    let reason = failure.reason;
    match failure.retained.shutdown().await {
        Ok(_) => reason,
        Err(retained) => {
            let cleanup = retained.reason;
            std::mem::forget(retained.retained);
            format!("{reason}; actual cleanup retained: {cleanup}")
        }
    }
}
fn archive_diagnostics(
    root: &Path,
    name: TrialName,
    plan: &[Slot],
    report: &trial_complete::PipelineControlReport,
) -> Result<(Value, Vec<String>), String> {
    use std::io::BufRead;
    let dir = root.join(name.label());
    let native: Value = serde_json::from_reader(
        std::fs::File::open(dir.join("native-registry-final.json")).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let rows = std::io::BufReader::new(
        std::fs::File::open(dir.join("control-rows.jsonl")).map_err(|e| e.to_string())?,
    )
    .lines()
    .map(|line| {
        serde_json::from_str::<Value>(&line.map_err(|e| e.to_string())?).map_err(|e| e.to_string())
    })
    .collect::<Result<Vec<_>, String>>()?;
    let Some(state) = &report.fault_overload else {
        return Err("actual overload runner diagnostics missing".into());
    };
    let mut errors = Vec::new();
    let refusals = state
        .actual_refusals
        .values()
        .map(
            |refusal| match check_refusal(root, plan, &native, &rows, refusal) {
                Ok(disposition) => {
                    if let super::refusal::BusyDisposition::Invalid {
                        original_id,
                        reason,
                    } = &disposition
                    {
                        errors.push(format!("invalid refusal {original_id}: {reason}"));
                    }
                    json!(disposition)
                }
                Err(error) => {
                    errors.push(format!("refusal archive {}: {error}", refusal.original_id));
                    json!({"original_id":refusal.original_id,"archive_error":error})
                }
            },
        )
        .collect::<Vec<_>>();
    Ok((
        json!({"intended_semantic":400,"offer_window_us":1_000_000,"semantic_caller_count":12,"semantic_credits_per_caller":2,
        "actual_refusals":refusals,"verified_refused_count":state.verified_refusals.len(),"zero_busy_is_valid":true,"admitted_marker_semantics":"classified intake before admitter Full, not durable acceptance","offered_unsent_dependents":state.censored_dependents,
        "interpretation":"valid proved preaccept busy is neither product defect nor successful mutation; no rewrite/retry/replacement; all engine violations remain attached",
        "native_rows":native["rows"].as_array().map(Vec::len),"archived_client_rows":rows.len()}),
        errors,
    ))
}
fn check_refusal(
    root: &Path,
    plan: &[Slot],
    native: &Value,
    rows: &[Value],
    refusal: &trial_complete::ControlRefusal,
) -> Result<super::refusal::BusyDisposition, String> {
    let slot = plan
        .iter()
        .find(|slot| slot.original_id == refusal.original_id)
        .ok_or("actual refusal outside immutable400")?;
    if !Path::new(&refusal.raw_response_artifact)
        .components()
        .all(|part| matches!(part, std::path::Component::Normal(_)))
    {
        return Err("raw refusal artifact must remain inside its original archive".into());
    }
    let raw: Value = serde_json::from_reader(
        std::fs::File::open(root.join(&refusal.raw_response_artifact))
            .map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    if raw["original_id"] != refusal.original_id || raw["case"] != refusal.method {
        return Err("actual raw refusal artifact identity changed".into());
    }
    let matching = native["rows"]
        .as_array()
        .ok_or("native rows missing")?
        .iter()
        .filter(|row| row["original_id"] == refusal.original_id)
        .collect::<Vec<_>>();
    let row = if matching.len() == 1 {
        Some(matching[0])
    } else {
        None
    };
    let frames = frame_count(rows, &refusal.original_id, &refusal.method)?;
    Ok(super::refusal::archived(
        slot,
        refusal,
        &raw["decoded_response"],
        row,
        frames,
    ))
}

async fn close_failed_backend(backend: Arc<dyn trial_complete::TrialBackend>) -> bool {
    backend.request_close();
    let until = Instant::now() + CLEANUP_BOUND;
    while !backend.closed() && Instant::now() < until {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    if backend.closed() {
        true
    } else {
        std::mem::forget(backend);
        false
    }
}

/// A final client row with an explicitly empty array is evidence of no frame.
/// Missing, malformed or duplicate archive rows are unavailable evidence.
fn frame_count(rows: &[Value], original: &str, method: &str) -> Result<usize, String> {
    let mut matching = rows
        .iter()
        .filter(|row| row["slot"]["original_id"] == original);
    let row = matching.next().ok_or("refusal final client row missing")?;
    if matching.next().is_some() {
        return Err("refusal final client row duplicated".into());
    }
    if row["recipe"]["method"] != method {
        return Err("refusal final client method missing or mismatched".into());
    }
    row["frames"]
        .as_array()
        .map(Vec::len)
        .ok_or_else(|| "refusal executed-frame archive missing or malformed".into())
}
fn validate_engine(run: &PipelineControlRun, errors: &mut Vec<String>) {
    let report = &run.report;
    if run.unfinished.is_some() || !report.cleanup_drained || !report.backend_closed {
        errors.push("overload native cleanup is unfinished".into());
    }
    if let Some(error) = &report.invalid_infrastructure {
        errors.push(format!("overload infrastructure: {error}"));
    }
    if !report.explicit_proof_gaps.is_empty() {
        errors.push("overload accepted work lacks exact effect/completion proofs".into());
    }
    for violation in &report.violations {
        errors.push(format!(
            "overload engine violation {}: {}",
            violation.kind, violation.detail
        ));
    }
    let refused = report
        .fault_overload
        .as_ref()
        .map_or(0, |state| state.verified_refusals.len());
    if report.fault_overload.as_ref().is_none_or(|state| {
        state.verified_refusals.len() != state.actual_refusals.len()
            || !state.censored_dependents.is_empty()
    }) {
        errors.push("overload has unproved refusal candidates or unsent dependent calls".into());
    }
    let counts = report.final_counts.get(&trial_complete::Traffic::Write);
    if counts.is_none_or(|counts| {
        counts.intended != 400
            || counts.offered != 400
            || counts.sent != 400
            || counts.completed != 400
            || counts.verified_success + refused != 400
            || counts.failed != 0
    }) {
        errors.push("overload did not retain and complete all 400 original offered calls".into());
    }
    if report.offer_window_us != 1_000_000 || report.qualification {
        errors.push("overload control window/qualification identity changed".into());
    }
}

#[cfg(test)]
mod tests;

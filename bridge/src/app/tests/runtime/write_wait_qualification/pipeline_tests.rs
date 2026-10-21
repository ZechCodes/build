//! OFFSOURCE runnable short-control draft. Not executed or qualification evidence.
//! Materialize only after the concrete assembly/source window is reviewed.
#![cfg(unix)]
use super::entrypoint;
use super::factory::ConcreteFactory;
use super::trial_complete::{
    ArtifactSink, JsonArtifacts, PipelineControlReport, Provenance, Route, Row, Stage, Traffic,
};
use futures_util::FutureExt;
use serde::Serialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::io::{BufRead, Read, Write};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

const UNPROVIDED: &str = "not-provided-NoQualification";
const OUTER_BOUND: Duration = Duration::from_secs(120);
mod artifacts;
mod assertions;

use artifacts::{
    actual_provenance, infrastructure_failure, load_native, load_rows, panic_detail, persist,
};
use assertions::{
    assert_complete_rows, assert_native_identity, assert_proof_only_failure,
    assert_transport_and_cleanup,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "mandatory 312-client release-opt3 checkpoint with explicit quiet release; separate from six normal qualification trials"]
async fn two_second_native_pipeline_preserves_every_offered_original_and_cleans_up() {
    assert!(
        !cfg!(debug_assertions),
        "the physical 312-client checkpoint requires the pinned release-opt3 binary; debug execution is forbidden before fixture setup"
    );
    crate::git_fixture::environment::isolated_git_test!();
    // Preserve BEFORE any preparation, attempted traffic, infrastructure failure
    // or intended final proof RED. TempDir drop must never erase this evidence.
    let retained = tempfile::Builder::new()
        .prefix("build-pipeline-NoQualification-")
        .tempdir()
        .expect("create private control artifact directory")
        .keep();
    eprintln!("pipeline_control_artifacts={}", retained.display());
    persist(
        &retained,
        "control-intent.json",
        &json!({
            "qualification":false,"status":"NoQualification","seconds":2,
            "semantic_intents":80,"exempt_intents":200,"read_intents":32,"all_client_intents":312,
            "mcp_reads":0,"router_endpoints":0,"coding_endpoints":8,
            "source_revision":UNPROVIDED,"source_tree":UNPROVIDED,"fresh_quiet_release_sha256":UNPROVIDED,
            "outer_bound_seconds":OUTER_BOUND.as_secs(),
            "profile_contract":"mandatory release-opt3; actual compiler flags and quiet freshness verified by the external pinned runner",
            "selector":"two_second_native_pipeline_preserves_every_offered_original_and_cleans_up"
        }),
    );
    let provenance = match actual_provenance() {
        Ok(provenance) => provenance,
        Err(error) => infrastructure_failure(&retained, "metadata", &error),
    };
    persist(&retained, "actual-process-metadata.json", &provenance);
    if provenance.nice != 10 {
        infrastructure_failure(
            &retained,
            "priority",
            &format!("actual NI{}; this control requires NI10", provenance.nice),
        );
    }
    let sink: Arc<dyn ArtifactSink> = Arc::new(
        JsonArtifacts::new(retained.join("traffic"))
            .unwrap_or_else(|error| infrastructure_failure(&retained, "artifact-writer", &error)),
    );
    let factory = Arc::new(ConcreteFactory::pipeline_control(provenance, sink));
    let trial_factory = Arc::clone(&factory);
    let mut owned_attempt = Box::pin(
        std::panic::AssertUnwindSafe(async move {
            entrypoint::two_second_pipeline_control(trial_factory.as_ref()).await
        })
        .catch_unwind(),
    );
    let attempt = tokio::time::timeout(OUTER_BOUND, owned_attempt.as_mut()).await;
    let outcome = match attempt {
        Ok(outcome) => {
            drop(owned_attempt);
            outcome
        }
        Err(_) => {
            // Preserve the future and its actual owned setup. Timeout must not
            // drop accepted work and then claim ordinary clean fixture closure.
            std::mem::forget(owned_attempt);
            std::mem::forget(factory);
            infrastructure_failure(
                &retained,
                "outer-timeout",
                "cleanup not proven; owned control future retained, supervisor process containment required; no retry",
            );
        }
    };
    let mut run = match outcome {
        Ok(Ok(run)) => run,
        Ok(Err(error)) => {
            std::mem::forget(factory);
            infrastructure_failure(&retained, "entrypoint-returned-error", &error)
        }
        Err(payload) => {
            let detail = panic_detail(&*payload);
            std::mem::forget(factory);
            infrastructure_failure(&retained, "entrypoint-panicked", &detail)
        }
    };
    // Retain the complete actual report before any assertion. Do not replace
    // traffic/infrastructure errors with an expected proof-gap success result.
    persist(&retained, "returned-NoQualification.json", &run.report);
    if let Some(unfinished) = run.unfinished.take() {
        persist(
            &retained,
            "unfinished-cleanup.json",
            &json!({
                "qualification":false,"original_ids":unfinished.original_ids,
                "owned_tasks":unfinished.tasks.len(),"cleanup":"not-proven-owned-resources-retained"
            }),
        );
        std::mem::forget(unfinished);
        std::mem::forget(factory);
        infrastructure_failure(
            &retained,
            "unfinished-cleanup",
            "actual control returned owned unfinished work",
        )
    }
    let trial_dir = retained.join("traffic").join(run.report.trial_seed.label());
    let rows = load_rows(&trial_dir.join("control-rows.jsonl"), &retained);
    let native = load_native(&trial_dir.join("native-registry-final.json"), &retained);
    let prerequisite_checks = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        assert_transport_and_cleanup(&run.report, &retained);
        assert_complete_rows(&rows, &run.report, &retained);
        assert_native_identity(&rows, &native);
        assert_proof_only_failure(&run.report, &retained);
        assert!(
            trial_dir.join("NoQualification.json").is_file(),
            "actual sink report missing; artifacts={}",
            retained.display()
        );
    }));
    if let Err(payload) = prerequisite_checks {
        infrastructure_failure(
            &retained,
            "actual-prerequisite-check",
            &panic_detail(&*payload),
        );
    }
    persist(
        &retained,
        "proof-boundary-NoQualification.json",
        &json!({
            "qualification":false,"status":"NoQualification",
            "actual_originals":rows.len(),"final_counts":run.report.final_counts,
            "transport_schedule_completed":run.report.transport_schedule_completed,
            "cleanup_drained":run.report.cleanup_drained,"backend_closed":run.report.backend_closed,
            "pipeline_succeeded":run.report.pipeline_succeeded,
            "explicit_proof_gaps":run.report.explicit_proof_gaps,
            "prerequisite_checks":"passed; final pipeline proof assertion follows"
        }),
    );
    // This is intentionally NOT should_panic and does NOT assert false as a
    // passing result. Missing real accepted/saved/incremental proof remains RED
    // only after every actual original and native cleanup prerequisite passed.
    assert!(
        run.report.pipeline_succeeded,
        "INTENDED PROOF RED after actual 312/80/200/32 transport and clean drain: missing_proofs={}; artifacts={}",
        run.report.explicit_proof_gaps.len(),
        retained.display()
    );
}

//! OFFSOURCE: explicit six-original release selector, never a smoke substitute.
use super::factory::ConcreteFactory;
use super::trial_complete::{ArtifactSink, JsonArtifacts, ORIGINAL_TRIALS};
use futures_util::FutureExt;
use std::sync::Arc;

mod evidence;
#[cfg(test)]
mod original_retention;
mod provenance;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "six original 600s local NI10 qualification trials; requires frozen manifests, all gates and explicit coordinator quiet release"]
async fn qualify_small_and_large_three_original_ten_minute_trials() {
    assert!(!cfg!(debug_assertions), "normal qualification requires the pinned release opt3 build; debug execution is forbidden before any fixture or original trial");
    crate::git_fixture::environment::isolated_git_test!();
    let attempt = provenance::load();
    let input = match attempt.result {
        Ok(input) => input,
        Err(error) => evidence::preflight_failure(&attempt.captured, "provenance", &error),
    };
    // create_dir is the exclusive claim: an old run is never overwritten,
    // appended to or retried. The raw inputs are persisted before any fixture.
    let artifacts = match evidence::EntryArtifacts::claim(&input.artifact_root, &attempt.captured) {
        Ok(artifacts) => artifacts,
        Err(error) => evidence::preflight_failure(&attempt.captured, "claim-original-root", &error),
    };
    eprintln!(
        "qualification_original_artifacts={}",
        artifacts.root.display()
    );
    artifacts.write("original-intent.json", &serde_json::json!({
        "schema":"build.qualification.original-intent.v1","run_id":input.run_id,
        "originals":ORIGINAL_TRIALS,"seconds_each":600,"writes_per_second":40,
        "expected_per_original":{"semantic":24000,"exempt":60000,"read":59000,"total":143000},
        "replacement_permitted":false,"source_revision":input.provenance.source_revision,
        "source_tree":input.provenance.source_tree,"binary_sha256":input.provenance.binary_sha256,
        "fixture_counts":"declarations are not measurement; normal backend must bind native pre-T0 receipt",
    })).unwrap_or_else(|error| artifacts.fail("persist-intent", &error));
    let sink: Arc<dyn ArtifactSink> = Arc::new(
        JsonArtifacts::new(artifacts.root.join("traffic"))
            .unwrap_or_else(|error| artifacts.fail("artifact-sink", &error)),
    );
    // This remains the existing fail-closed factory until the separately
    // reviewed native proof/count/read adapters make its one engine Ready.
    let frozen_provenance = input.provenance.clone();
    let factory = Arc::new(ConcreteFactory::qualification(input.provenance, sink));
    let trial_factory = Arc::clone(&factory);
    let mut owned_attempt = Box::pin(
        std::panic::AssertUnwindSafe(async move {
            super::entrypoint::qualification(&trial_factory).await
        })
        .catch_unwind(),
    );
    let outcome = evidence::suite_outcome(owned_attempt.as_mut().await);
    let mut result = match outcome {
        Ok(result) => {
            drop(owned_attempt);
            result
        }
        Err((stage, detail)) => {
            std::mem::forget(owned_attempt);
            std::mem::forget(factory);
            artifacts.fail(stage, &detail);
        }
    };
    // Save the returned original outcomes before any assertion or ownership
    // handoff, independently of the streaming suite-history file.
    if let Err(error) = artifacts.write("returned-suite.json", &result.report) {
        std::mem::forget(result);
        std::mem::forget(factory);
        artifacts.fail("persist-returned-suite", &error);
    }
    if let Some(unfinished) = result.unfinished.take() {
        let detail = serde_json::json!({
            "original_ids":unfinished.original_ids,"owned_tasks":unfinished.tasks.len(),
            "cleanup":"unproven; exact owned process requires external containment"
        });
        let saved = artifacts.write("unfinished-cleanup.json", &detail);
        std::mem::forget(unfinished);
        std::mem::forget(factory);
        if let Err(error) = saved {
            artifacts.fail("persist-unfinished", &error);
        }
        artifacts.fail(
            "unfinished-cleanup",
            "no later original or replacement may start",
        );
    }
    super::qualification_metadata::validate_current_source(&frozen_provenance)
        .unwrap_or_else(|error| artifacts.fail("source-changed-during-originals", &error));
    assert_eq!(result.report.attempts.len(), ORIGINAL_TRIALS.len(),
        "all six original outcomes, including not-attempted reasons, must remain visible; artifacts={}", artifacts.root.display());
    assert!(
        result.report.qualified,
        "qualification failed; all original results retained at {}; no retry or best-run selection",
        artifacts.root.display()
    );
}

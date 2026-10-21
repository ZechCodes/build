//! Independent release-only fault controls. Never six-original replacements.
use super::backend::faults::{FaultArtifacts, PhysicalCase};
use super::trial_complete::{Envelope, TrialName};
use serde_json::json;
pub(super) mod provenance;
const NAME: TrialName = TrialName {
    envelope: Envelope::Small,
    number: 1,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "physical 6s app-lock fault; frozen build and purpose-bound quiet receipt required"]
async fn physical_pre_ack_app_lock_six_seconds() {
    crate::git_fixture::environment::isolated_git_test!();
    physical(PhysicalCase::PreAckApp, "physical-pre-ack-app", "app::tests::runtime::write_wait_qualification::fault_tests::physical_pre_ack_app_lock_six_seconds").await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "physical 6s exclusion fault; frozen build and purpose-bound quiet receipt required"]
async fn physical_pre_ack_source_exclusion_six_seconds() {
    crate::git_fixture::environment::isolated_git_test!();
    physical(PhysicalCase::PreAckExclusion, "physical-pre-ack-exclusion", "app::tests::runtime::write_wait_qualification::fault_tests::physical_pre_ack_source_exclusion_six_seconds").await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "physical 6s required child fault; frozen build and purpose-bound quiet receipt required"]
async fn physical_post_ack_required_child_six_seconds() {
    crate::git_fixture::environment::isolated_git_test!();
    physical(PhysicalCase::PostAckRequired, "physical-post-ack-required", "app::tests::runtime::write_wait_qualification::fault_tests::physical_post_ack_required_child_six_seconds").await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "400 semantic offers/s fault control; frozen build and purpose-bound quiet receipt required"]
async fn overload_four_hundred_semantic_offers_one_second() {
    crate::git_fixture::environment::isolated_git_test!();
    let bound = provenance::load("overload400", "app::tests::runtime::write_wait_qualification::fault_tests::overload_four_hundred_semantic_offers_one_second").expect("actual fault provenance required before setup");
    let artifacts = artifacts(&bound);
    let mut run = super::backend::faults::run_overload(NAME, &bound, &artifacts)
        .await
        .expect("actual overload result; errors preserve own cleanup status");
    // All raw reports are persisted and cleanup has been attempted before any
    // success assertion. Never drop unfinished accepted native work on unwind.
    if let Some(unfinished) = run.engine.unfinished.take() {
        std::mem::forget(unfinished);
        panic!(
            "overload cleanup unproved; original artifacts at {}",
            bound.artifact_root().display()
        );
    }
    assert!(
        run.validation_errors.is_empty(),
        "overload failures: {:?}; raw report={}",
        run.validation_errors,
        run.artifact.display()
    );
}
async fn physical(case: PhysicalCase, purpose: &str, selector: &str) {
    let bound =
        provenance::load(purpose, selector).expect("actual fault provenance required before setup");
    let artifacts = artifacts(&bound);
    let mut run = super::backend::faults::run_physical(case, NAME, &bound, &artifacts)
        .await
        .expect("actual physical result; errors preserve own cleanup status");
    if let Some(unfinished) = run.run.unfinished.take() {
        std::mem::forget(unfinished);
        panic!(
            "physical cleanup unproved; original artifacts at {}",
            bound.artifact_root().display()
        );
    }
    assert!(
        run.validation_errors.is_empty(),
        "physical fault detector failed: {:?}; validation={}",
        run.validation_errors,
        run.validation_artifact.display()
    );
}
fn artifacts(bound: &provenance::BoundFault) -> FaultArtifacts {
    let artifacts = FaultArtifacts::new(bound.artifact_root())
        .expect("new immutable fault root; no replacement");
    for (name, bytes) in &bound.captured {
        // Capture exact bound inputs before native fixture/resource creation.
        let path = bound.artifact_root().join(name);
        use std::io::Write;
        std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(path)
            .unwrap()
            .write_all(bytes)
            .unwrap();
    }
    artifacts
        .write(
            "actual-fault-binding",
            &json!({"qualification":false,"actual":bound.actual}),
        )
        .expect("persist actual process binding");
    artifacts
}

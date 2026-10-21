//! Concrete independently-invoked scripts, never replacement suite attempts.
//! Persist raw report and validation before the caller asserts the outcome.
use super::*;
use serde::Serialize;
#[derive(Clone, Copy, Debug, Serialize)]
pub(in crate::app::tests::runtime::write_wait_qualification) enum PhysicalCase {
    PreAckApp,
    PreAckExclusion,
    PostAckRequired,
}
pub(in crate::app::tests::runtime::write_wait_qualification) struct PhysicalRun {
    pub run: FaultRun,
    pub validation_artifact: PathBuf,
    pub validation_errors: Vec<String>,
}
pub(in crate::app::tests::runtime::write_wait_qualification) async fn run_physical(
    case: PhysicalCase,
    name: TrialName,
    bound: &BoundFault,
    artifacts: &FaultArtifacts,
) -> Result<PhysicalRun, String> {
    let provenance = bound.provenance();
    artifacts.write("physical-script-context",&json!({"qualification":false,"physical_case":case,"trial_seed":name,"supplied_actual_provenance":provenance,"clock":"std Instant real elapsed; no clock substitution"}))?;
    let run = match case {
        PhysicalCase::PreAckApp => pre_ack_app(name, artifacts).await?,
        PhysicalCase::PreAckExclusion => pre_ack_exclusion(name, artifacts).await?,
        PhysicalCase::PostAckRequired => post_ack_required(artifacts).await?,
    };
    let validation_errors = run.validation_errors();
    let validation_artifact=artifacts.write("physical-validation",&json!({"qualification":false,"report_artifact":run.artifact,"original_id":run.report.original_id,"validation_errors":validation_errors}))?;
    Ok(PhysicalRun {
        run,
        validation_artifact,
        validation_errors,
    })
}

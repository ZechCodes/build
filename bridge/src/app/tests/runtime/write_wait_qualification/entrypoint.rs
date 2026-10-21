//! OFFSOURCE entrypoints: separate full qualification and explicit shortcontrol.
use super::backend::PipelineMode;
use super::factory::ConcreteFactory;
use super::trial_complete::{Envelope, FixtureFactory, PipelineControlRun, SuiteRun, TrialName};

pub(super) async fn qualification(factory: &ConcreteFactory) -> Result<SuiteRun, String> {
    if factory.mode() != PipelineMode::Qualification {
        return Err("shortcontrol factory cannot start original six qualification trials".into());
    }
    super::trial_complete::run_all_six(factory, factory.artifacts()).await
}
/// Named separately from all six originals; caller supplies a separate artifact
/// root and an explicit release to run. Current real constructor prerequisites
/// return their genuine RED before any qualification traffic is offered.
pub(super) async fn two_second_pipeline_control(
    factory: &ConcreteFactory,
) -> Result<PipelineControlRun, String> {
    if factory.mode() != PipelineMode::TwoSecondControl {
        return Err("pipelinecontrol requires explicit two-second factory".into());
    }
    let seed = TrialName {
        envelope: Envelope::Small,
        number: 1,
    };
    let backend = factory.create(seed).await?;
    super::trial_complete::run_pipeline_control(seed, backend, factory.artifacts()).await
}

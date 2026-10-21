//! Proposed backend::faults::preparation child. No watched carrier is replaced.
//! Needs ONLY setup-helper visibility + explicit1s producer constructor patch.
use super::*;
use trial_complete::Provenance;
pub(super) async fn prepare_overload(
    name: TrialName,
    provenance: Provenance,
    plan: Vec<Slot>,
) -> Result<PreparedBackend, PreparationFailure> {
    let (fixture, bounded_reads) = QualificationFixture::pipeline_control().await;
    let mut prepared = PreparedBackend {
        name,
        mode: PipelineMode::TwoSecondControl,
        provenance,
        plan: Vec::new(),
        full_plan: Vec::new(),
        transport_archive: None,
        native_archive: None,
        optional_notices: None,
        mcp_shutdown: None,
        catalog: ReadCatalog {
            rpc: Vec::new(),
            mcp: Vec::new(),
            router_only_mcp: Default::default(),
        },
        common: BTreeMap::new(),
        saved_evidence: None,
        targets: None,
        reads: None,
        rpc_cases: Vec::new(),
        bounded_reads: Some(bounded_reads),
        bounded_rpc_cases: Vec::new(),
        mcp_inputs: Vec::new(),
        fixture: Some(fixture),
        terminal: None,
        services: None,
        frames: super::super::archive::FrameArchive::new(),
        producers: None,
        watch: None,
        lifecycle: None,
        mcp: None,
        probe: None,
    };
    use futures_util::FutureExt;
    let assembled = std::panic::AssertUnwindSafe(assemble(&mut prepared, plan))
        .catch_unwind()
        .await;
    match assembled {
        Ok(Ok(())) => Ok(prepared),
        Ok(Err(reason)) => Err(PreparationFailure {
            reason,
            retained: prepared,
        }),
        Err(_) => Err(PreparationFailure {
            reason: "actual overload setup panicked; owned cleanup required".into(),
            retained: prepared,
        }),
    }
}
async fn assemble(prepared: &mut PreparedBackend, plan: Vec<Slot>) -> Result<(), String> {
    prepared.prepare_bounded_recipes()?; // unchanged full24000 semantic mapping
    prepared.plan = plan;
    support(prepared).await?;
    prepared.wait_bootstrap_terminal().await?;
    // The shared collector preparation captures accepted evidence exactly once.
    prepared.prepare_collectors()?; // all400 relative due clocks ACKed here
    Ok(())
}
async fn support(prepared: &mut PreparedBackend) -> Result<(), String> {
    let fixture = prepared
        .fixture
        .as_ref()
        .ok_or("actual overload fixture missing")?;
    prepared.terminal = Some(
        super::super::exempt::ExemptTerminal::create(&fixture.rpc, &fixture.active_workspaces[0])
            .await,
    );
    super::super::exempt::install_file_observation_subscriptions(
        &fixture.rpc,
        &fixture.active_workspaces,
    )
    .await;
    let bodies = super::super::producer_bodies::from_fixture(fixture)?;
    prepared.producers = Some(Arc::new(ConcreteProducers::fault_control(
        prepared.name,
        Arc::clone(&fixture.registry),
        bodies,
        Duration::from_secs(1),
    )?));
    Ok(())
}

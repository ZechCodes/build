//! All real support/bootstrap work occurs before the one-shot start boundary.
use super::super::client_watch::TrafficCohort;
use super::super::common_requests::build_common_requests;
use super::super::mcp_adapter::McpIdentity;
use super::*;
use crate::app::tests::runtime::write_wait_qualification::read_recipes::{
    mcp_cases, mcp_read_method, McpCaller,
};
use crate::test_support::finite_service::{PlannedService, ServiceKind};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

impl PreparedBackend {
    pub(super) async fn assemble(&mut self) -> Result<(), String> {
        self.assemble_support().await?;
        self.bind_native_fixture_counts()?;
        self.prepare_collectors()?;
        Ok(())
    }
    // The compatibility control stops at this identical native support boundary,
    // before enrolling any normal caller/watch record.
    pub(super) async fn assemble_support(&mut self) -> Result<(), String> {
        match self.mode {
            PipelineMode::Qualification => self.prepare_recipes()?,
            PipelineMode::TwoSecondControl => self.prepare_bounded_recipes()?,
        }
        self.prepare_support().await?;
        self.wait_bootstrap_terminal().await?;
        Ok(())
    }
    fn prepare_recipes(&mut self) -> Result<(), String> {
        let fixture = self.fixture.as_ref().ok_or("fixture ownership missing")?;
        let reads = fixture
            .read_inputs
            .as_ref()
            .ok_or("genuine ReadyReadInputs missing")?;
        reads.assert_pinned_footprint(&fixture.rpc);
        self.rpc_cases = reads.rpc_cases();
        self.mcp_inputs = fixture
            .active_workspaces
            .iter()
            .map(|workspace| {
                // A primary checkout remains a valid authenticated caller but
                // is deliberately absent from the native workspace list. Use
                // an existing listed sibling in the same project as the saved
                // roster/membership witness; never add a fixture resident.
                let listed = fixture
                    .active_workspaces
                    .iter()
                    .find(|candidate| {
                        candidate.project == workspace.project
                            && candidate.directory
                                != fixture.projects[workspace.project].repository
                    })
                    .ok_or("same-project linked active read target missing")?;
                Ok(reads.mcp_for(workspace, listed))
            })
            .collect::<Result<Vec<_>, String>>()?;
        let cases = mcp_cases(&self.mcp_inputs[0].0);
        self.catalog = ReadCatalog {
            rpc: self
                .rpc_cases
                .iter()
                .map(|case| case.method.to_owned())
                .collect(),
            mcp: cases
                .iter()
                .map(|(_, action)| mcp_read_method(action))
                .collect(),
            router_only_mcp: cases
                .iter()
                .filter(|(caller, _)| *caller == McpCaller::Router)
                .map(|(_, action)| mcp_read_method(action))
                .collect(),
        };
        self.full_plan = super::super::trial_complete::make_plan(self.name, &self.catalog)?;
        self.plan = self.mode.plan(self.name, &self.catalog)?;
        self.reads = Some(Arc::clone(reads));
        let (_, targets) = git_support::prepare(fixture)?;
        self.common = build_common_requests(self.name, &targets, &self.full_plan)?;
        self.targets = Some(targets);
        Ok(())
    }
    fn prepare_bounded_recipes(&mut self) -> Result<(), String> {
        let fixture = self.fixture.as_ref().ok_or("fixture ownership missing")?;
        let seed = self
            .bounded_reads
            .as_ref()
            .ok_or("actual bounded seed missing")?;
        self.bounded_rpc_cases =
            crate::app::tests::runtime::write_wait_qualification::read_recipes::rpc_cases(seed)
                .into_iter()
                .filter(|case| case.method != "capture.get")
                .collect();
        self.catalog = ReadCatalog {
            rpc: self
                .bounded_rpc_cases
                .iter()
                .map(|case| case.method.to_owned())
                .collect(),
            mcp: Vec::new(),
            router_only_mcp: Default::default(),
        };
        self.full_plan = super::super::trial_complete::make_semantic_slots(self.name);
        self.plan = self.mode.plan(self.name, &self.catalog)?;
        let (_, targets) = git_support::prepare(fixture)?;
        self.common = build_common_requests(self.name, &targets, &self.full_plan)?;
        self.targets = Some(targets);
        Ok(())
    }
    async fn prepare_support(&mut self) -> Result<(), String> {
        let fixture = self.fixture.as_ref().ok_or("fixture ownership missing")?;
        self.terminal =
            Some(exempt::ExemptTerminal::create(&fixture.rpc, &fixture.active_workspaces[0]).await);
        exempt::install_file_observation_subscriptions(&fixture.rpc, &fixture.active_workspaces)
            .await;
        let bodies = producer_bodies::from_fixture(fixture)?;
        self.producers = Some(Arc::new(match self.mode {
            PipelineMode::Qualification => {
                ConcreteProducers::new(self.name, Arc::clone(&fixture.registry), bodies)?
            }
            PipelineMode::TwoSecondControl => ConcreteProducers::pipeline_control(
                self.name,
                Arc::clone(&fixture.registry),
                bodies,
            )?,
        }));
        Ok(())
    }
    async fn wait_bootstrap_terminal(&self) -> Result<(), String> {
        let registry = &self
            .fixture
            .as_ref()
            .ok_or("fixture ownership missing")?
            .registry;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let (_, active) = registry.active_snapshot_with_time();
            if active.is_empty() {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(format!(
                    "actual bootstrap roots/children did not drain before attachment: {:?}",
                    active
                        .iter()
                        .map(|row| (&row.original_id, row.id))
                        .collect::<Vec<_>>()
                ));
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        // attach_client_watch additionally validates every prior offered ID,
        // receiver state and all archived cancelled/abandoned rows. No active
        // row is filtered here and no bad history is erased or relabelled.
    }
    pub(super) fn prepare_collectors(&mut self) -> Result<(), String> {
        self.saved_evidence = Some(saved_evidence::SavedEvidence::capture(
            self.fixture.as_ref().ok_or("fixture ownership missing")?,
            self.plan
                .iter()
                .filter(|slot| slot.traffic == super::super::trial_complete::Traffic::Write)
                .map(|slot| slot.original_id.clone())
                .collect(),
        )?);
        let fixture = self.fixture.as_ref().ok_or("fixture ownership missing")?;
        let plans = self.plan.iter().filter_map(finite_plan).collect();
        self.services = Some(FixtureServices::install(
            fixture
                .active_workspaces
                .iter()
                .map(|workspace| workspace.directory.clone())
                .collect(),
            plans,
            None,
        ));
        let frames = Arc::clone(&self.frames);
        fixture
            .state
            .lock()
            .map_err(|_| "fixture app lock poisoned")?
            .frame_clock
            .observe_completed_frames(Arc::new(move |frame| frames.receive(frame)));
        let watch = ClientWatch::new(Arc::new(Instant::now));
        for slot in &self.plan {
            watch.register_relative_at(
                &slot.original_id,
                Duration::from_micros(slot.due_us),
                cohort(slot),
            );
        }
        let guard = watch.watch(); // synchronous full relative-index ACK
        let owner = watch_owner::WatchOwner::new(guard);
        let clients = owner.observer();
        self.lifecycle = Some(owner);
        self.watch = Some(watch.clone());
        let mcp = match self.mode {
            PipelineMode::Qualification => McpHarness::new(
                Arc::clone(&fixture.state),
                Arc::clone(&fixture.registry),
                actual_identities(
                    fixture,
                    self.reads.as_deref().ok_or("read fixture missing")?,
                ),
                watch,
            )?,
            // Exact explicit control constructor is a sibling-owned prerequisite;
            // qualification new still requires genuine8Coding+1passiveRouter.
            PipelineMode::TwoSecondControl => McpHarness::pipeline_control(
                Arc::clone(&fixture.state),
                Arc::clone(&fixture.registry),
                coding_identities(fixture),
                watch,
            )?,
        };
        self.probe = Some(Arc::new(probe::Probe {
            registry: Arc::clone(&fixture.registry),
            clients,
            producers: Arc::clone(self.producers.as_ref().ok_or("producer handles missing")?),
            mcp: mcp.observation(),
            finite_ceilings: self
                .plan
                .iter()
                .filter_map(|slot| {
                    slot.finite_ceiling_us
                        .map(|us| (slot.original_id.clone(), Duration::from_micros(us)))
                })
                .collect(),
            start: OnceLock::new(),
        }));
        self.mcp = Some(mcp);
        // ControlBackend::new attaches exactly once after the bounded real
        // terminal-bootstrap wait above. Its tracked preflight rechecks all
        // actual history/receiver rows without reset. Normal remains blocked.
        // No T0 is set here.
        Ok(())
    }
}
fn finite_plan(slot: &Slot) -> Option<PlannedService> {
    let target = Duration::from_micros(slot.finite_target_us?);
    let kind = match slot.class? {
        super::super::trial_complete::WriteClass::FilesystemProcess => ServiceKind::Filesystem,
        super::super::trial_complete::WriteClass::GitProvider => ServiceKind::Git,
        super::super::trial_complete::WriteClass::State => return None,
    };
    Some(PlannedService {
        original_id: slot.original_id.clone(),
        kind,
        target,
    })
}
fn cohort(slot: &Slot) -> TrafficCohort {
    match slot.traffic {
        super::super::trial_complete::Traffic::Write => TrafficCohort::SemanticWrite,
        super::super::trial_complete::Traffic::Read => TrafficCohort::Read,
        super::super::trial_complete::Traffic::Exempt => TrafficCohort::Exempt,
    }
}
pub(super) fn actual_identities(
    fixture: &QualificationFixture,
    reads: &ReadyReadInputs,
) -> Vec<McpIdentity> {
    let mut identities = coding_identities(fixture);
    let (agent, token) = reads.router_endpoint();
    identities.push(McpIdentity {
        agent: agent.into(),
        token: token.into(),
        read_only: true,
    });
    identities
}

fn coding_identities(fixture: &QualificationFixture) -> Vec<McpIdentity> {
    fixture
        .active_workspaces
        .iter()
        .map(|workspace| McpIdentity {
            agent: workspace.agent_id.clone(),
            token: format!("qual-token-{}", workspace.agent_id),
            read_only: false,
        })
        .collect()
}

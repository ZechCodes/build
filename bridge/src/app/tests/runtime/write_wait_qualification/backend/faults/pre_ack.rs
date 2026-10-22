//! Real six-second AppState/source-exclusion stalls through common RpcHarness.
use super::super::super::common_requests::{
    build_common_requests, ActualRequest, CommonRequest, CommonTargets, ExpectedEffect, SavedEffect,
};
use super::context::Context;
use super::evidence::{native, us, watch};
use super::hold::{Hold, Target};
use super::*;

pub(in crate::app::tests::runtime::write_wait_qualification) async fn pre_ack_app(
    name: TrialName,
    artifacts: &FaultArtifacts,
) -> Result<FaultRun, String> {
    run(name, false, artifacts).await
}
pub(in crate::app::tests::runtime::write_wait_qualification) async fn pre_ack_exclusion(
    name: TrialName,
    artifacts: &FaultArtifacts,
) -> Result<FaultRun, String> {
    run(name, true, artifacts).await
}
async fn run(
    name: TrialName,
    exclusion: bool,
    artifacts: &FaultArtifacts,
) -> Result<FaultRun, String> {
    let mut step = Step::new(name, exclusion).await?;
    step.start(exclusion).await;
    step.stall(exclusion).await;
    step.release().await;
    step.join_call().await;
    step.finish(artifacts).await
}
struct ReportEnd {
    joined: bool,
    client_watch_report: Value,
    client_rows: Vec<Value>,
    completed: Option<WriteSnapshot>,
    actual_registry_boundary: Value,
    actual_decoded_reply: Option<Value>,
    effect_checks: Value,
}
struct Step {
    context: Option<Context>,
    slot: Slot,
    request: CommonRequest,
    targets: CommonTargets,
    case: &'static str,
    holder: Option<Hold>,
    holder_joined: bool,
    call: Option<tokio::task::JoinHandle<ReplyProof>>,
    reply: Option<ReplyProof>,
    active: Option<WriteSnapshot>,
    fired: Vec<Value>,
    hold_us: u64,
    errors: Vec<String>,
}
impl Step {
    async fn new(name: TrialName, exclusion: bool) -> Result<Self, String> {
        let slots = trial_complete::make_semantic_slots(name);
        let slot = slots[if exclusion { 36 } else { 32 }].clone();
        let context = Context::new(&slot.original_id).await?;
        let (targets, request) = match prepare_request(&context, name, &slots, &slot) {
            Ok(value) => value,
            Err(error) => {
                let _ = context.shutdown().await;
                return Err(error);
            }
        };
        if !matches!(&request.request, ActualRequest::Rpc { .. }) {
            let _ = context.shutdown().await;
            return Err("physical preACK requires actual SPA recipe".into());
        }
        Ok(Self {
            context: Some(context),
            slot,
            request,
            targets,
            case: if exclusion {
                "pre-ack-source-exclusion"
            } else {
                "pre-ack-app"
            },
            holder: None,
            holder_joined: false,
            call: None,
            reply: None,
            active: None,
            fired: Vec::new(),
            hold_us: 0,
            errors: Vec::new(),
        })
    }
    async fn start(&mut self, exclusion: bool) {
        let context = self.context.as_mut().expect("owned context");
        let target = if exclusion {
            Target::Exclusion(self.targets.active[self.slot.owner].path.clone())
        } else {
            Target::App(Arc::clone(&context.fixture.state))
        };
        let (holder, acquired) = Hold::start(target).await;
        self.holder = Some(holder);
        if let Err(error) = acquired {
            self.errors.push(error);
        }
        context.intended_offer(&self.slot.original_id).await;
        let ActualRequest::Rpc { method, params } = &self.request.request else {
            unreachable!("validated actual RPC variant")
        };
        let fixture = Arc::clone(&context.fixture);
        let original = self.slot.original_id.clone();
        let params = params.clone();
        let method = *method;
        let caller = self.slot.caller;
        self.call = Some(tokio::spawn(async move {
            fixture.rpc.request(caller, &original, method, params).await
        }));
    }
    async fn stall(&mut self, exclusion: bool) {
        let context = self.context.as_ref().expect("owned context");
        let kind = if exclusion {
            WaitKind::ResourceLock
        } else {
            WaitKind::AppLock
        };
        if let Some(started) = context.wait_kind(&self.slot.original_id, kind).await {
            tokio::time::sleep_until(tokio::time::Instant::from_std(started + PHYSICAL_HOLD)).await;
        } else {
            self.errors
                .push("actual selected wait never became active".into());
        }
        self.active = context.active(&self.slot.original_id);
        self.fired = context
            .fired
            .lock()
            .unwrap()
            .iter()
            .map(|row| native(row, context.anchor))
            .collect();
    }
    async fn release(&mut self) {
        match self
            .holder
            .as_mut()
            .expect("actual holder")
            .release_and_join()
            .await
        {
            Ok(value) => {
                self.holder_joined = true;
                self.hold_us = us(value.held_for);
                if value.safety_release {
                    self.errors.push("holder safety release fired".into());
                }
            }
            Err(error) => self.errors.push(error),
        }
    }
    async fn join_call(&mut self) {
        match tokio::time::timeout(
            CLEANUP_BOUND,
            self.call.as_mut().expect("actual original caller"),
        )
        .await
        {
            Ok(Ok(reply)) => {
                self.reply = Some(reply);
                self.call.take();
            }
            Ok(Err(error)) => {
                self.errors.push(error.to_string());
                self.call.take();
            }
            Err(_) => self
                .errors
                .push("actual released RPC did not join; no cancellation/retry/replacement".into()),
        }
    }
    async fn finish(mut self, artifacts: &FaultArtifacts) -> Result<FaultRun, String> {
        if self.reply.is_none() || !self.holder_joined {
            return self.retain(artifacts);
        }
        let reply = self.reply.take().expect("actual decoded final");
        let effect = verify_effect(&self.request.effect, &reply.reply, &self.request);
        if let Err(error) = &effect {
            self.errors.push(error.clone());
        }
        let context = self.context.take().expect("owned context");
        let registry = Arc::clone(&context.registry);
        let clock = context.watch.clone();
        let anchor = context.anchor;
        let (joined, client_report, unfinished) = match context.shutdown().await {
            Ok(report) => (true, watch(&report, anchor), None),
            Err(retained) => (
                false,
                json!({"error":"native cleanup unfinished"}),
                Some(retained),
            ),
        };
        let (completed, actual_registry_boundary) =
            evidence::boundary_archive(&registry, anchor, &self.slot.original_id, &mut self.errors);
        let report=self.report(anchor,ReportEnd{joined,client_watch_report:client_report,client_rows:super::context::clients(&clock,anchor),completed,actual_registry_boundary,actual_decoded_reply:Some(reply.reply),effect_checks:json!({"native_finite_effect_verified":effect.is_ok(),"witness":effect.ok().map(|witness|format!("{witness:?}"))})});
        let artifact = artifacts.write(&report.case, &report)?;
        Ok(FaultRun {
            report,
            artifact,
            unfinished,
        })
    }
    fn retain(mut self, artifacts: &FaultArtifacts) -> Result<FaultRun, String> {
        let context = self.context.take().expect("owned context");
        self.errors.push(
            "actual caller/holder cleanup unconfirmed; roots and original monitors retained".into(),
        );
        let decoded = self.reply.take().map(|reply| reply.reply);
        let (_, actual_registry_boundary) = evidence::boundary_archive(
            &context.registry,
            context.anchor,
            &self.slot.original_id,
            &mut self.errors,
        );
        let report = self.report(
            context.anchor,
            ReportEnd {
                joined: false,
                client_watch_report: json!({"unfinished":true}),
                client_rows: context.clients(),
                completed: None,
                actual_registry_boundary,
                actual_decoded_reply: decoded,
                effect_checks: json!({"verified":false}),
            },
        );
        let mut retained = super::context::retained(Arc::clone(&context.fixture));
        retained.caller = self.call.take();
        if let Some(holder) = self.holder.take() {
            retained.holders.push(holder);
        }
        std::mem::forget(context);
        let artifact = artifacts.write(&report.case, &report)?;
        Ok(FaultRun {
            report,
            artifact,
            unfinished: Some(retained),
        })
    }
    fn report(&self, anchor: Instant, end: ReportEnd) -> FaultReport {
        let ReportEnd {
            joined,
            client_watch_report,
            client_rows,
            completed,
            actual_registry_boundary,
            actual_decoded_reply,
            effect_checks,
        } = end;
        FaultReport {
            schema: "build.physical-fault.v1-draft",
            qualification: false,
            case: self.case.into(),
            original_id: self.slot.original_id.clone(),
            durable_operation_id: self.request.durable_operation_id.clone(),
            physical_hold_us: self.hold_us,
            observed_wait_before_release: self.active.as_ref().map(|row| native(row, anchor)),
            completed_original: completed.as_ref().map(|row| native(row, anchor)),
            actual_registry_boundary,
            client_rows,
            client_watch_report,
            registry_watch_reports: self.fired.clone(),
            actual_decoded_reply,
            effect_checks,
            errors: self.errors.clone(),
            cleanup_joined: joined,
        }
    }
}
fn verify_effect(
    effect: &ExpectedEffect,
    reply: &Value,
    request: &CommonRequest,
) -> Result<trial_complete::OracleWitness, String> {
    let actual = match effect {
        ExpectedEffect::File { path, .. } => SavedEffect::File {
            actual_path: path.clone(),
            actual_bytes: std::fs::read(path).map_err(|e| e.to_string())?,
        },
        ExpectedEffect::GitHead { directory, .. } => actual_git_head(directory)?,
        _ => return Err("physical finite verifier received non-finite case".into()),
    };
    request.verify_saved(reply, actual)
}
fn actual_git_head(directory: &Path) -> Result<SavedEffect, String> {
    let repo = git2::Repository::open(directory).map_err(|e| e.to_string())?;
    let head = repo.head().map_err(|e| e.to_string())?;
    let actual_commit = head
        .peel_to_commit()
        .map_err(|e| e.to_string())?
        .id()
        .to_string();
    Ok(SavedEffect::GitHead {
        actual_directory: directory.to_owned(),
        actual_symbolic_head: head.name().ok_or("actual HEAD missing")?.to_owned(),
        actual_commit,
    })
}
fn prepare_request(
    context: &Context,
    name: TrialName,
    slots: &[Slot],
    slot: &Slot,
) -> Result<(CommonTargets, CommonRequest), String> {
    let targets = super::super::git_support::prepare(&context.fixture)?;
    let requests = build_common_requests(name, &targets, slots)?;
    let request = requests
        .get(&slot.original_id)
        .cloned()
        .ok_or("fixed actual finite fault recipe missing")?;
    Ok((targets, request))
}

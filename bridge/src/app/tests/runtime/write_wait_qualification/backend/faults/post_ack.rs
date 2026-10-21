//! Real post-final required delivery; original ID survives release and cleanup.
//! Existing fixture is exposed by seams/post-reply-fixture-visibility.patch.
use super::evidence::{native, us, watch};
use super::hold::{Hold, Target};
use super::*;
use crate::app::tests::runtime::write_wait_qualification::post_reply::DeliveryFixture;
use crate::app::tests::{wait_for_agent_tab, wait_for_deliveries};
const ORIGINAL: &str = "qual/fault/post-ack-required/original";
struct Step {
    fixture: Option<DeliveryFixture>,
    clock: ClientWatch,
    guard: Option<ClientWatchGuard>,
    anchor: Instant,
    registry: Arc<WriteWaitRegistry>,
    watchdog: Option<crate::timing::write_wait::Watchdog>,
    fired: Arc<Mutex<Vec<WriteSnapshot>>>,
    call: Option<tokio::task::JoinHandle<ReplyProof>>,
    reply: Option<ReplyProof>,
    holder: Option<Hold>,
    holder_joined: bool,
    active: Option<WriteSnapshot>,
    hold_us: u64,
    converged: bool,
    effects: Value,
    errors: Vec<String>,
}
pub(in crate::app::tests::runtime::write_wait_qualification) async fn post_ack_required(
    artifacts: &FaultArtifacts,
) -> Result<FaultRun, String> {
    let mut step = Step::new().await?;
    step.offer().await;
    step.hold_required().await;
    step.release().await;
    step.converge().await;
    step.finish(artifacts).await
}
impl Step {
    async fn new() -> Result<Self, String> {
        let mut fixture = DeliveryFixture::new();
        let clock = ClientWatch::new(Arc::new(Instant::now));
        clock.register_relative_at(ORIGINAL, Duration::ZERO, TrafficCohort::SemanticWrite);
        let guard = clock.watch();
        let Some(rpc) = Arc::get_mut(&mut fixture.rpc) else {
            let _ = cleanup_raw(fixture).await;
            return Err("fresh actual RPC carrier unexpectedly shared".into());
        };
        rpc.attach_client_watch(clock.clone());
        let fired = Arc::new(Mutex::new(Vec::new()));
        let reports = Arc::clone(&fired);
        let registry = Arc::clone(&fixture.registry);
        let watchdog = registry.watchdog(
            Duration::from_millis(5),
            Arc::new(move |row| reports.lock().unwrap().push(row)),
        );
        Ok(Self {
            fixture: Some(fixture),
            clock,
            guard: Some(guard),
            anchor: Instant::now(),
            registry,
            watchdog: Some(watchdog),
            fired,
            call: None,
            reply: None,
            holder: None,
            holder_joined: false,
            active: None,
            hold_us: 0,
            converged: false,
            effects: Value::Null,
            errors: Vec::new(),
        })
    }
    async fn offer(&mut self) {
        self.anchor = Instant::now() + Duration::from_millis(100);
        self.clock.start_at(self.anchor);
        tokio::time::sleep_until(tokio::time::Instant::from_std(self.anchor)).await;
        self.clock.offer(ORIGINAL);
        let rpc = Arc::clone(&self.fixture.as_ref().expect("owned native fixture").rpc);
        self.call = Some(tokio::spawn(async move {
            rpc.request(
                0,
                ORIGINAL,
                "agent.start",
                json!({"id":"run-logical-required-delivery"}),
            )
            .await
        }));
        match tokio::time::timeout(CLEANUP_BOUND, self.call.as_mut().expect("actual caller")).await
        {
            Ok(Ok(reply)) => {
                self.reply = Some(reply);
                self.call.take();
            }
            _ => self
                .errors
                .push("original caller unfinished/failed; never replaced".into()),
        }
    }
    async fn hold_required(&mut self) {
        let fixture = self.fixture.as_mut().expect("owned native fixture");
        if self.reply.is_none() {
            let _ = fixture.spawn_release.send(());
            return;
        }
        let arrived = fixture.spawned.recv_timeout(CLEANUP_BOUND).is_ok();
        while fixture.waits.try_recv().is_ok() {}
        let (holder, held) = Hold::start(Target::App(Arc::clone(&fixture.state))).await;
        self.holder = Some(holder);
        if let Err(error) = held {
            self.errors.push(error);
        }
        let _ = fixture.spawn_release.send(());
        let entered = if arrived {
            required_wait(&self.registry).await
        } else {
            None
        };
        if let Some(started) = entered {
            tokio::time::sleep_until(tokio::time::Instant::from_std(started + PHYSICAL_HOLD)).await;
        } else {
            self.errors
                .push("actual postACK required AppLock wait not reached".into());
        }
        self.active = self
            .registry
            .active_snapshot_with_time()
            .1
            .into_iter()
            .find(|row| row.original_id == ORIGINAL);
    }
    async fn release(&mut self) {
        let Some(holder) = &mut self.holder else {
            return;
        };
        match holder.release_and_join().await {
            Ok(value) => {
                self.hold_us = us(value.held_for);
                self.holder_joined = true;
                if value.safety_release {
                    self.errors.push("holder safety release fired".into());
                }
            }
            Err(error) => self.errors.push(error),
        }
    }
    async fn converge(&mut self) {
        if self.reply.is_none() || !self.holder_joined {
            return;
        }
        let fixture = self.fixture.as_ref().expect("owned native fixture");
        use futures_util::FutureExt;
        self.converged = tokio::time::timeout(
            CLEANUP_BOUND,
            std::panic::AssertUnwindSafe(async {
                wait_for_agent_tab(&fixture.state, &fixture.key).await;
                wait_for_deliveries(&fixture.state).await;
            })
            .catch_unwind(),
        )
        .await
        .is_ok_and(|result| result.is_ok());
        if self.converged {
            self.effects = actual_effects(fixture, self.reply.as_ref().expect("actual reply"));
        } else {
            self.errors
                .push("actual required delivery convergence unfinished".into());
        }
    }
    async fn finish(mut self, artifacts: &FaultArtifacts) -> Result<FaultRun, String> {
        if !self.converged || !self.holder_joined {
            return self.retain(artifacts);
        }
        let cleanup = cleanup_raw(self.fixture.take().expect("owned native fixture")).await;
        let (mut joined, unfinished) = match cleanup {
            Ok(()) => (true, None),
            Err(retained) => {
                self.errors.push("actual native cleanup unconfirmed".into());
                (false, Some(retained))
            }
        };
        self.watchdog.take();
        let guard = self.guard.take().expect("existing client watchdog");
        let client_report = match tokio::task::spawn_blocking(move || guard.finish()).await {
            Ok(report) => watch(&report, self.anchor),
            Err(error) => {
                joined = false;
                self.errors.push(error.to_string());
                json!({"error":"watchdog join failed"})
            }
        };
        let (completed, actual_registry_boundary) =
            evidence::boundary_archive(&self.registry, self.anchor, ORIGINAL, &mut self.errors);
        let report = self.report(
            joined,
            client_report,
            completed.as_ref(),
            actual_registry_boundary,
        );
        let artifact = artifacts.write(&report.case, &report)?;
        Ok(FaultRun {
            report,
            artifact,
            unfinished,
        })
    }
    fn retain(mut self, artifacts: &FaultArtifacts) -> Result<FaultRun, String> {
        let (_, actual_registry_boundary) =
            evidence::boundary_archive(&self.registry, self.anchor, ORIGINAL, &mut self.errors);
        let report = self.report(
            false,
            json!({"unfinished":true}),
            None,
            actual_registry_boundary,
        );
        let mut retained = raw_retained(self.fixture.take().expect("owned native fixture"));
        retained.caller = self.call.take();
        if let Some(holder) = self.holder.take() {
            retained.holders.push(holder);
        }
        if let Some(guard) = self.guard.take() {
            std::mem::forget(guard);
        }
        if let Some(watchdog) = self.watchdog.take() {
            std::mem::forget(watchdog);
        }
        let artifact = artifacts.write(&report.case, &report)?;
        Ok(FaultRun {
            report,
            artifact,
            unfinished: Some(retained),
        })
    }
    fn report(
        &self,
        joined: bool,
        client_report: Value,
        completed: Option<&WriteSnapshot>,
        actual_registry_boundary: Value,
    ) -> FaultReport {
        FaultReport {
            schema: "build.physical-fault.v1-draft",
            qualification: false,
            case: "post-ack-required".into(),
            original_id: ORIGINAL.into(),
            durable_operation_id: self.reply.as_ref().and_then(|value| {
                value.reply["result"]["operation_id"]
                    .as_str()
                    .map(str::to_owned)
            }),
            physical_hold_us: self.hold_us,
            observed_wait_before_release: self.active.as_ref().map(|row| native(row, self.anchor)),
            completed_original: completed.map(|row| native(row, self.anchor)),
            actual_registry_boundary,
            client_rows: clients(&self.clock, self.anchor),
            client_watch_report: client_report,
            registry_watch_reports: self
                .fired
                .lock()
                .unwrap()
                .iter()
                .map(|row| native(row, self.anchor))
                .collect(),
            actual_decoded_reply: self.reply.as_ref().map(|value| value.reply.clone()),
            effect_checks: self.effects.clone(),
            errors: self.errors.clone(),
            cleanup_joined: joined,
        }
    }
}
async fn required_wait(registry: &WriteWaitRegistry) -> Option<Instant> {
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(started) = registry
            .active_snapshot_with_time()
            .1
            .iter()
            .filter(|row| {
                row.original_id == ORIGINAL && row.root_finished && row.acknowledged_at.is_some()
            })
            .flat_map(|row| &row.active)
            .find(|span| span.category == IntervalCategory::KnownWait(WaitKind::AppLock))
            .map(|span| span.started)
        {
            return Some(started);
        }
        if Instant::now() >= until {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}
fn actual_effects(fixture: &DeliveryFixture, reply: &ReplyProof) -> Value {
    let Ok(app) = fixture.state.try_lock() else {
        return json!({"unknown":"actual state still locked"});
    };
    let tab = app.session_registry.test_tab(&fixture.key);
    json!({"positive_final":reply.reply["ok"]==true,"reserved_tab_matches":reply.reply["result"]["term_id"]==fixture.key.tab_id,"actual_session_instance_present":tab.is_some_and(|tab|tab.session_instance.is_some())})
}
async fn cleanup_raw(fixture: DeliveryFixture) -> Result<(), RetainedFault> {
    let keys = {
        let app = fixture
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        app.session_registry
            .test_tabs()
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>()
    };
    let mut receipts = Vec::new();
    let mut retirement_failed = false;
    for key in keys {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            fixture
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .retire_tab(&key, "physical_fault_cleanup")
        })) {
            Ok(Some(receipt)) => receipts.push(receipt),
            Ok(None) => {}
            Err(_) => retirement_failed = true,
        }
    }
    let joined = tokio::task::spawn_blocking(move || {
        receipts
            .into_iter()
            .map(|receipt| {
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    receipt.wait(Duration::from_secs(2))
                }))
                .unwrap_or(false)
            })
            .collect::<Vec<_>>()
    })
    .await
    .is_ok_and(|all| all.into_iter().all(|value| value));
    if !joined || retirement_failed {
        return Err(raw_retained(fixture));
    }
    finish_carrier(fixture).await
}
async fn finish_carrier(fixture: DeliveryFixture) -> Result<(), RetainedFault> {
    let DeliveryFixture {
        _directory,
        state,
        registry: _,
        rpc,
        key: _,
        spawned: _,
        spawn_release: _,
        waits: _,
    } = fixture;
    let rpc = match Arc::try_unwrap(rpc) {
        Ok(rpc) => rpc,
        Err(rpc) => {
            return Err(RetainedFault {
                fixture: None,
                caller: None,
                raw_state: Some(state),
                raw_rpc: Some(rpc),
                private_roots: vec![_directory],
                holders: Vec::new(),
            })
        }
    };
    use futures_util::FutureExt;
    let clean = std::panic::AssertUnwindSafe(rpc.finish_report())
        .catch_unwind()
        .await
        .is_ok_and(|report| report.is_clean());
    let weak = Arc::downgrade(&state);
    drop(state);
    let until = Instant::now() + CLEANUP_BOUND;
    while weak.upgrade().is_some() && Instant::now() < until {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    if !clean || weak.upgrade().is_some() {
        let _ = _directory.keep();
        return Err(RetainedFault {
            fixture: None,
            caller: None,
            raw_state: weak.upgrade(),
            raw_rpc: None,
            private_roots: Vec::new(),
            holders: Vec::new(),
        });
    }
    drop(_directory);
    Ok(())
}
fn raw_retained(fixture: DeliveryFixture) -> RetainedFault {
    let DeliveryFixture {
        _directory,
        state,
        registry: _,
        rpc,
        key: _,
        spawned: _,
        spawn_release: _,
        waits: _,
    } = fixture;
    RetainedFault {
        fixture: None,
        caller: None,
        raw_state: Some(state),
        raw_rpc: Some(rpc),
        private_roots: vec![_directory],
        holders: Vec::new(),
    }
}
fn clients(clock: &ClientWatch, anchor: Instant) -> Vec<Value> {
    clock.snapshot().iter().map(|row|json!({"original_id":row.original_id,"intended_us":evidence::at(row.intended_at,anchor),"sent_us":row.sent_at.map(|v|evidence::at(v,anchor)),"decoded_final_us":row.final_at.map(|v|evidence::at(v,anchor)),"breached":row.breached,"cancelled":row.cancelled})).collect()
}

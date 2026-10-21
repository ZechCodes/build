//! Executable rare-family scripts with real RPC, effect and origin proofs.
use super::rpc::{ReplyProof, RpcHarness};
use crate::app::AppState;
use crate::timing::write_wait::{
    Watchdog, WriteCohort, WriteSnapshot, WriteWaitRegistry, WAIT_BUDGET,
};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

mod agents_services;
mod filesystem_git;
mod lifecycle;
mod runs;
mod tracker_reviews;

static FIXTURE_ID: AtomicU64 = AtomicU64::new(0);

pub(super) struct VerifiedStep {
    pub method: String,
    pub original_id: String,
    pub effect: Value,
    pub reply: Value,
    pub wait: WriteSnapshot,
}

pub(super) struct RareFixture {
    family: &'static str,
    fixture_id: u64,
    next: AtomicU64,
    rpc: RpcHarness,
    _watchdog: Watchdog,
    breached: Arc<AtomicBool>,
    verified: Vec<VerifiedStep>,
}

impl RareFixture {
    pub(super) fn new(family: &'static str, app: AppState) -> Self {
        Self::shared(family, app.shared())
    }

    pub(super) fn shared(family: &'static str, state: Arc<Mutex<AppState>>) -> Self {
        let registry = WriteWaitRegistry::new();
        let breached = Arc::new(AtomicBool::new(false));
        let reported = Arc::clone(&breached);
        let watchdog = registry.watchdog(
            Duration::from_millis(10),
            Arc::new(move |row| {
                reported.store(true, Ordering::Release);
                eprintln!("rare_active_write_breach={row:?}");
            }),
        );
        Self {
            family,
            fixture_id: FIXTURE_ID.fetch_add(1, Ordering::Relaxed),
            next: AtomicU64::new(0),
            rpc: RpcHarness::new(state, registry),
            _watchdog: watchdog,
            breached,
            verified: Vec::new(),
        }
    }

    pub(super) fn state(&self) -> &Arc<Mutex<AppState>> {
        &self.rpc.state
    }

    /// Keep the real service held until the request root finishes, then capture
    /// its retained descendant. A transient root after reply enqueue is not a
    /// service obligation. Finish/join the service before asserting this value.
    pub(super) async fn origin_active(&self, proof: &ReplyProof) -> bool {
        let deadline = Instant::now() + WAIT_BUDGET;
        loop {
            let row = self
                .rpc
                .registry
                .snapshot()
                .into_iter()
                .find(|row| row.original_id == proof.original_id);
            if let Some(row) = row.filter(|row| row.root_finished) {
                return !row.outstanding.is_empty() || !row.active.is_empty();
            }
            if Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    }

    pub(super) async fn call(&self, method: &str, params: Value) -> ReplyProof {
        assert!(
            !self.breached.load(Ordering::Acquire),
            "the rare fixture cannot offer more work after a watchdog failure"
        );
        let sequence = self.next.fetch_add(1, Ordering::Relaxed);
        let id = format!(
            "qual/rare/{}/{}/{sequence:04}",
            self.family, self.fixture_id
        );
        let proof = self.rpc.request(0, &id, method, params).await;
        // Print the captured response even on failure; no failed caller can
        // disappear from the artifact because its effect assertion panicked.
        eprintln!(
            "rare_reply={}",
            json!({
                "family": self.family,
                "original_id": proof.original_id,
                "method": proof.method,
                "reply": proof.reply,
            })
        );
        assert_eq!(proof.reply["ok"], true, "{method}: {}", proof.reply);
        proof
    }

    /// The family supplies actual independently asserted before/after evidence.
    /// This method cannot turn a receipt or an unfinished origin into success.
    pub(super) async fn verify(&mut self, proof: ReplyProof, effect: Value) {
        assert!(effect.is_object() && !effect.as_object().unwrap().is_empty());
        let wait = self.settled_origin(&proof.original_id).await;
        assert_eq!(wait.method, proof.method);
        assert_eq!(wait.cohort, WriteCohort::SemanticWrite);
        assert!(
            wait.admitted_at.is_some(),
            "the actual write was not admitted"
        );
        assert!(!wait.cancelled && !wait.abandoned && !wait.breached());
        assert!(wait.conservative_upper <= WAIT_BUDGET);
        assert!(wait.max_chargeable_interval <= WAIT_BUDGET);
        assert!(
            !self
                .verified
                .iter()
                .any(|step| step.original_id == proof.original_id),
            "a reply cannot count twice: {}",
            proof.original_id
        );
        self.verified.push(VerifiedStep {
            method: proof.method,
            original_id: proof.original_id,
            effect,
            reply: proof.reply,
            wait,
        });
    }

    async fn settled_origin(&self, original_id: &str) -> WriteSnapshot {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let row = self
                .rpc
                .registry
                .snapshot()
                .into_iter()
                .find(|row| row.original_id == original_id);
            if let Some(row) = row {
                assert!(!row.breached(), "active rare origin breached: {row:?}");
                if row.root_finished && row.outstanding.is_empty() && row.active.is_empty() {
                    return row;
                }
            }
            // Failure does not claim completion. The outer owned-runtime runner
            // retains its registry dump and performs bounded cleanup.
            assert!(
                Instant::now() < deadline,
                "rare origin did not settle: {original_id}; {:?}",
                self.rpc.registry.snapshot()
            );
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    }

    pub(super) async fn finish(self) -> Vec<VerifiedStep> {
        assert!(
            !self.breached.load(Ordering::Acquire),
            "an active rare write exceeded its budget"
        );
        assert!(
            self.rpc.registry.active_snapshot().is_empty(),
            "rare fixture still owns active requests: {:?}",
            self.rpc.registry.active_snapshot()
        );
        for step in &self.verified {
            eprintln!(
                "rare_verified={}",
                json!({
                    "method":step.method,"original_id":step.original_id,"effect":step.effect,
                    "reply":step.reply,"known_wait_ms":step.wait.known_wait_union.as_secs_f64()*1000.0,
                    "sqlite_service_ms":step.wait.sql_service_union.as_secs_f64()*1000.0,
                    "conservative_upper_ms":step.wait.conservative_upper.as_secs_f64()*1000.0,
                    "external_envelope_ms":step.wait.own_external_union.as_secs_f64()*1000.0,
                    "external_active_ms":step.wait.own_external_active.as_secs_f64()*1000.0,
                })
            );
        }
        self.rpc.finish().await;
        self.verified
    }
}

pub(super) async fn execute_all() -> Vec<VerifiedStep> {
    let mut steps = filesystem_git::execute().await;
    steps.extend(tracker_reviews::execute().await);
    steps.extend(runs::execute().await);
    steps.extend(lifecycle::execute().await);
    steps.extend(agents_services::execute().await);
    steps
}

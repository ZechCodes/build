//! Qualification-only reporting of the existing typed optional-notice policy.
//! Enrollment never bypasses native recipient or accepted-publication checks.
use super::super::common_requests::{ActualRequest, ExpectedEffect};
use super::super::trial_complete::Traffic;
use super::*;
use crate::app::AppState;
use crate::mcp::BridgeAction;
use crate::timing::optional_notices::{OptionalNoticeKind as Kind, OptionalNotices};
use crate::timing::write_wait::{WriteCohort, WriteWaitRegistry};
use serde_json::{json, Value};
use std::time::Duration;

#[path = "optional_notices/tests.rs"]
mod tests;

pub(super) struct OptionalNoticeEvidence {
    policy: Arc<OptionalNotices>,
    enabled: bool,
    declared: Vec<(String, Kind)>,
    before_cleanup: Option<Value>,
    queue_cleanup: Option<Result<usize, String>>,
}
impl OptionalNoticeEvidence {
    fn capture(
        app: &AppState,
        plan: &[Slot],
        common: &BTreeMap<String, CommonRequest>,
        enabled: bool,
    ) -> Result<Self, String> {
        let declared = declarations(plan, common)?;
        let enrolled = if enabled {
            declared.clone()
        } else {
            Vec::new()
        };
        Ok(Self {
            policy: app.frame_clock.capture_optional_notices(enrolled),
            enabled,
            declared,
            before_cleanup: None,
            queue_cleanup: None,
        })
    }
    fn freeze_before_cleanup(&mut self) -> Result<(), String> {
        if self.before_cleanup.is_some() {
            return Err("optional notice cleanup evidence already captured".into());
        }
        // This snapshot survives cancellation: censored ages are never counted
        // as completed delivery lifetimes, including in the final raw export.
        self.before_cleanup = Some(checkpoint(&self.policy));
        Ok(())
    }
    fn cancel_queued(&mut self, app: &mut AppState) -> Result<usize, String> {
        if self.before_cleanup.is_none() || self.queue_cleanup.is_some() {
            return Err("optional cleanup requires one pre-cancel snapshot".into());
        }
        let result = app
            .cancel_optional_notices_for_cleanup()
            .map_err(str::to_owned);
        self.queue_cleanup = Some(result.clone());
        result
    }
    pub(super) fn report(&self, _native_cleanup_attempted: bool) -> Value {
        Value::Null // TESTFIRST: no report consumer or final evidence yet.
    }
}
impl PreparedBackend {
    pub(super) fn prepare_optional_notices(&mut self) -> Result<(), String> {
        if self.optional_notices.is_some() {
            return Err("optional notice capture already installed".into());
        }
        let fixture = self.fixture.as_ref().ok_or("owned fixture missing")?;
        let app = fixture
            .state
            .lock()
            .map_err(|_| "fixture app lock poisoned")?;
        self.optional_notices = Some(OptionalNoticeEvidence::capture(
            &app,
            &self.plan,
            &self.common,
            true,
        )?);
        Ok(())
    }
    pub(super) async fn drain_optional_notices(
        &mut self,
        deadline: tokio::time::Instant,
    ) -> Result<(), String> {
        let fixture = self.fixture.as_ref().ok_or("owned fixture missing")?;
        let evidence = self
            .optional_notices
            .as_mut()
            .ok_or("actual optional notice capture missing")?;
        // Full final-evidence snapshots are taken without the AppState guard.
        evidence.freeze_before_cleanup()?;
        {
            let mut app = fixture
                .state
                .lock()
                .map_err(|_| "fixture app lock poisoned")?;
            evidence.cancel_queued(&mut app)?;
        }
        wait_running_notices(&fixture.registry, deadline).await
    }
}
async fn wait_running_notices(
    registry: &WriteWaitRegistry,
    deadline: tokio::time::Instant,
) -> Result<(), String> {
    loop {
        // Existing bounded active-state reader only. Full samples are taken at
        // the two report boundaries, never polled as a progress signal.
        let active = registry.active_snapshot();
        if active.is_empty() {
            return Ok(());
        }
        if active
            .iter()
            .any(|row| row.cohort != WriteCohort::OptionalNotice)
        {
            return Err(
                "required work appeared during optional cleanup; ownership retained".into(),
            );
        }
        if tokio::time::Instant::now() >= deadline {
            return Err("running optional notices did not drain within existing cleanup deadline; ownership retained".into());
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}
fn declarations(
    plan: &[Slot],
    common: &BTreeMap<String, CommonRequest>,
) -> Result<Vec<(String, Kind)>, String> {
    let mut output = Vec::new();
    for slot in plan.iter().filter(|slot| slot.traffic == Traffic::Write) {
        let request = common
            .get(&slot.original_id)
            .ok_or("planned write recipe missing")?;
        let ExpectedEffect::TaskStatus { task, status, .. } = &request.effect else {
            continue;
        };
        let exact = match &request.request {
            ActualRequest::Rpc {
                method: "tasks.update",
                params,
            } => {
                params["task_id"].as_str() == Some(task.as_str())
                    && params["status"].as_str() == Some(status.as_str())
            }
            ActualRequest::Mcp {
                action:
                    BridgeAction::TrackerMoveTask {
                        task_id,
                        status: actual,
                        ..
                    },
                ..
            } => task_id == task && actual == status,
            _ => false,
        };
        if !exact || request.original_id != slot.original_id {
            return Err(
                "optional notice declaration differs from exact native status request".into(),
            );
        }
        output.push((slot.original_id.clone(), Kind::TrackerWatchNotice));
    }
    // Common requests include no Done/Complete action. Rare CompleteReminder
    // eligibility stays covered by its native control, never inferred here.
    Ok(output)
}
fn checkpoint(policy: &OptionalNotices) -> Value {
    let (summary, samples) = policy.snapshot();
    json!({
        "offered":summary.offered,"created":summary.created,
        "completed":summary.completed,"active":summary.active,
        "cancelled_for_cleanup":summary.cancelled_for_cleanup,
        "completed_lifetime_us":{"count":summary.completed,"p95":us(summary.completed_p95),"max":us(summary.completed_maximum)},
        "active_age_us":{"censored":true,"count":summary.active,"p95":us(summary.active_age_p95),"max":us(summary.active_age_maximum)},
        "created_by_kind":{
            "TrackerWatchNotice":samples.iter().filter(|sample| sample.kind == Kind::TrackerWatchNotice).count(),
            "CompleteReminder":samples.iter().filter(|sample| sample.kind == Kind::CompleteReminder).count()
        },
        "samples":samples.iter().map(|sample| json!({
            "write_id":format!("{:?}",sample.delivery.id),"original_ids":sample.original_ids,
            "kind":format!("{:?}",sample.kind),"cohort":format!("{:?}",sample.delivery.cohort),
            "elapsed_us":us(sample.elapsed),"censored":sample.delivery.completed_at.is_none(),
            "completed":sample.delivery.completed_at.is_some() && !sample.delivery.cancelled && !sample.delivery.abandoned,
            "cancelled":sample.delivery.cancelled,"abandoned":sample.delivery.abandoned,
            "outstanding":sample.delivery.outstanding.iter().map(|id| format!("{id:?}")).collect::<Vec<_>>()
        })).collect::<Vec<_>>()
    })
}
fn us(value: Duration) -> u64 {
    value.as_micros().min(u64::MAX as u128) as u64
}

use std::sync::atomic::Ordering;
use std::sync::Arc;

use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use super::SessionCore;
use crate::harness::codex_app_server::observation::GoalReadFence;
use crate::harness::codex_app_server::protocol::{
    InboundNotification, OperationResult, ParentThreadFilter, ParentThreadRoute, PendingOperation,
    RpcError,
};
use crate::harness::HarnessError;

impl SessionCore {
    pub(super) fn request_goal(self: &Arc<Self>, fence: GoalReadFence, attempt: u8) {
        if self.shutting_down.load(Ordering::Acquire) {
            return;
        }
        if !self
            .surfaces
            .lock()
            .unwrap()
            .observation
            .begin_goal_read(&fence)
        {
            return;
        }
        let operation = PendingOperation::ReadGoal {
            thread_id: fence.thread_id.clone(),
            generation: fence.generation,
            revision: fence.revision,
            attempt,
        };
        if self.connection.request(operation).is_err() {
            let mut surfaces = self.surfaces.lock().unwrap();
            surfaces.observation.finish_goal_read(&fence);
            if surfaces.observation.mark_goal_stale() {
                self.surfaces_revision.bump();
            }
        }
    }

    pub(super) fn handle_goal_response(
        self: &Arc<Self>,
        fence: GoalReadFence,
        attempt: u8,
        result: Result<OperationResult, RpcError>,
    ) -> Result<(), HarnessError> {
        let current = {
            let mut surfaces = self.surfaces.lock().unwrap();
            if !surfaces.observation.finish_goal_read(&fence) {
                return Ok(());
            }
            surfaces.observation.is_current(&fence)
        };
        if !current {
            let next = self.surfaces.lock().unwrap().observation.fence();
            if let Some(next) = next {
                self.request_goal(next, 0);
            }
            return Ok(());
        }
        match result {
            Ok(OperationResult::GoalRead(result)) => {
                self.accept_goal_read(fence, attempt, result.goal)
            }
            Err(error) if error.code == -32601 => {
                if self
                    .surfaces
                    .lock()
                    .unwrap()
                    .observation
                    .mark_goal_unsupported(&fence)
                {
                    self.surfaces_revision.bump();
                }
            }
            Err(_) => self.recover_goal_read(attempt),
            Ok(_) => unreachable!("goal request produced another operation result"),
        }
        Ok(())
    }

    fn accept_goal_read(
        self: &Arc<Self>,
        fence: GoalReadFence,
        attempt: u8,
        goal: Option<crate::harness::codex_app_server::protocol::ThreadGoal>,
    ) {
        if goal
            .as_ref()
            .is_some_and(|goal| goal.thread_id != fence.thread_id)
        {
            self.recover_goal_read(attempt);
            return;
        }
        if self
            .surfaces
            .lock()
            .unwrap()
            .observation
            .apply_goal_read(&fence, goal, observed_at())
        {
            self.surfaces_revision.bump();
        }
    }

    fn recover_goal_read(self: &Arc<Self>, attempt: u8) {
        let next = {
            let mut surfaces = self.surfaces.lock().unwrap();
            if surfaces.observation.mark_goal_stale() {
                self.surfaces_revision.bump();
            }
            surfaces.observation.fence()
        };
        if let (Some(attempt), Some(next)) = (next_goal_retry(attempt), next) {
            self.request_goal(next, attempt);
        }
    }

    pub(super) fn handle_observation_notification(
        self: &Arc<Self>,
        inbound: InboundNotification,
    ) -> Result<(), HarnessError> {
        let expected = self.expected_parent_thread();
        if ParentThreadFilter::notification(&inbound.method, &inbound.params, expected.as_deref())
            != ParentThreadRoute::Parent
        {
            return Ok(());
        }
        let result = {
            let mut surfaces = self.surfaces.lock().unwrap();
            match inbound.method.as_str() {
                "thread/goal/updated" => surfaces
                    .observation
                    .apply_goal_updated(&inbound.params, observed_at()),
                "thread/goal/cleared" => surfaces
                    .observation
                    .apply_goal_cleared(&inbound.params, observed_at()),
                "turn/plan/updated" => surfaces
                    .observation
                    .apply_plan(&inbound.params, observed_at()),
                _ => unreachable!(),
            }
        };
        match result {
            Ok(true) => self.surfaces_revision.bump(),
            Ok(false) => {}
            Err(_) if inbound.method.starts_with("thread/goal/") => {
                self.recover_goal_read(u8::MAX);
                let next = self.surfaces.lock().unwrap().observation.fence();
                if let Some(next) = next {
                    self.request_goal(next, 0);
                }
            }
            Err(_) => {
                if self
                    .surfaces
                    .lock()
                    .unwrap()
                    .observation
                    .mark_checklist_stale()
                {
                    self.surfaces_revision.bump();
                }
            }
        }
        Ok(())
    }
}

pub(super) fn observed_at() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .expect("UTC receipt time formats as RFC 3339")
}

pub(super) fn next_goal_retry(attempt: u8) -> Option<u8> {
    match attempt {
        0 | 1 => Some(attempt + 1),
        _ => None,
    }
}

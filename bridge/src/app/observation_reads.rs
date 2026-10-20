//! Read-only observation preparation and execution, independent of AppState.
//! Publication supplies committed identities; each job owns all external inputs.

pub(in crate::app) mod mixed;
pub(in crate::app) mod scopes;

use super::read_view::ReadViewSlot;
use super::AppState;
use crate::api::{self, ApiError};
use crate::carrier::PreparedObservation;
use crate::store::{ReadConnection, ReadTransaction};
use crate::transport::Frame;
use serde_json::{json, Value};
use std::sync::Arc;

pub(in crate::app) struct ObservationContext {
    pub scopes: scopes::ScopeReadContext,
    pub mixed: mixed::MixedReadContext,
    deleting_project: bool,
}

impl ObservationContext {
    pub fn capture(app: &AppState) -> Self {
        Self {
            scopes: scopes::ScopeReadContext::capture(app),
            mixed: mixed::MixedReadContext::capture(app),
            deleting_project: app.project_deletion_in_progress,
        }
    }
}

#[derive(Clone)]
pub(in crate::app) struct ObservationService {
    view: ReadViewSlot,
    admission: Option<Arc<crate::update::AdmissionGate>>,
    github: crate::github::GithubCli,
    #[cfg(test)]
    gate: Option<crate::test_support::off_lock::OffLockGate>,
}

enum Job {
    Scope(scopes::ScopeReadJob),
    Mixed(mixed::MixedJob),
    Github(crate::github::GithubCli),
}

enum Fence {
    Scope(scopes::ScopeReadFence),
    Mixed(mixed::MixedFence),
    Github,
}

impl AppState {
    pub(in crate::app) fn observation_service(&self) -> ObservationService {
        ObservationService {
            view: self.read_view.clone(),
            admission: self.update_admission(),
            github: self.github.clone(),
            #[cfg(test)]
            gate: self.off_lock_gate.clone(),
        }
    }
}

impl ObservationService {
    /// SQL preparation ends here. The returned closure owns no read connection.
    pub fn prepare(&self, frame: Frame, connection: Option<ReadConnection>) -> PreparedObservation {
        let id = frame.payload.get("id").cloned().unwrap_or(Value::Null);
        let method = frame.payload["method"].as_str().unwrap_or("").to_owned();
        let params = frame
            .payload
            .get("params")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let prepared = self.prepare_job(&method, &params, connection);
        let service = self.clone();
        Box::new(move || {
            api::reply(
                id,
                prepared.and_then(|(job, fence, _lease)| {
                    let result = match job {
                        Job::Scope(job) => {
                            #[cfg(test)]
                            if let Some(gate) = &service.gate {
                                gate.arrive();
                            }
                            job.run()
                        }
                        Job::Mixed(job) => job.run(),
                        Job::Github(github) => github
                            .list_repos()
                            .map(|repos| json!({"repos":repos}))
                            .map_err(ApiError::classify),
                    }?;
                    service.validate(&method, &fence)?;
                    api::v1::check_read_result(&method, result)
                }),
            )
        })
    }

    fn prepare_job(
        &self,
        method: &str,
        params: &Value,
        mut connection: Option<ReadConnection>,
    ) -> Result<(Job, Fence, Option<crate::update::AdmissionLease>), ApiError> {
        let lease = self
            .admission
            .as_ref()
            .map(|gate| {
                gate.try_enter()
                    .ok_or_else(|| ApiError::busy("bridge update handoff in progress"))
            })
            .transpose()?;
        let params = api::v1::normalize_read_params(method, params)?;
        let context = self.context()?;
        if context.deleting_project && !super::rpc::allowed_during_project_deletion(method) {
            return Err(ApiError::unavailable(
                super::rpc::PROJECT_DELETION_IN_PROGRESS,
            ));
        }
        let (job, fence) = if method == "github.repos" {
            (Job::Github(self.github.clone()), Fence::Github)
        } else if scopes::METHODS.contains(&method) {
            let job = context.scopes.prepare(method, &params)?;
            let fence = job.fence();
            (Job::Scope(job), Fence::Scope(fence))
        } else {
            let prepare = |transaction: Option<&ReadTransaction<'_>>| {
                context.mixed.prepare(method, &params, transaction)
            };
            let job = match connection.as_mut() {
                Some(connection) => connection
                    .read(|transaction| prepare(Some(transaction)))
                    .map_err(|error| ApiError::classify(error.to_string()))??,
                None => prepare(None)?,
            };
            let fence = job.fence();
            (Job::Mixed(job), Fence::Mixed(fence))
        };
        drop(connection);
        Ok((job, fence, lease))
    }

    fn context(&self) -> Result<Arc<ObservationContext>, ApiError> {
        self.view
            .load()
            .observations
            .clone()
            .ok_or_else(|| ApiError::unavailable("observation context is not initialized"))
    }

    fn validate(&self, method: &str, fence: &Fence) -> Result<(), ApiError> {
        let context = self.context()?;
        if context.deleting_project && !super::rpc::allowed_during_project_deletion(method) {
            return Err(ApiError::unavailable(
                super::rpc::PROJECT_DELETION_IN_PROGRESS,
            ));
        }
        match fence {
            Fence::Scope(fence) => context.scopes.validate(fence),
            Fence::Mixed(fence) => context.mixed.validate(fence),
            Fence::Github => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::read_view::SnapshotDelta;

    #[test]
    fn project_deletion_keeps_allowed_observations_and_refuses_task_reads() {
        let root = tempfile::tempdir().unwrap();
        let mut app = AppState::new_unrooted(
            root.path().join("worktrees"),
            "main",
            true,
            "/tmp/observation-delete.sock",
        );
        app.project_deletion_in_progress = true;
        app.read_view.publish_delta(
            SnapshotDelta::new(app.read_view.next_version())
                .set_observations(Arc::new(ObservationContext::capture(&app))),
        );
        let service = app.observation_service();
        let frame = |method: &str, params: Value| Frame {
            session_id: "observation-test".into(),
            message_id: "observation-message".into(),
            sender: "client".into(),
            created_at: String::new(),
            frame_type: "data".into(),
            payload: json!({"id":1,"method":method,"params":params}),
        };
        let allowed = service.prepare(frame("fs.list", json!({"path":root.path()})), None)();
        assert_eq!(allowed["ok"], true, "{allowed}");
        let params = api::v1::testing::fixture("tasks.review.get")["params"].clone();
        let refused = service.prepare(frame("tasks.review.get", params), None)();
        assert_eq!(refused["error_code"], "unavailable", "{refused}");
        assert_eq!(
            refused["error"],
            super::super::rpc::PROJECT_DELETION_IN_PROGRESS
        );
    }
}

#[cfg(all(test, unix))]
mod github_tests {
    use super::*;
    use crate::carrier::SessionSender;
    use std::os::unix::fs::PermissionsExt;
    use std::time::{Duration, Instant};

    fn handler(
        script: &str,
    ) -> (
        tempfile::TempDir,
        Arc<std::sync::Mutex<AppState>>,
        crate::carrier::FrameHandler,
    ) {
        let root = tempfile::tempdir().unwrap();
        let executable = root.path().join("fixture-gh");
        std::fs::write(&executable, format!("#!/bin/sh\n{script}\n")).unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        let app = AppState::new_unrooted(
            root.path().join("worktrees"),
            "main",
            true,
            "/tmp/observation-github.sock",
        )
        .with_github_cli(crate::github::GithubCli::at(executable, "fixture-machine"));
        let state = app.shared();
        let handler = AppState::handler(state.clone());
        (root, state, handler)
    }

    fn request() -> Frame {
        Frame {
            session_id: "github-fixture".into(),
            message_id: "github-request".into(),
            sender: "client".into(),
            created_at: String::new(),
            frame_type: "data".into(),
            payload: json!({"id":7,"method":"github.repos","params":{}}),
        }
    }

    #[test]
    fn independent_github_handler_preserves_refusal_while_the_app_is_held() {
        let (_root, state, handler) = handler("exit 4");
        let _app = state.lock().unwrap();
        let answer = handler.call(SessionSender::detached("github-fixture"), request());
        assert_eq!(answer["id"], 7);
        assert_eq!(answer["error_code"], "unavailable", "{answer}");
        assert_eq!(answer["error"], "Build cannot list GitHub repositories on fixture-machine because gh is not signed in. Run `gh auth login` on fixture-machine.");
    }

    #[test]
    fn independent_github_handler_preserves_the_external_command_deadline() {
        let (_root, state, handler) = handler("exec /bin/sleep 30");
        let _app = state.lock().unwrap();
        let started = Instant::now();
        let answer = handler.call(SessionSender::detached("github-fixture"), request());
        assert!(started.elapsed() < crate::github::GH_DEADLINE + Duration::from_secs(5));
        assert_eq!(answer["error_code"], "unavailable", "{answer}");
        assert_eq!(answer["error"], "Build cannot list GitHub repositories on fixture-machine because gh did not answer within 20 seconds.");
    }
}

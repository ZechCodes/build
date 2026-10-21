//! Actual bounded router boot. Requires the separately tested router
//! recovery/no-lineage production prerequisites; never bypasses either fence.
use crate::app::tests::*;
use crate::harness::{ActivityReport, OpenedSession, SessionOutput};
use crate::test_support::agent_session::FixtureSessions;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

pub(super) struct RouterBoot {
    _factory: FixtureSessions,
    pub log: SessionLog,
    pub opens: Arc<AtomicUsize>,
    pub agent: String,
    pub capture: String,
    pub scratch: PathBuf,
}

pub(super) struct ReadyRouter {
    pub agent: String,
    pub token: String,
    pub factory_opens: usize,
    pub session: Arc<dyn AgentSession>,
}

impl RouterBoot {
    pub(super) fn prepare(app: &mut AppState, root: &Path, capture: &str, agent: &str) -> Self {
        let router = app
            .router_sessions
            .get(capture)
            .expect("native router reservation");
        assert_eq!(router.agent_id(), agent);
        let scratch = AppState::canonical_root(router.scratch_dir());
        let allowed = scratch.clone();
        let log = SessionLog::default();
        let logged = log.clone();
        let opens = Arc::new(AtomicUsize::new(0));
        let counted = opens.clone();
        let factory = FixtureSessions::install(
            std::fs::canonicalize(root).unwrap(),
            Arc::new(move |provider, request| {
                assert_eq!(
                    AppState::canonical_root(&request.root),
                    allowed,
                    "unplanned provider root"
                );
                assert_eq!(request.choice.provider, provider);
                counted.fetch_add(1, Ordering::SeqCst);
                let (activity, _) = broadcast::channel::<ActivityReport>(16);
                let output = SessionOutput::reporting(activity.subscribe(), None);
                Ok(OpenedSession {
                    session: Arc::new(
                        DictatedSession::reporting(AgentStatus::Waiting)
                            .ending_finitely()
                            .reporting_activity(activity)
                            .recording_into(&logged)
                            .natively_accepting(request.choice)
                            .interruptible(),
                    ),
                    output,
                })
            }),
        );
        assert!(factory.is_enrolled());
        let only = scratch.clone();
        app.session_locator_factory = Arc::new(move |cwd, _| {
            assert_eq!(
                AppState::canonical_root(cwd),
                only,
                "unplanned provider locator"
            );
            None
        });
        Self {
            _factory: factory,
            log,
            opens,
            agent: agent.into(),
            capture: capture.into(),
            scratch,
        }
    }

    pub(super) async fn await_ready(&self, state: &Arc<Mutex<AppState>>) -> ReadyRouter {
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            // Copy the observation under the app guard; report a failed
            // prerequisite only after dropping it. The fixture still fails
            // closed when #726 has removed the Router reservation.
            let observed = {
                let app = state.lock().unwrap();
                match app.router_sessions.get(&self.capture) {
                    None => Err("router stays reserved"),
                    Some(router) => {
                        let tab = app
                            .session_registry
                            .test_tab(&TabKey::agent(&self.scratch, &self.agent));
                        if router.started()
                            && app.delivery_queue.is_idle()
                            && self.log.turns().iter().any(|turn| {
                                turn.contains("Qualification immutable router read witness")
                            })
                        {
                            match tab.filter(|tab| tab.session_is_live()) {
                                None => Ok(None),
                                Some(tab) => app
                                    .session_registry
                                    .test_token(&self.agent)
                                    .map(|token| Some((token.to_owned(), tab.session.clone())))
                                    .ok_or("actual spawn issued token"),
                            }
                        } else {
                            Ok(None)
                        }
                    }
                }
            };
            let ready = observed.unwrap_or_else(|diagnostic| panic!("{diagnostic}"));
            if let Some((token, session)) = ready {
                assert_eq!(
                    self.opens.load(Ordering::SeqCst),
                    1,
                    "exactly one planned router boot"
                );
                return ReadyRouter {
                    agent: self.agent.clone(),
                    token,
                    factory_opens: 1,
                    session,
                };
            }
            assert!(
                std::time::Instant::now() < deadline,
                "blocked #726: actual finite router boot did not settle"
            );
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    }
}

// Shared finite end→exit behavior remains test-first in the qualification
// provider fixture. Do not manufacture it locally or bypass Router #726.
impl RouterBoot {
    pub(super) async fn shutdown(&self, state: &Arc<Mutex<AppState>>, ready: &ReadyRouter) {
        state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .abandon_router_session(&self.capture);
        let child = ready.session.clone();
        tokio::task::spawn_blocking(move || {
            child.end();
            assert!(
                child.exited_within(Duration::from_secs(2)),
                "finite router did not close"
            );
        })
        .await
        .expect("finite router close worker joined");
        let parent = self.scratch.parent().expect("router scratch parent");
        let prefix = format!(
            "{}{}",
            self.scratch.file_name().unwrap().to_string_lossy(),
            crate::reaper::RETIRING_DIR_MARK
        );
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            let idle = state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .delivery_queue
                .is_idle();
            let retiring = std::fs::read_dir(parent).is_ok_and(|entries| {
                entries
                    .filter_map(Result::ok)
                    .any(|entry| entry.file_name().to_string_lossy().starts_with(&prefix))
            });
            if idle && !self.scratch.exists() && !retiring {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "router retirement/directory removal did not join"
            );
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        let app = state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert!(!app.router_sessions.contains_key(&self.capture));
        assert!(app.session_registry.test_token(&self.agent).is_none());
    }
}

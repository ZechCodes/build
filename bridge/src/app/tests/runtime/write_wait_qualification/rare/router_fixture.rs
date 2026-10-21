//! Actual capture admission and finite router publication shared by MCP controls.
use super::{finite_provider::FiniteProvider, RareFixture, VerifiedStep};
use crate::app::tests::*;
use std::path::PathBuf;
use std::time::Duration;

pub(super) struct RouterIdentity {
    pub(super) capture_id: String,
    pub(super) agent_id: String,
    pub(super) scratch: PathBuf,
    pub(super) token: String,
}

struct EndSessions(Arc<Mutex<AppState>>);

impl Drop for EndSessions {
    fn drop(&mut self) {
        let sessions: Vec<_> = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .session_registry
            .test_tabs()
            .map(|(_, tab)| Arc::clone(&tab.session))
            .collect();
        for session in sessions {
            session.end();
        }
    }
}

pub(super) struct RouterFixture {
    pub(super) fixture: RareFixture,
    pub(super) provider: FiniteProvider,
    pub(super) project: String,
    pub(super) repository: PathBuf,
    cleanup: EndSessions,
    directory: tempfile::TempDir,
}

impl RouterFixture {
    pub(super) fn new(family: &'static str) -> Self {
        let (directory, repository) = init_repo();
        let provider = FiniteProvider::install(directory.path());
        provider.require_enrolled();
        let mut app = qa_state(&repository, directory.path());
        app.default_harness = AgentProvider::Claude;
        app.router_choice = Some(ModelChoice {
            provider: AgentProvider::Claude,
            model: None,
            effort: Some("low".into()),
        });
        let project = app.project_at(0).id.clone();
        let fixture = RareFixture::new(family, app);
        let cleanup = EndSessions(Arc::clone(fixture.state()));
        Self {
            fixture,
            provider,
            project,
            repository,
            cleanup,
            directory,
        }
    }

    pub(super) async fn capture(&mut self, text: &str) -> RouterIdentity {
        let proof = self
            .fixture
            .call("capture.create", json!({"text":text}))
            .await;
        let capture_id = proof.reply["result"]["id"].as_str().unwrap().to_owned();
        let published = wait_for(Duration::from_secs(2), || {
            let app = self.fixture.state().lock().unwrap();
            let router = app.router_sessions.get(&capture_id)?;
            let scratch = AppState::canonical_root(router.scratch_dir());
            let tab = app
                .session_registry
                .test_tab(&TabKey::agent(&scratch, router.agent_id()))?;
            if !router.started()
                || !tab.session_is_live()
                || !app.delivery_queue.is_idle()
                || !self
                    .provider
                    .log
                    .turns()
                    .iter()
                    .any(|turn| turn.contains(text))
            {
                return None;
            }
            Some(RouterIdentity {
                capture_id: capture_id.clone(),
                agent_id: router.agent_id().to_owned(),
                scratch,
                token: app
                    .session_registry
                    .test_token(router.agent_id())?
                    .to_owned(),
            })
        })
        .await;
        let diagnostic = {
            let app = self.fixture.state().lock().unwrap();
            json!({"capture_id":capture_id,"capture":app.captures.get(&capture_id),
                "router":app.router_sessions.get(&capture_id).map(|router| json!({"agent_id":router.agent_id(),"scratch":router.scratch_dir(),"started":router.started()})),"delivery_idle":app.delivery_queue.is_idle(),
                "claims":app.session_registry.test_counts().claims,"factory_calls":self.provider.opens.load(std::sync::atomic::Ordering::SeqCst),
                "turns_delivered":self.provider.log.turns().len()})
        };
        if published.is_none() {
            self.quiesce().await;
            panic!("actual capture router did not publish and consume its turn: {diagnostic}");
        }
        let identity = published.unwrap();
        let saved = self.capture_record(&capture_id);
        assert_eq!(saved["text"], text);
        assert_eq!(saved["state"], "routing");
        self.fixture
            .verify(
                proof,
                json!({"capture_id":capture_id,"persisted_text":text,
            "actual_router_agent":identity.agent_id,"router_turn_delivered":true}),
            )
            .await;
        identity
    }

    pub(super) fn capture_record(&self, id: &str) -> Value {
        let app = self.fixture.state().lock().unwrap();
        let saved = app.store.as_ref().unwrap().load_all_captures().unwrap();
        serde_json::to_value(saved.iter().find(|capture| capture.id == id).unwrap()).unwrap()
    }

    pub(super) async fn quiesce(&self) {
        {
            let mut app = self
                .fixture
                .state()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let ids: Vec<_> = app.router_sessions.keys().cloned().collect();
            for id in ids {
                app.abandon_router_session(&id);
            }
        }
        let sessions: Vec<_> = self
            .fixture
            .state()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .session_registry
            .test_tabs()
            .map(|(_, tab)| Arc::clone(&tab.session))
            .collect();
        for session in sessions {
            session.end();
        }
        let scratch = self
            .directory
            .path()
            .join(crate::router::ROUTER_SCRATCH_DIR_NAME);
        let joined = wait_for(Duration::from_secs(5), || {
            let idle = self
                .fixture
                .state()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .delivery_queue
                .is_idle();
            let reaped = match std::fs::read_dir(&scratch) {
                Ok(mut entries) => entries.next().is_none(),
                Err(error) => error.kind() == std::io::ErrorKind::NotFound,
            };
            (idle && reaped).then_some(())
        })
        .await;
        assert!(
            joined.is_some(),
            "owned router delivery/retirement did not finish"
        );
    }

    pub(super) async fn finish(self) -> Vec<VerifiedStep> {
        self.quiesce().await;
        let Self {
            fixture,
            provider,
            cleanup,
            directory,
            ..
        } = self;
        let steps = fixture.finish().await;
        drop(cleanup);
        drop(provider);
        drop(directory);
        // The real capture setup was separately verified; the MCP collector
        // consumes only authenticated semantic actions.
        steps
            .into_iter()
            .filter(|step| step.mcp_action.is_some())
            .collect()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn actual_capture_router_publishes_and_consumes_before_mcp_actions() {
    let mut context = RouterFixture::new("router-delivery-prerequisite");
    let identity = context.capture("router delivery prerequisite").await;
    assert!(identity.scratch.is_dir());
    assert!(
        context
            .provider
            .opens
            .load(std::sync::atomic::Ordering::SeqCst)
            > 0
    );
    assert!(context.finish().await.is_empty());
}

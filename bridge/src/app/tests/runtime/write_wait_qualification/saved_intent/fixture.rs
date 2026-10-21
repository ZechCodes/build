use crate::app::tests::project_agent::project_agent;
use crate::app::tests::tracker::{filed, tracked};
use crate::app::tests::{dictated_agent_tab, req, DictatedSession, SessionLog};
use crate::app::{AppState, DeliveryRunner, TabKey, TabRole};
use crate::harness::{AgentSession, AgentStatus, HarnessError, SessionStatusSnapshot};
use crate::store::DeliveryIntent;
use crate::test_support::agent_session::FixtureSessions;
use crate::tracker::Actor;
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::watch;

pub(super) struct Fixture {
    _root: tempfile::TempDir,
    _repo: tempfile::TempDir,
    pub state: Arc<Mutex<AppState>>,
    pub intent: DeliveryIntent,
    pub owner: String,
    pub agent: String,
    pub warm: WarmSession,
}

impl Fixture {
    pub fn new() -> Self {
        // No Tokio context: shared() must not start the autonomous assignment
        // service before the measured test explicitly invokes its real step.
        std::thread::spawn(Self::build).join().unwrap()
    }

    pub fn database_path(&self) -> std::path::PathBuf {
        self._root.path().join("store/build.db")
    }

    pub fn spare_blocker(&self) -> String {
        let mut app = self.state.lock().unwrap();
        let project = app
            .projects
            .iter()
            .find(|project| project.repo_path.to_str() == Some(self.intent.project_path.as_str()))
            .unwrap()
            .id
            .clone();
        filed(&mut app, &project, "lifecycle spare blocker")["id"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    /// Real transports must be closed before releasing the fixture's last
    /// AppState owner. Remaining workers are observed, never forcibly stopped.
    async fn retire(self, transport_clean: bool) -> Result<RetiredFixture, String> {
        let Self {
            _root,
            _repo,
            state,
            intent,
            owner,
            agent,
            warm,
        } = self;
        let retired = RetiredFixture {
            root: _root,
            repo: _repo,
            intent,
            owner,
            agent,
        };
        warm.end();
        let weak = Arc::downgrade(&state);
        drop(warm);
        drop(state);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while weak.strong_count() != 0 && std::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        let owners = weak.strong_count();
        if !transport_clean || owners != 0 {
            return Err(retired.preserve(format!(
                "fixture cleanup unconfirmed: transport_clean={transport_clean}, AppState owners={owners}"
            )));
        }
        Ok(retired)
    }

    pub async fn finish_owned(self, transport_clean: bool) -> Result<(), String> {
        drop(self.retire(transport_clean).await?);
        Ok(())
    }

    /// Reopen only after the old native owners have disappeared. A failed
    /// teardown preserves both roots; it cannot pretend that restart happened.
    pub async fn reopen(self, transport_clean: bool) -> Result<Self, String> {
        let retired = self.retire(transport_clean).await?;
        std::thread::spawn(move || {
            let opened = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let mut app = crate::app::tests::project_agent::rooted(retired.root.path())
                    .with_config(retired.root.path().join("config.json"))
                    .map_err(|error| error.to_string())?
                    .with_task_store(retired.root.path().join("store"))?;
                let warm = WarmSession::install(&mut app, &retired.owner, &retired.agent);
                Ok::<_, String>((app.shared(), warm))
            }));
            match opened {
                Ok(Ok((state, warm))) => Ok(Self {
                    _root: retired.root,
                    _repo: retired.repo,
                    intent: retired.intent,
                    owner: retired.owner,
                    agent: retired.agent,
                    state,
                    warm,
                }),
                Ok(Err(error)) => Err(retired.preserve(format!("fixture reopen failed: {error}"))),
                Err(_) => Err(retired.preserve("fixture reopen panicked".into())),
            }
        })
        .join()
        .map_err(|_| "joined fixture reopen worker panicked".to_owned())?
    }

    fn build() -> Self {
        Self::build_configured(false)
    }

    pub fn restartable() -> Self {
        std::thread::spawn(|| Self::build_configured(true))
            .join()
            .unwrap()
    }

    fn build_configured(restartable: bool) -> Self {
        let root = tempfile::tempdir().unwrap();
        let (repo, mut app, project) = tracked(root.path());
        if restartable {
            app = app.with_config(root.path().join("config.json")).unwrap();
        }
        let (owner, agent) = project_agent(&mut app, &project);
        let intent = failed_ready_assignment(&mut app, &project, &agent);
        let warm = WarmSession::install(&mut app, &owner, &agent);

        // Finish setup's real failure/dependency notices before introducing a
        // registry. They are distinct operations, not the retry under test.
        let mut steps = 0;
        while app.pump_task_deliveries_for_test().unwrap() {
            steps += 1;
            assert!(steps < 32, "bounded setup notices must drain");
        }
        app.delivery_queue.lapse_settle_windows();
        if restartable {
            app.persist();
        }
        let state = app.shared();
        let clock = Arc::clone(&state.lock().unwrap().frame_clock);
        DeliveryRunner::drain(&state, &clock.frame("qualification.saved_retry_setup"));
        assert!(state.lock().unwrap().delivery_queue.is_idle());
        assert_eq!(warm.opens.load(Ordering::SeqCst), 0);
        assert!(state
            .lock()
            .unwrap()
            .tracker_store()
            .unwrap()
            .operation(&intent.operation_id)
            .unwrap()
            .is_none());
        Self {
            _root: root,
            _repo: repo,
            state,
            intent,
            owner,
            agent,
            warm,
        }
    }
}

struct RetiredFixture {
    root: tempfile::TempDir,
    repo: tempfile::TempDir,
    intent: DeliveryIntent,
    owner: String,
    agent: String,
}

impl RetiredFixture {
    fn preserve(self, error: String) -> String {
        let root = self.root.keep();
        let repo = self.repo.keep();
        format!(
            "{error}; private roots preserved: {}, {}; pid={}",
            root.display(),
            repo.display(),
            std::process::id()
        )
    }
}

fn failed_ready_assignment(app: &mut AppState, project: &str, agent: &str) -> DeliveryIntent {
    let task = filed(app, project, "saved retry original")["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let blocker = filed(app, project, "saved retry prerequisite")["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let path = app.tracker_project_path(project).unwrap();
    app.tracker_store()
        .unwrap()
        .edit_waiting_on(
            &path,
            &task,
            std::slice::from_ref(&blocker),
            &[],
            &Actor::User,
            &crate::store::now_rfc3339(),
        )
        .unwrap();
    reconcile(app);
    let accepted = app.handle(req(
        "tasks.assign",
        json!({
            "task_id":task, "assignee":{"kind":"agent","agent_id":agent},
            "note":"saved retry required delivery",
        }),
    ));
    assert_eq!(accepted["ok"], true, "{accepted}");
    assert_eq!(
        accepted["result"]["task"]["deferred_assignment"]["state"],
        "waiting"
    );
    let moved = app.handle(req(
        "tasks.update",
        json!({"task_id":blocker,"status":"done"}),
    ));
    assert_eq!(moved["ok"], true, "{moved}");
    reconcile(app);
    let intent = app
        .tracker_store()
        .unwrap()
        .current_delivery_intent(&task)
        .unwrap()
        .unwrap();
    app.tracker_store()
        .unwrap()
        .fail_delivery_intent(
            &intent.id,
            intent.generation,
            "test fixture prior attempt failed",
        )
        .unwrap();
    app.tracker_store()
        .unwrap()
        .load_delivery_intent(&intent.id)
        .unwrap()
        .unwrap()
}

fn reconcile(app: &AppState) {
    for _ in 0..32 {
        if !app
            .tracker_store()
            .unwrap()
            .reconcile_waiting_on(100)
            .unwrap()
            .pending
        {
            return;
        }
    }
    panic!("bounded fixture dependency reconciliation must finish");
}

pub(super) struct WarmSession {
    session: Arc<dyn AgentSession>,
    _factory: FixtureSessions,
    _status: watch::Sender<SessionStatusSnapshot>,
    pub log: SessionLog,
    pub opens: Arc<AtomicUsize>,
}

impl WarmSession {
    fn install(app: &mut AppState, owner: &str, agent: &str) -> Self {
        let checkout = app.runs[owner].worktree.path.clone();
        let choice = app.runs[owner].agents.by_id(agent).unwrap().choice.clone();
        let opens = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&opens);
        let factory = FixtureSessions::install(
            checkout.clone(),
            Arc::new(move |_, _| {
                count.fetch_add(1, Ordering::SeqCst);
                Err(HarnessError::Setup(
                    "saved retry must reuse its warm fixture session".into(),
                ))
            }),
        );
        assert!(factory.is_enrolled());
        let log = SessionLog::default();
        let (status, receiver) = watch::channel(SessionStatusSnapshot::new(AgentStatus::Waiting));
        let session = DictatedSession::reporting(AgentStatus::Waiting)
            .watching_status(receiver)
            .recording_into(&log)
            .natively_accepting(choice.clone())
            .interruptible();
        let mut tab = dictated_agent_tab(&checkout, owner, agent, session);
        tab.role = TabRole::Agent {
            owner: owner.into(),
            agent_id: agent.into(),
            provider: choice.provider,
        };
        tab.session_instance =
            app.record_agent_session_start(owner, agent, &checkout, &choice, "saved-retry-warm");
        assert!(app.session_instance_uses_choice(tab.session_instance.as_ref().unwrap(), &choice));
        let session = Arc::clone(&tab.session);
        app.session_registry.test_insert_tab(
            TabKey::agent(&AppState::canonical_root(&checkout), agent),
            tab,
        );
        Self {
            session,
            _factory: factory,
            _status: status,
            log,
            opens,
        }
    }

    pub fn end(&self) {
        self.session.end();
    }
}

impl Drop for WarmSession {
    fn drop(&mut self) {
        self.end();
    }
}

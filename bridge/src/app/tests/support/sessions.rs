use super::super::*;

/// What a dictated session was asked to do, still readable once the daemon
/// owns the session itself — an `Arc<dyn AgentSession>` in a tab cannot be
/// looked inside, so the record lives beside it.
#[derive(Clone, Default)]
pub(in crate::app::tests) struct SessionLog {
    turns: Arc<Mutex<Vec<String>>>,
    choices: Arc<Mutex<Vec<Option<ModelChoice>>>>,
    ended: Arc<std::sync::atomic::AtomicBool>,
}

impl SessionLog {
    /// Every turn the daemon has handed this session, in order.
    pub(in crate::app::tests) fn turns(&self) -> Vec<String> {
        self.turns.lock().unwrap().clone()
    }

    pub(in crate::app::tests) fn choices(&self) -> Vec<Option<ModelChoice>> {
        self.choices.lock().unwrap().clone()
    }

    /// Whether the daemon has ended this session, waiting out the
    /// retirement thread that carries the kill.
    pub(in crate::app::tests) fn ended(&self) -> bool {
        settles(|| self.ended.load(std::sync::atomic::Ordering::Relaxed))
    }
}

/// A session that reports what it is told to, and remembers what was asked
/// of it.
///
/// It has no terminal and no process, so nothing about it can be inferred
/// from paint or from a wait — a caller reading a PTY would find no answer
/// here at all. Only a caller that asks the session gets one.
pub(in crate::app::tests) struct DictatedSession {
    status: AgentStatus,
    quiet: Duration,
    log: SessionLog,
    surfaces: Option<AgentSurfaces>,
    watched_surface_revision: Option<tokio::sync::watch::Receiver<u64>>,
    watched_status: Option<tokio::sync::watch::Receiver<crate::harness::SessionStatusSnapshot>>,
    active_model: Option<String>,
    native_choices: Vec<ModelChoice>,
}

impl DictatedSession {
    /// A session reporting `status`, silent for no time at all.
    pub(in crate::app::tests) fn reporting(status: AgentStatus) -> DictatedSession {
        DictatedSession {
            status,
            quiet: Duration::ZERO,
            log: SessionLog::default(),
            surfaces: None,
            watched_surface_revision: None,
            watched_status: None,
            active_model: None,
            native_choices: Vec::new(),
        }
    }

    pub(in crate::app::tests) fn showing_surfaces(
        mut self,
        surfaces: AgentSurfaces,
    ) -> DictatedSession {
        self.surfaces = Some(surfaces);
        self
    }

    pub(in crate::app::tests) fn moving_surfaces_on(
        self,
        revision: SurfaceRevision,
    ) -> DictatedSession {
        self.watching_a_revision_the_caller_can_close(revision.subscribe())
    }

    pub(in crate::app::tests) fn watching_a_revision_the_caller_can_close(
        mut self,
        watched: tokio::sync::watch::Receiver<u64>,
    ) -> DictatedSession {
        self.watched_surface_revision = Some(watched);
        self
    }

    pub(in crate::app::tests) fn watching_status(
        mut self,
        watched: tokio::sync::watch::Receiver<crate::harness::SessionStatusSnapshot>,
    ) -> Self {
        self.watched_status = Some(watched);
        self
    }

    /// The same session, with nothing heard from it for `quiet` — the
    /// anomaly clock the idle sweep demotes on.
    pub(in crate::app::tests) fn silent_for(mut self, quiet: Duration) -> DictatedSession {
        self.quiet = quiet;
        self
    }

    /// The same session, recording what it is asked into `log`.
    pub(in crate::app::tests) fn recording_into(mut self, log: &SessionLog) -> DictatedSession {
        self.log = log.clone();
        self
    }

    pub(in crate::app::tests) fn announcing_model(mut self, model: &str) -> DictatedSession {
        self.active_model = Some(model.to_string());
        self
    }

    pub(in crate::app::tests) fn natively_accepting(mut self, choice: ModelChoice) -> Self {
        self.native_choices.push(choice);
        self
    }
}

impl AgentSession for DictatedSession {
    fn send_turn(&self, turn: &Turn) -> Result<(), HarnessError> {
        self.log.turns.lock().unwrap().push(turn.text.clone());
        self.log.choices.lock().unwrap().push(
            turn.choice
                .as_ref()
                .map(|choice| choice.model_choice.clone()),
        );
        Ok(())
    }
    fn accepts_turn_choice(&self) -> bool {
        !self.native_choices.is_empty()
    }
    fn turn_choice_support(&self, choice: &ModelChoice) -> TurnChoiceSupport {
        if self.native_choices.contains(choice) {
            TurnChoiceSupport::Native
        } else {
            TurnChoiceSupport::RestartRequired
        }
    }
    fn status(&self) -> AgentStatus {
        self.watched_status
            .as_ref()
            .map_or(self.status, |status| status.borrow().status)
    }
    fn status_changed(
        &self,
    ) -> Option<tokio::sync::watch::Receiver<crate::harness::SessionStatusSnapshot>> {
        self.watched_status.clone()
    }
    fn quiet_for(&self) -> Duration {
        self.quiet
    }
    fn active_model(&self) -> Option<String> {
        self.active_model.clone()
    }
    fn exited_within(&self, _timeout: Duration) -> bool {
        matches!(self.status, AgentStatus::Ended { .. })
    }
    fn end(&self) {
        self.log
            .ended
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
    fn backdate_last_output(&self, _ago: Duration) {}
    fn surfaces(&self) -> Option<AgentSurfaces> {
        self.surfaces.clone()
    }
    fn surfaces_changed(&self) -> Option<tokio::sync::watch::Receiver<u64>> {
        self.watched_surface_revision.clone()
    }
}

/// A live tab whose session reports `status`.
pub(in crate::app::tests) fn tab_reporting(role: TabRole, status: AgentStatus) -> Tab {
    tab_running(role, DictatedSession::reporting(status))
}

/// A live tab carrying `session`, rooted nowhere in particular.
pub(in crate::app::tests) fn tab_running(role: TabRole, session: DictatedSession) -> Tab {
    let session_instance = role.agent().map(|(owner, agent_id)| SessionInstance {
        id: format!("session-test-{agent_id}"),
        entity_id: owner.to_string(),
        agent_id: agent_id.to_string(),
        conversation_id: agent_id.to_string(),
        checkout: "/nowhere".to_string(),
    });
    Tab {
        tab_id: "term-status".to_string(),
        root: PathBuf::from("/nowhere"),
        role,
        created_at: now_rfc3339(),
        session: Arc::new(session),
        session_instance,
        // `DictatedSession` says nothing about terminals, so it has none —
        // and a tab with no terminal has no grid, because the two are made
        // together. A screen here would be the flag that disagrees with
        // reality.
        screen: None,
        live: true,
        call_sequences: HashMap::new(),
        last_delivered_at: None,
    }
}

/// An agent tab rooted at `root` whose session has no terminal — the shape
/// a session protocol has.
///
/// Built by hand so a test can put a session in a state it chooses. The
/// headless provider produces the real thing —
/// `the_terminal_verbs_refuse_the_headless_agent_the_daemon_spawned` walks
/// the refusals against a child the daemon spawned — and this stays for the
/// tests that need a session reporting a dictated status rather than
/// whatever a real one happens to be doing.
pub(in crate::app::tests) fn terminal_free_agent_tab(
    root: &std::path::Path,
    owner: &str,
    agent_id: &str,
) -> Tab {
    dictated_agent_tab(
        root,
        owner,
        agent_id,
        DictatedSession::reporting(AgentStatus::Working),
    )
}

pub(in crate::app::tests) fn a_run_with_a_reporting_tab(
    run_id: &str,
) -> (tempfile::TempDir, Arc<Mutex<AppState>>, TabKey) {
    a_run_with_a_dictated_tab(run_id, DictatedSession::reporting(AgentStatus::Working))
}

pub(in crate::app::tests) fn a_run_with_a_dictated_tab(
    run_id: &str,
    session: DictatedSession,
) -> (tempfile::TempDir, Arc<Mutex<AppState>>, TabKey) {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let root = insert_run(&mut app, &repo, dir.path(), run_id, RunState::Building);
    let key = insert_dictated_agent_tab(&mut app, &root, run_id, session);
    (dir, app.shared(), key)
}

/// A live agent tab at `root` carrying a session that reports exactly what
/// it was told — the only way a test can put a turn-reporting session where
/// the daemon expects one, since a PTY can only be asked about paint.
pub(in crate::app::tests) fn dictated_agent_tab(
    root: &std::path::Path,
    owner: &str,
    agent_id: &str,
    session: DictatedSession,
) -> Tab {
    Tab {
        tab_id: agent_tab_id(agent_id),
        root: root.to_path_buf(),
        role: TabRole::Agent {
            owner: owner.to_string(),
            agent_id: agent_id.to_string(),
            provider: AgentProvider::default(),
        },
        created_at: now_rfc3339(),
        session: Arc::new(session),
        session_instance: Some(SessionInstance {
            id: format!("session-test-{agent_id}"),
            entity_id: owner.to_string(),
            agent_id: agent_id.to_string(),
            conversation_id: agent_id.to_string(),
            checkout: root.display().to_string(),
        }),
        screen: None,
        live: true,
        call_sequences: HashMap::new(),
        last_delivered_at: None,
    }
}

/// Poll `look` until it answers, or give up after `budget`.
///
/// A pump runs on its own task, so a test speaks about what it did by
/// waiting for the thing itself — never by sleeping a guess and asserting
/// on whatever had happened by then.
pub(in crate::app::tests) async fn wait_for<T>(
    budget: Duration,
    mut look: impl FnMut() -> Option<T>,
) -> Option<T> {
    let deadline = std::time::Instant::now() + budget;
    loop {
        if let Some(found) = look() {
            return Some(found);
        }
        if std::time::Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Wait for every turn the verbs just run queued to have been delivered.
///
/// The queue is taken and its owners marked in flight under the acquisition
/// the verb's own reply comes out of, so a caller that has its reply and
/// finds neither is looking at a delivery that has finished.
pub(in crate::app::tests) async fn wait_for_deliveries(state: &Arc<Mutex<AppState>>) {
    wait_for(Duration::from_secs(20), || {
        let s = state.lock().unwrap();
        (s.delivery_queue.queued_is_empty() && s.delivery_queue.is_idle()).then_some(())
    })
    .await
    .expect("every queued turn reached its agent");
}

/// Wait for the tab a queued turn's delivery opens.
///
/// A verb answers as soon as its own state change is durable and the turn
/// it queued is delivered on a thread of its own, so the reply is never the
/// moment the agent tab arrives.
pub(in crate::app::tests) async fn wait_for_agent_tab(state: &Arc<Mutex<AppState>>, key: &TabKey) {
    wait_for(Duration::from_secs(10), || {
        state
            .lock()
            .unwrap()
            .session_registry
            .contains(key)
            .then_some(())
    })
    .await
    .unwrap_or_else(|| panic!("the delivery never opened {key:?}"));
}

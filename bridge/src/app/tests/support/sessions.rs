use super::super::*;

mod finite_turns;

/// What a dictated session was asked to do, still readable once the daemon
/// owns the session itself — an `Arc<dyn AgentSession>` in a tab cannot be
/// looked inside, so the record lives beside it.
#[derive(Clone, Default)]
pub(in crate::app::tests) struct SessionLog {
    turns: Arc<Mutex<Vec<String>>>,
    choices: Arc<Mutex<Vec<Option<ModelChoice>>>>,
    ended: Arc<std::sync::atomic::AtomicBool>,
    interrupted: Arc<std::sync::atomic::AtomicBool>,
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

    pub(in crate::app::tests) fn interrupted(&self) -> bool {
        self.interrupted.load(std::sync::atomic::Ordering::Relaxed)
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
    finite_exit: Option<std::sync::atomic::AtomicBool>,
    finite_turns: Option<finite_turns::FiniteTurns>,
    activity: Mutex<Option<tokio::sync::broadcast::Sender<crate::harness::ActivityReport>>>,
    quiet: Duration,
    log: SessionLog,
    surfaces: Option<AgentSurfaces>,
    watched_surface_revision: Option<tokio::sync::watch::Receiver<u64>>,
    watched_status: Option<tokio::sync::watch::Receiver<crate::harness::SessionStatusSnapshot>>,
    watched_receipts: Option<tokio::sync::watch::Receiver<crate::harness::TurnReceiptSnapshot>>,
    activity: Option<tokio::sync::broadcast::Sender<crate::harness::ActivityReport>>,
    active_model: Option<String>,
    native_choices: Vec<ModelChoice>,
    interruptible: bool,
    start_refused: Option<String>,
    recovery_episode: Option<String>,
}

impl DictatedSession {
    /// A session reporting `status`, silent for no time at all.
    pub(in crate::app::tests) fn reporting(status: AgentStatus) -> DictatedSession {
        DictatedSession {
            status,
            finite_exit: None,
            finite_turns: None,
            activity: Mutex::new(None),
            quiet: Duration::ZERO,
            log: SessionLog::default(),
            surfaces: None,
            watched_surface_revision: None,
            watched_status: None,
            watched_receipts: None,
            activity: None,
            active_model: None,
            native_choices: Vec::new(),
            interruptible: false,
            start_refused: None,
            recovery_episode: None,
        }
    }

    /// Give this process-free session its own completed-retirement state.
    pub(in crate::app::tests) fn ending_finitely(mut self) -> Self {
        self.finite_exit = Some(std::sync::atomic::AtomicBool::new(false));
        self
    }

    /// Complete each actual send through an owned bounded status producer.
    pub(in crate::app::tests) fn finishing_turns_after(mut self, delay: Duration) -> Self {
        assert!(
            delay > Duration::ZERO && delay <= Duration::from_millis(500),
            "finite turns must complete within 500ms with a positive work interval"
        );
        assert!(
            self.watched_status.is_none() && self.finite_turns.is_none(),
            "finite turns own their status producer"
        );
        assert_eq!(
            self.status,
            AgentStatus::Waiting,
            "finite turns start Waiting"
        );
        let (turns, watched) = finite_turns::FiniteTurns::start(delay);
        self.finite_turns = Some(turns);
        self.watched_status = Some(watched);
        self
    }

    /// Own the final activity sender so end closes the normal activity pump.
    pub(in crate::app::tests) fn reporting_activity(
        self,
        activity: tokio::sync::broadcast::Sender<crate::harness::ActivityReport>,
    ) -> Self {
        *self.activity.lock().unwrap() = Some(activity);
        self
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
        assert!(
            self.finite_turns.is_none(),
            "finite turns own their status producer"
        );
        self.watched_status = Some(watched);
        self
    }

    pub(in crate::app::tests) fn watching_receipts(
        mut self,
        watched: tokio::sync::watch::Receiver<crate::harness::TurnReceiptSnapshot>,
    ) -> Self {
        self.watched_receipts = Some(watched);
        self
    }

    pub(in crate::app::tests) fn reporting_activity(
        mut self,
        activity: tokio::sync::broadcast::Sender<crate::harness::ActivityReport>,
    ) -> Self {
        self.activity = Some(activity);
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

    pub(in crate::app::tests) fn interruptible(mut self) -> Self {
        self.interruptible = true;
        self
    }

    /// The same session, ended by Build before its first turn for `reason`.
    pub(in crate::app::tests) fn refusing_start(mut self, reason: &str) -> Self {
        self.start_refused = Some(reason.to_string());
        self
    }

    pub(in crate::app::tests) fn owning_recovery(mut self, episode: &str) -> Self {
        self.recovery_episode = Some(episode.into());
        self
    }
}

impl AgentSession for DictatedSession {
    fn activity(&self) -> Option<tokio::sync::broadcast::Receiver<crate::harness::ActivityReport>> {
        self.activity.as_ref().map(|activity| activity.subscribe())
    }
    fn send_turn(&self, turn: &Turn) -> Result<(), HarnessError> {
        self.log.turns.lock().unwrap().push(turn.text.clone());
        self.log.choices.lock().unwrap().push(
            turn.choice
                .as_ref()
                .map(|choice| choice.model_choice.clone()),
        );
        if let Some(turns) = &self.finite_turns {
            turns.sent()?;
        }
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
    fn provider_recovery_episode(&self) -> Option<String> {
        self.recovery_episode.clone()
    }
    fn can_interrupt(&self) -> bool {
        self.interruptible && matches!(self.status(), AgentStatus::Working)
    }
    fn interrupt(&self) -> Result<(), HarnessError> {
        if !self.can_interrupt() {
            return Err(HarnessError::Unsupported(
                "dictated session cannot interrupt".into(),
            ));
        }
        self.log
            .interrupted
            .store(true, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }
    fn activity(&self) -> Option<tokio::sync::broadcast::Receiver<crate::harness::ActivityReport>> {
        self.activity
            .lock()
            .unwrap()
            .as_ref()
            .map(|activity| activity.subscribe())
    }
    fn status_changed(
        &self,
    ) -> Option<tokio::sync::watch::Receiver<crate::harness::SessionStatusSnapshot>> {
        self.watched_status.clone()
    }
    fn turn_receipts(
        &self,
    ) -> Option<tokio::sync::watch::Receiver<crate::harness::TurnReceiptSnapshot>> {
        self.watched_receipts.clone()
    }
    fn quiet_for(&self) -> Duration {
        self.quiet
    }
    fn active_model(&self) -> Option<String> {
        self.active_model.clone()
    }
    fn exited_within(&self, _timeout: Duration) -> bool {
        matches!(self.status, AgentStatus::Ended { .. })
            || self
                .finite_exit
                .as_ref()
                .is_some_and(|ended| ended.load(std::sync::atomic::Ordering::Acquire))
    }
    fn end(&self) {
        if let Some(turns) = &self.finite_turns {
            turns.end();
        }
        self.activity.lock().unwrap().take();
        self.log
            .ended
            .store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(ended) = &self.finite_exit {
            ended.store(true, std::sync::atomic::Ordering::Release);
        }
    }
    fn backdate_last_output(&self, _ago: Duration) {}
    fn start_refused(&self) -> Option<String> {
        self.start_refused.clone()
    }
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
        project_source: None,
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
    session: impl AgentSession + 'static,
) -> Tab {
    Tab {
        tab_id: agent_tab_id(agent_id),
        root: root.to_path_buf(),
        role: TabRole::Agent {
            owner: owner.to_string(),
            agent_id: agent_id.to_string(),
            provider: AgentProvider::default(),
        },
        project_source: None,
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

#[test]
fn finite_waiting_session_reports_exit_only_after_its_own_retirement() {
    let shared_log = SessionLog::default();
    let (activity, mut output) =
        tokio::sync::broadcast::channel::<crate::harness::ActivityReport>(8);
    let retiring = Arc::new(
        DictatedSession::reporting(AgentStatus::Waiting)
            .ending_finitely()
            .recording_into(&shared_log)
            .reporting_activity(activity),
    );
    let (peer_activity, _) = tokio::sync::broadcast::channel::<crate::harness::ActivityReport>(8);
    let peer = DictatedSession::reporting(AgentStatus::Waiting)
        .ending_finitely()
        .recording_into(&shared_log)
        .reporting_activity(peer_activity);
    assert!(!retiring.exited_within(Duration::ZERO));
    assert!(!peer.exited_within(Duration::ZERO));
    let session: Arc<dyn AgentSession> = retiring.clone();
    let retirement = crate::reaper::Retirement::begin(session);
    let reaped = retirement.wait(Duration::from_secs(2));
    let closed = matches!(
        output.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Closed)
    );
    let retired_exited = retiring.exited_within(Duration::ZERO);
    let peer_exited = peer.exited_within(Duration::ZERO);
    // Finish owned cleanup before exposing the intended missing exit-state RED.
    peer.end();
    assert!(reaped, "the real retirement receipt did not settle");
    assert!(closed, "retirement must close the owned activity stream");
    assert!(shared_log.ended(), "the actual end call was not recorded");
    assert!(
        !peer_exited,
        "another session's retirement cannot end a live peer"
    );
    assert!(
        retired_exited,
        "a finite Waiting session still reports no exit after its actual retirement"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn finite_turns_report_actual_working_then_waiting_and_join_on_retirement() {
    let log = SessionLog::default();
    let (receipts, watched_receipts) =
        tokio::sync::watch::channel(crate::harness::TurnReceiptSnapshot::default());
    let (activity, _) = tokio::sync::broadcast::channel::<crate::harness::ActivityReport>(8);
    let session = Arc::new(
        DictatedSession::reporting(AgentStatus::Waiting)
            .ending_finitely()
            .recording_into(&log)
            .watching_receipts(watched_receipts)
            .reporting_activity(activity)
            .finishing_turns_after(Duration::from_millis(25)),
    );
    let status = session.status_changed();
    let started = std::time::Instant::now();
    session
        .send_turn(
            &Turn::new("complete this actual finite turn")
                .with_operation_id("finite-turn-operation"),
        )
        .unwrap();
    let working_after_send = session.status() == AgentStatus::Working;
    let completed = if let Some(status) = status.as_ref() {
        wait_for(Duration::from_millis(500), || {
            let snapshot = status.borrow();
            (snapshot.status == AgentStatus::Waiting && snapshot.last_worked_at.is_some())
                .then_some(snapshot.clone())
        })
        .await
    } else {
        None
    };
    let completion_elapsed = started.elapsed();
    let recorded = log.turns();
    let receipt_snapshot = session.turn_receipts().unwrap().borrow().clone();
    let retiring: Arc<dyn AgentSession> = session;
    let retirement = crate::reaper::Retirement::begin(retiring);
    let retired = tokio::task::spawn_blocking(move || retirement.wait(Duration::from_secs(2)))
        .await
        .unwrap();
    drop(receipts);
    let status_closed_after_retirement = status
        .as_ref()
        .is_some_and(|status| status.has_changed().is_err());
    assert!(
        retired,
        "finite worker did not join through actual retirement"
    );
    assert_eq!(recorded, vec!["complete this actual finite turn"]);
    assert_eq!(
        receipt_snapshot,
        crate::harness::TurnReceiptSnapshot::default(),
        "finite timing must not invent provider consumption receipts"
    );
    assert!(
        working_after_send,
        "actual send did not transition the opted-in Waiting session to Working"
    );
    assert!(
        completed.is_some(),
        "owned status stream did not finish the actual turn as Waiting"
    );
    assert!(
        completion_elapsed <= Duration::from_millis(500),
        "finite turn exceeded its declared bound"
    );
    assert!(
        status_closed_after_retirement,
        "retirement left an owned status producer able to update after teardown"
    );
}

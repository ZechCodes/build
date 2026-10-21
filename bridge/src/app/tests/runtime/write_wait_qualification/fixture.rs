//! Native temporary residents for the declared total qualification envelope.
//! Cardinalities include primary checkouts and every supplemental history.
use super::rpc::RpcHarness;
use crate::app::tests::project_agent::{added_project, rooted};
use crate::app::tests::runtime::idle_sessions::fake_run_record;
use crate::app::tests::tracker::filed;
use crate::app::tests::*;
use crate::timing::write_wait::WriteWaitRegistry;
use std::sync::{Arc, Mutex};

mod cleanup;

#[derive(Clone, Copy, Debug)]
pub(super) enum Envelope {
    Small,
    Large,
}

impl Envelope {
    fn label(self) -> &'static str {
        match self {
            Self::Small => "small",
            Self::Large => "large",
        }
    }
    fn tasks(self) -> usize {
        match self {
            Self::Small => 100,
            Self::Large => 10_000,
        }
    }
    fn workspaces(self) -> usize {
        match self {
            Self::Small => 10,
            Self::Large => 200,
        }
    }
    fn thread_items(self) -> usize {
        match self {
            Self::Small => 1000,
            Self::Large => 100_000,
        }
    }
    fn workspace_distribution(self) -> [usize; 4] {
        match self {
            Self::Small => [3, 3, 2, 2],
            Self::Large => [50; 4],
        }
    }
}

pub(super) struct ProjectContext {
    pub project_id: String,
    pub repository: PathBuf,
    pub source_id: String,
    pub tasks: Vec<String>,
}

#[derive(Clone)]
pub(super) struct WorkspaceContext {
    pub project: usize,
    pub workspace_id: String,
    pub source_id: String,
    pub directory: PathBuf,
    pub owner: String,
    pub agent_id: String,
    pub conversation_id: String,
    pub thread_id: String,
}

pub(super) struct QualificationFixture {
    pub state: Arc<Mutex<AppState>>,
    pub registry: Arc<WriteWaitRegistry>,
    pub rpc: RpcHarness,
    pub projects: Vec<ProjectContext>,
    pub residents: Vec<WorkspaceContext>,
    pub active_workspaces: Vec<WorkspaceContext>,
    pub sessions: Vec<(TabKey, Arc<dyn AgentSession>)>,
    pub logs: Vec<SessionLog>,
    pub read_inputs: Option<Arc<super::read_recipes::ReadyReadInputs>>,
    _root: tempfile::TempDir,
}

struct NativeFixture {
    app: AppState,
    root: tempfile::TempDir,
    projects: Vec<ProjectContext>,
    residents: Vec<WorkspaceContext>,
    active_workspaces: Vec<WorkspaceContext>,
    logs: Vec<SessionLog>,
}

impl QualificationFixture {
    pub(super) fn new(envelope: Envelope) -> Self {
        let mut native = NativeFixture::prepare(envelope);
        super::history_fill::fill(&mut native.app, envelope.thread_items());
        native.finish()
    }

    /// Actual full read prerequisites, including the bounded real Router boot.
    /// #726 remains a prerequisite; this constructor never substitutes a fake
    /// successful Router startup or a missing read recipe.
    pub(super) async fn ready(envelope: Envelope) -> Self {
        let mut native = NativeFixture::prepare(envelope);
        let project = &native.projects[0];
        let workspace = &native.active_workspaces[0];
        let bounded = super::read_recipes::SeededReadInputs::seed(
            &mut native.app,
            super::read_recipes::SeedContext {
                project: &project.project_id,
                workspace: &workspace.workspace_id,
                owner: &workspace.owner,
                agent: &workspace.agent_id,
                task: &project.tasks[project.tasks.len() - 2],
                dependent: &project.tasks[project.tasks.len() - 1],
            },
        );
        let prepared = super::read_recipes::PreparedReadInputs::prepare(
            &mut native.app,
            native.root.path(),
            bounded,
            &native.projects,
            &native.residents,
            &native.active_workspaces,
        );
        let remaining = envelope
            .thread_items()
            .checked_sub(prepared.reserved_tail_items())
            .expect("read tail is inside the global history budget");
        super::history_fill::fill(&mut native.app, remaining);
        prepared.finish_history(&mut native.app, envelope.thread_items() as u64);
        let mut fixture = native.finish();
        use futures_util::FutureExt;
        let outcome = std::panic::AssertUnwindSafe(prepared.finalize(&fixture.rpc))
            .catch_unwind()
            .await;
        match outcome {
            Ok(ready) => {
                fixture.read_inputs = Some(Arc::new(ready));
                fixture
            }
            Err(original) => {
                // The read prerequisite's own guard retires its planned
                // Router/shell before this catch boundary. Keep native roots
                // until actual carrier receivers/shared work also finish;
                // preserve the original #726/startup failure afterward.
                let cleanup = fixture.cleanup_owned_resources().await;
                for error in cleanup {
                    eprintln!("startup failure cleanup: {error}");
                }
                std::panic::resume_unwind(original)
            }
        }
    }

    pub(super) async fn shutdown(self) {
        self.counts_and_shutdown().await;
    }
}

impl NativeFixture {
    fn prepare(envelope: Envelope) -> Self {
        let root = tempfile::tempdir().unwrap();
        let mut app = rooted(root.path())
            .with_task_store(root.path().join("store"))
            .unwrap();
        let mut projects = Vec::new();
        let mut residents = Vec::new();
        let mut active_workspaces = Vec::new();
        let per_project = envelope.workspace_distribution();
        for (project_index, resident_count) in per_project.into_iter().enumerate() {
            let repo = crate::git_fixture::init_repo_named(
                root.path(),
                &format!("project-{project_index}"),
            );
            let project = added_project(&mut app, &repo);
            let source = app.projects.get(&project).unwrap().sources[0].id.clone();
            let tasks = seed_tasks(&mut app, &project, envelope.tasks() / 4);
            let repository = git2::Repository::open(&repo).unwrap();
            for resident in 0..resident_count {
                let name = format!("qual-{}-{project_index}-{resident:03}", envelope.label());
                // Project creation already exposes its primary checkout as a
                // real legacy workspace. It consumes one declared resident;
                // adding resident_count more linked checkouts would inflate
                // the small/large envelopes by four.
                let (owner, path, branch) = if resident == 0 {
                    (
                        format!("legacy-{project}"),
                        repo.clone(),
                        "main".to_string(),
                    )
                } else {
                    let path = root.path().join(&name);
                    repository.worktree(&name, &path, None).unwrap();
                    (format!("run-{name}"), path, name.clone())
                };
                app.workspaces.adopt_root(
                    &project,
                    owner.clone(),
                    owner.clone(),
                    path.clone(),
                    source.clone(),
                    true,
                );
                let mut record = fake_run_record(&owner);
                record.project_path = repo.display().to_string();
                record.worktree_path = path.display().to_string();
                record.worktree_name = name.clone();
                record.branch = branch;
                record.adopted = true;
                let agent = crate::agent::derived_agent_id(&owner);
                record.agents[0] = crate::agent::Agent::new(
                    agent.clone(),
                    &owner,
                    record.agents[0].choice.clone(),
                    1,
                    record.created_at.clone(),
                );
                record.agents[0].watched = true;
                let active = crate::orchestrator::ActiveRun::reattach(&record, String::new());
                let primary = active.agents.primary().unwrap();
                let conversation_id = primary.conversation_id().to_string();
                let thread_id = primary.thread.id.clone();
                app.projects.bind_entity(owner.clone(), project.clone());
                app.persist_run_record(&owner, &active).unwrap();
                app.runs.insert(owner.clone(), active);
                let context = WorkspaceContext {
                    project: project_index,
                    workspace_id: owner.clone(),
                    source_id: source.clone(),
                    directory: path,
                    owner,
                    agent_id: agent,
                    conversation_id,
                    thread_id,
                };
                if resident < 2 {
                    active_workspaces.push(context.clone());
                }
                residents.push(context);
            }
            projects.push(ProjectContext {
                project_id: project,
                repository: repo,
                source_id: source,
                tasks,
            });
        }
        assert_eq!(projects.len(), 4);
        assert_eq!(
            projects.iter().map(|p| p.tasks.len()).sum::<usize>(),
            envelope.tasks()
        );
        assert_eq!(residents.len(), envelope.workspaces());
        assert_eq!(active_workspaces.len(), 8);
        let mut logs = Vec::new();
        for workspace in &active_workspaces {
            let log = SessionLog::default();
            let mut tab = dictated_agent_tab(
                &workspace.directory,
                &workspace.owner,
                &workspace.agent_id,
                DictatedSession::reporting(AgentStatus::Working).recording_into(&log),
            );
            let choice = app.runs[&workspace.owner]
                .agents
                .by_id(&workspace.agent_id)
                .unwrap()
                .choice
                .clone();
            tab.session_instance = app.record_agent_session_start(
                &workspace.owner,
                &workspace.agent_id,
                &workspace.directory,
                &choice,
                "qualification-fixture",
            );
            let key = TabKey::agent(
                &AppState::canonical_root(&workspace.directory),
                &workspace.agent_id,
            );
            app.session_registry.test_insert_tab(key, tab);
            app.test_install_mcp_token(
                workspace.agent_id.clone(),
                format!("qual-token-{}", workspace.agent_id),
            );
            logs.push(log);
            // Real finite fs.write targets rotate bounded existing editor bodies.
            std::fs::write(
                workspace.directory.join("qualified-editor.txt"),
                b"initial qualification text\n",
            )
            .unwrap();
            std::fs::write(
                workspace.directory.join("qualified-stage.txt"),
                b"qualified local Git stage\n",
            )
            .unwrap();
        }
        // Global's two-phase read seed reuses existing tasks/owners and may
        // add exactly declared supplemental legacy plan records. Their thread
        // items and8session starts consume the SAME envelope history budget.
        // SeededReadInputs::seed(&mut app, root.path(), &projects, &active_workspaces)
        // returns ledger of all canonical histories and delayed archive action.
        // Fill deterministically only AFTER those inputs exist, counting unique
        // canonical IDs across runs AND legacy plans, not only resident runs.
        // Assert summed total_item_count()==envelope.thread_items(), then archive
        // the selected inactive existing run and assert persisted count remains.
        app.session_locator_factory =
            Arc::new(|_, _| panic!("qualification must not launch an unplanned provider"));
        app.delivery_queue.clear_queued();
        app.seed_anchors_for_records_without_one();
        // Install fake CLI/catalogue/update/GitHub inputs from reusable read
        // builders BEFORE warm/listener capture; no real executable probe.
        Self {
            app,
            root,
            projects,
            residents,
            active_workspaces,
            logs,
        }
    }

    fn finish(self) -> QualificationFixture {
        let Self {
            app,
            root,
            projects,
            residents,
            active_workspaces,
            logs,
        } = self;
        let state = app.shared();
        let registry = WriteWaitRegistry::new();
        let sessions = {
            let app = state.lock().unwrap();
            active_workspaces
                .iter()
                .map(|workspace| {
                    let key = TabKey::agent(
                        &AppState::canonical_root(&workspace.directory),
                        &workspace.agent_id,
                    );
                    let tab = app.session_registry.test_tab(&key).unwrap();
                    assert!(tab.session_is_live());
                    (key, Arc::clone(&tab.session))
                })
                .collect()
        };
        let rpc = RpcHarness::new(Arc::clone(&state), Arc::clone(&registry));
        QualificationFixture {
            state,
            registry,
            rpc,
            projects,
            residents,
            active_workspaces,
            sessions,
            logs,
            read_inputs: None,
            _root: root,
        }
    }
}

fn seed_tasks(app: &mut AppState, project: &str, count: usize) -> Vec<String> {
    let first = filed(app, project, "qualification seed task");
    let first_id = first["id"].as_str().unwrap().to_string();
    let template = app
        .tracker_store()
        .unwrap()
        .load_tracker_task(&first_id)
        .unwrap()
        .unwrap();
    let mut ids = vec![first_id];
    for index in 1..count {
        let mut draft = template.clone();
        draft.id = crate::tracker::new_task_id();
        draft.title = format!("qualification bounded task {index}");
        draft.number = 0;
        let event = crate::tracker::TaskEvent::new(
            &draft.id,
            crate::tracker::Actor::User,
            crate::tracker::TaskEventKind::Created,
            json!({"title":draft.title}),
            &draft.created_at,
        );
        let task = app
            .tracker_store()
            .unwrap()
            .create_tracker_task(draft, &[event])
            .unwrap();
        ids.push(task.id);
    }
    ids
}

#[derive(Debug)]
struct NativeCounts {
    projects: usize,
    task_counts: Vec<usize>,
    primary_checkout_counts: Vec<usize>,
    linked_checkout_counts: Vec<usize>,
    registry_residents: usize,
    active_workspaces: usize,
    canonical_histories: usize,
    total_thread_items: u64,
    live_sessions: usize,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn small_native_fixture_preserves_declared_totals_and_eight_real_fake_session_bindings() {
    crate::git_fixture::environment::isolated_git_test!();
    let fixture = QualificationFixture::new(Envelope::Small);
    let counts = fixture.counts_and_shutdown().await;
    assert_eq!(counts.projects, 4);
    assert_eq!(counts.task_counts, vec![25; 4]);
    assert_eq!(counts.primary_checkout_counts, vec![1; 4]);
    assert_eq!(counts.linked_checkout_counts, vec![2, 2, 1, 1]);
    assert_eq!(
        counts
            .primary_checkout_counts
            .iter()
            .zip(&counts.linked_checkout_counts)
            .map(|(primary, linked)| primary + linked)
            .collect::<Vec<_>>(),
        vec![3, 3, 2, 2],
        "the primary checkouts are part of the pinned resident total"
    );
    assert_eq!(counts.registry_residents, 10);
    assert_eq!(counts.active_workspaces, 8);
    assert!(counts.canonical_histories >= 10);
    assert_eq!(counts.live_sessions, 8);
    assert_eq!(
        counts.total_thread_items, 1000,
        "read-fixture and generic history must share one actual total budget"
    );
}

#[test]
fn large_envelope_counts_are_totals_not_silently_multiplied_by_four() {
    let envelope = Envelope::Large;
    assert_eq!(envelope.tasks(), 10_000);
    assert_eq!(envelope.workspace_distribution(), [50; 4]);
    assert_eq!(envelope.workspaces(), 200);
    assert_eq!(envelope.thread_items(), 100_000);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn autonomous_status_producer_retains_background_origin_while_waiting_for_app_state() {
    crate::git_fixture::environment::isolated_git_test!();
    use crate::timing::write_wait::{IntervalCategory, WaitKind, WriteCohort};
    use std::time::{Duration, Instant};
    let mut fixture = QualificationFixture::new(Envelope::Small);
    let base = Instant::now();
    let elapsed = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let sampled = Arc::clone(&elapsed);
    fixture.registry = WriteWaitRegistry::with_clock(Arc::new(move || {
        base + Duration::from_secs(sampled.load(std::sync::atomic::Ordering::Acquire))
    }));
    fixture
        .state
        .lock()
        .unwrap()
        .frame_clock
        .set_write_wait_registry(Arc::clone(&fixture.registry));
    let (breached_tx, breached_rx) = std::sync::mpsc::channel();
    let watchdog = fixture.registry.watchdog(
        Duration::from_millis(1),
        Arc::new(move |row| {
            let _ = breached_tx.send(row);
        }),
    );
    let (key, session) = &fixture.sessions[0];
    let instance = fixture
        .state
        .lock()
        .unwrap()
        .session_registry
        .test_tab(key)
        .unwrap()
        .session_instance
        .clone()
        .unwrap();
    let mut pump = crate::app::runtime::pumps::status_timing::MeasuredStatusPump::new(
        &fixture.state,
        key.clone(),
        session,
        instance,
    );
    let original = "qual/small/trial1/background/status/00000";
    let root = fixture
        .registry
        .begin_background(original, "agent.poll_status", base);
    let context = root.context();
    let state = Arc::clone(&fixture.state);
    let (held_tx, held_rx) = std::sync::mpsc::sync_channel(0);
    let (release_tx, release_rx) = std::sync::mpsc::sync_channel(0);
    let holder = std::thread::spawn(move || {
        let held = state.lock().unwrap();
        held_tx.send(()).unwrap();
        release_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        drop(held);
    });
    held_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    let (started_tx, started_rx) = std::sync::mpsc::sync_channel(0);
    let worker = std::thread::spawn(move || {
        let binding = context.bind_sync();
        started_tx.send(()).unwrap();
        pump.observe(&crate::harness::SessionStatusSnapshot::new(
            AgentStatus::Working,
        ));
        drop(binding);
        root.complete();
    });
    started_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    // Observe only immutable active registry roots, without attempting AppState.
    // Cleanup and join both real work and the holder before any assertion.
    let until = Instant::now() + Duration::from_millis(250);
    let active = loop {
        let rows = fixture.registry.active_snapshot();
        let row = rows
            .into_iter()
            .find(|row| row.original_id == original)
            .unwrap();
        if row
            .active
            .iter()
            .any(|interval| interval.category == IntervalCategory::KnownWait(WaitKind::AppLock))
            || Instant::now() >= until
        {
            break row;
        }
        std::thread::sleep(Duration::from_millis(1));
    };
    elapsed.store(6, std::sync::atomic::Ordering::Release);
    let reported = breached_rx.recv_timeout(Duration::from_millis(250)).ok();
    let unfinished = fixture
        .registry
        .active_snapshot()
        .into_iter()
        .find(|row| row.original_id == original)
        .unwrap();
    release_tx.send(()).unwrap();
    holder.join().unwrap();
    worker.join().unwrap();
    let final_row = fixture
        .registry
        .snapshot()
        .into_iter()
        .find(|row| row.original_id == original)
        .unwrap();
    drop(watchdog);
    fixture.counts_and_shutdown().await;
    assert_eq!(active.cohort, WriteCohort::BackgroundWork);
    assert!(!active.root_finished);
    assert!(
        active
            .active
            .iter()
            .any(|interval| interval.category == IntervalCategory::KnownWait(WaitKind::AppLock)),
        "actual background status guard must be visible while unfinished: {active:?}"
    );
    assert_eq!(
        reported.as_ref().map(|row| row.original_id.as_str()),
        Some(original),
        "independent core watchdog must observe the unfinished BackgroundWork origin"
    );
    assert!(unfinished.breached());
    assert!(final_row.root_finished && final_row.outstanding.is_empty());
    assert!(!final_row.abandoned && !final_row.cancelled);
}

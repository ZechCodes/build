//! Native temporary residents for the declared total qualification envelope.
//! The history/read-input stage remains explicitly incomplete until its RED control.
use super::rpc::RpcHarness;
use crate::app::tests::project_agent::{added_project, rooted};
use crate::app::tests::runtime::idle_sessions::fake_run_record;
use crate::app::tests::tracker::filed;
use crate::app::tests::*;
use crate::timing::write_wait::WriteWaitRegistry;
use std::sync::{Arc, Mutex};

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
    _root: tempfile::TempDir,
}

impl QualificationFixture {
    pub(super) fn new(envelope: Envelope) -> Self {
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
                let owner = format!("run-{name}");
                let path = root.path().join(&name);
                repository.worktree(&name, &path, None).unwrap();
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
                record.branch = name;
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
                let thread_id = active
                    .agents
                    .primary()
                    .unwrap()
                    .conversation_id()
                    .to_string();
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
        Self {
            state,
            registry,
            rpc,
            projects,
            residents,
            active_workspaces,
            sessions,
            logs,
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
    git_workspace_counts: Vec<usize>,
    registry_residents: usize,
    active_workspaces: usize,
    canonical_histories: usize,
    total_thread_items: u64,
    live_sessions: usize,
}

impl QualificationFixture {
    async fn counts_and_shutdown(self) -> NativeCounts {
        let counts = {
            let app = self.state.lock().unwrap();
            let mut canonical = std::collections::BTreeMap::new();
            for run in app.runs.values() {
                for agent in run.agents.iter() {
                    canonical.insert(
                        agent.conversation_id().to_string(),
                        agent.thread.total_item_count(),
                    );
                }
            }
            for plan in app.plans.values() {
                for agent in plan.agents.iter() {
                    canonical.insert(
                        agent.conversation_id().to_string(),
                        agent.thread.total_item_count(),
                    );
                }
            }
            NativeCounts {
                projects: self.projects.len(),
                task_counts: self
                    .projects
                    .iter()
                    .map(|project| {
                        app.tracker_store()
                            .unwrap()
                            .list_tracker_tasks(
                                project.repository.to_str().unwrap(),
                                Default::default(),
                            )
                            .unwrap()
                            .len()
                    })
                    .collect(),
                git_workspace_counts: self
                    .projects
                    .iter()
                    .map(|project| {
                        git2::Repository::open(&project.repository)
                            .unwrap()
                            .worktrees()
                            .unwrap()
                            .len()
                    })
                    .collect(),
                registry_residents: app.workspaces.list(None).len(),
                active_workspaces: self.active_workspaces.len(),
                canonical_histories: canonical.len(),
                total_thread_items: canonical.values().sum(),
                live_sessions: self
                    .sessions
                    .iter()
                    .filter(|(key, session)| {
                        app.session_registry.test_tab(key).is_some_and(|tab| {
                            tab.session_is_live() && Arc::ptr_eq(&tab.session, session)
                        })
                    })
                    .count(),
            }
        };
        eprintln!("qualification_native_fixture_counts={counts:?}");
        let Self {
            state, rpc, _root, ..
        } = self;
        let weak = Arc::downgrade(&state);
        drop(state);
        rpc.finish().await;
        let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while weak.upgrade().is_some() {
            assert!(
                std::time::Instant::now() < until,
                "native background work retained the fixture state"
            );
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        // Temporary repositories stay alive through receiver shutdown and the
        // last active weak service, even when a cardinality assertion will fail.
        drop(_root);
        counts
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn small_native_fixture_preserves_declared_totals_and_eight_real_fake_session_bindings() {
    crate::git_fixture::environment::isolated_git_test!();
    let fixture = QualificationFixture::new(Envelope::Small);
    let counts = fixture.counts_and_shutdown().await;
    assert_eq!(counts.projects, 4);
    assert_eq!(counts.task_counts, vec![25; 4]);
    assert_eq!(counts.git_workspace_counts, vec![3, 3, 2, 2]);
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

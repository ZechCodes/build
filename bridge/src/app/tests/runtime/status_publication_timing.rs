//! Status-pump critical-section measurements, separate from structural capture tests.

use super::*;
use crate::app::runtime::pumps::status_timing::{GuardSample, MeasuredStatusPump};
use crate::harness::SessionStatusSnapshot;
use serde_json::json;
use std::io::Write;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy)]
struct Scale {
    name: &'static str,
    plans: usize,
    runs: usize,
    workspaces: usize,
    archives: usize,
}

const SCALES: [Scale; 2] = [
    Scale {
        name: "small",
        plans: 4,
        runs: 12,
        workspaces: 4,
        archives: 64,
    },
    Scale {
        name: "representative",
        plans: 16,
        runs: 128,
        workspaces: 32,
        archives: 512,
    },
];

struct Fixture {
    _directory: tempfile::TempDir,
    state: Arc<Mutex<AppState>>,
    pump: MeasuredStatusPump,
    status: tokio::sync::watch::Sender<SessionStatusSnapshot>,
    owner: String,
    snapshot: SessionStatusSnapshot,
}

impl Fixture {
    fn new(scale: Scale) -> Self {
        let (directory, repo) = init_repo();
        let mut app = qa_state_before_admission(&repo, directory.path());
        let project = app.project_at(0).id.clone();
        let plans = seed_plans(&mut app, scale.plans);
        seed_workspaces(&mut app, directory.path(), &project, scale.workspaces);
        seed_runs(
            &mut app,
            &repo,
            directory.path(),
            &project,
            &plans,
            scale.runs,
        );
        seed_archive(&mut app, &repo, directory.path(), scale.archives);
        let owner = "status-retained-0".to_string();
        let agent = crate::agent::derived_agent_id(&owner);
        let root = app.runs[&owner].worktree.path.clone();
        let snapshot = SessionStatusSnapshot::new(AgentStatus::Working);
        let (status, watched) = tokio::sync::watch::channel(snapshot.clone());
        let key = crate::app::tests::protocol::status::insert_agent_tab(
            &mut app,
            &root,
            &owner,
            &agent,
            DictatedSession::reporting(AgentStatus::Working).watching_status(watched),
        );
        let tab = app.session_registry.test_tab(&key).unwrap();
        let session = Arc::clone(&tab.session);
        let instance = tab
            .session_instance
            .clone()
            .expect("real recorded session instance");
        app.delivery_queue.clear_queued();
        app.initialize_snapshot_projection();
        assert_eq!(app.plans.len(), scale.plans);
        assert_eq!(app.runs.len(), scale.runs);
        assert_eq!(app.workspaces.list(None).len(), scale.workspaces);
        assert_eq!(app.board.archived_values().count(), scale.archives);
        assert_eq!(
            app.require_store()
                .unwrap()
                .load_all_archived_worktrees()
                .unwrap()
                .len(),
            scale.archives,
        );
        assert!(
            !app.frame_clock.observes_requests(),
            "phase tracing remains off"
        );
        let state = app.shared();
        let pump = MeasuredStatusPump::new(&state, key, &session, instance);
        Self {
            _directory: directory,
            state,
            pump,
            status,
            owner,
            snapshot,
        }
    }

    fn event(&mut self) -> GuardSample {
        let next = if self.snapshot.status == AgentStatus::Working {
            AgentStatus::Waiting
        } else {
            AgentStatus::Working
        };
        self.snapshot = self.snapshot.transition(next).unwrap();
        self.status.send(self.snapshot.clone()).unwrap();
        let sample = self.pump.observe(&self.snapshot);
        let app = self.state.lock().unwrap();
        assert!(!app.frame_clock.observes_requests());
        assert!(app.delivery_queue.queued().next().is_none());
        if next == AgentStatus::Waiting {
            assert_eq!(
                app.board.attention().last_worked_at(&self.owner),
                self.snapshot.last_worked_at.as_deref()
            );
        }
        sample
    }
}

fn seed_plans(app: &mut AppState, count: usize) -> Vec<String> {
    (0..count)
        .map(|index| {
            let id = file_legacy_task(app, &format!("status retained plan {index}"));
            let mut plan = app.take_plan(&id).unwrap();
            plan.plan.state = crate::plan::PlanState::Approved;
            app.finish_plan_mutation(id.clone(), plan).unwrap();
            id
        })
        .collect()
}

fn seed_workspaces(app: &mut AppState, directory: &Path, project: &str, count: usize) {
    let repository = app.project_at(0).repo_path.clone();
    let source = app.project_at(0).sources[0].id.clone();
    for index in 0..count {
        let id = format!("status-workspace-{index}");
        let root = directory.join(&id);
        let finished = index % 2 == 1;
        if finished {
            // Finished contents must be clean and published. These clones
            // track only the temporary local source, never a network remote.
            git_in(
                directory,
                &[
                    "clone",
                    "--local",
                    repository.to_str().unwrap(),
                    root.to_str().unwrap(),
                ],
            );
        } else {
            std::fs::create_dir_all(&root).unwrap();
        }
        let mut workspace =
            app.workspaces
                .adopt_root(project, id.clone(), id, root, source.clone(), finished);
        if finished {
            app.workspaces.record_finished(&mut workspace).unwrap();
        }
    }
    app.workspaces.reload().unwrap();
}

fn seed_runs(
    app: &mut AppState,
    repo: &Path,
    directory: &Path,
    project: &str,
    plans: &[String],
    count: usize,
) {
    for index in 0..count {
        let id = format!("status-retained-{index}");
        let root = directory.join(&id);
        std::fs::create_dir_all(&root).unwrap();
        let mut record = super::idle_sessions::fake_run_record(&id);
        record.project_path = repo.display().to_string();
        record.worktree_path = root.display().to_string();
        record.plan_id = Some(plans[index % plans.len()].clone());
        record.state = if index % 2 == 1 {
            RunState::Archived
        } else {
            RunState::Building
        };
        record.branch = format!("build/status-retained-{index}");
        let active = ActiveRun::reattach(&record, ".build/plan.md".into());
        app.projects.bind_entity(id.clone(), project.to_string());
        app.persist_run_record(&id, &active).unwrap();
        app.runs.insert(id, active);
    }
}

fn seed_archive(app: &mut AppState, repo: &Path, directory: &Path, count: usize) {
    use crate::store::{WorktreeFinishAction, WorktreeFinishStatus};
    let records: std::collections::HashMap<_, _> = (0..count)
        .map(|index| {
            let id = format!("status-archive-{index}");
            (
                id.clone(),
                PersistedArchivedWorktree {
                    status: WorktreeFinishStatus::Archived,
                    project_path: repo.display().to_string(),
                    worktree_id: id.clone(),
                    worktree_name: id.clone(),
                    worktree_path: directory.join(&id).display().to_string(),
                    branch: Some(id),
                    head_sha: String::new(),
                    upstream: None,
                    unpushed: None,
                    dirty_files: 0,
                    uncommitted_files: 0,
                    uncommitted_insertions: 0,
                    uncommitted_deletions: 0,
                    action: WorktreeFinishAction::Cleanup,
                    archived_at: Some("2026-10-20T12:00:00Z".into()),
                },
            )
        })
        .collect();
    for record in records.values() {
        app.require_store()
            .unwrap()
            .save_archived_worktree(record)
            .unwrap();
    }
    app.board.replace_archived(records);
}

fn count(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .map(|value| value.parse().expect("positive sample count"))
        .unwrap_or(default)
}

#[test]
#[ignore = "explicit paired status-event benchmark; not a routine correctness gate"]
fn status_event_application_guard_benchmark() {
    let samples = count("BUILD_STATUS_BENCH_SAMPLES", 128);
    let warmups = count("BUILD_STATUS_BENCH_WARMUPS", 16);
    assert!(samples > 0);
    let output = PathBuf::from(std::env::var("BUILD_STATUS_BENCH_OUTPUT").expect("output path"));
    let epoch = Instant::now();
    let unix_ns = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let mut records = vec![
        json!({"kind":"configuration","unix_ns":unix_ns,"samples":samples,"warmups":warmups,
        "trace":false,"path":"StatusPump::observe","status_sequence":"Working/Waiting alternating; no context/usage event or queued delivery",
        "guard_wait_endpoint":"immediately before app lock through acquisition","guard_hold_endpoint":"after acquisition through immediately after MutexGuard drop",
        "collector":"cfg(test) two guard timestamps; thread-local recording only after app guard release; same overlay before/fixed",
        "fixture_semantics":"file-backed Store; approved legacy plans; persisted reattached retained runs in plain temporary directories; adopted workspaces half plain/ready and half clean tracked local Git clones/finished; persisted archived worktree rows; no live agent process or Git checkout per retained run/archive row"}),
    ];
    for scale in SCALES {
        let setup = Instant::now();
        let mut fixture = Fixture::new(scale);
        records.push(json!({"kind":"setup","scale":scale.name,"duration_ns":setup.elapsed().as_nanos(),
            "plans":scale.plans,"runs":scale.runs,"workspaces":scale.workspaces,"finished_workspaces":scale.workspaces/2,
            "archived_runs":scale.runs/2,"archived_records":scale.archives}));
        for index in 0..warmups + samples {
            let start_ns = epoch.elapsed().as_nanos();
            let sample = fixture.event();
            records.push(json!({"kind":"event","scale":scale.name,"population":if index < warmups {"warmup"} else {"measured"},
                "index":index,"start_ns":start_ns,"end_ns":epoch.elapsed().as_nanos(),
                "status":if fixture.snapshot.status==AgentStatus::Working {"working"} else {"waiting"},
                "guard_wait_ns":sample.wait.as_nanos(),"guard_hold_ns":sample.held.as_nanos(),"observe_wall_ns":sample.observe.as_nanos()}));
        }
    }
    let mut file = std::fs::File::create(output).unwrap();
    for record in records {
        writeln!(file, "{record}").unwrap();
    }
}

//! Measure actual owner commits, including the new native snapshot preparation.
use super::*;

const TASKS: usize = 16;
const RUNS: usize = 32;
const ITEMS: usize = 16;
const SAMPLES: usize = 5;
const NOW: &str = "2026-10-21T12:00:00Z";

struct Fixture {
    _directory: tempfile::TempDir,
    state: Arc<Mutex<AppState>>,
    task: String,
    run: String,
}

impl Fixture {
    fn new(agents_per_run: usize) -> Self {
        let (directory, repository) = init_repo();
        let mut app = qa_state_before_admission(&repository, directory.path());
        let project = app.project_at(0).id.clone();
        let tasks: Vec<_> = (0..TASKS)
            .map(|index| {
                let id = file_legacy_task(&mut app, &format!("native scale task {index}"));
                let mut plan = app.take_plan(&id).unwrap();
                for item in 0..ITEMS {
                    plan.agents.sole_thread_mut().post_user(
                        format!("task {index} retained message {item}"),
                        None,
                        NOW,
                    );
                }
                app.finish_plan_mutation(id.clone(), plan).unwrap();
                id
            })
            .collect();
        for index in 0..RUNS {
            let id = format!("run-native-scale-{index}");
            let root = directory.path().join(&id);
            std::fs::create_dir_all(&root).unwrap();
            let task = &tasks[index % TASKS];
            let mut record = crate::app::tests::runtime::idle_sessions::fake_run_record(&id);
            record.project_path = repository.display().to_string();
            record.worktree_path = root.display().to_string();
            record.plan_id = Some(task.clone());
            record.branch = format!("build/native-scale-{index}");
            record.agents = (0..agents_per_run)
                .map(|ordinal| {
                    let mut agent = crate::agent::Agent::new(
                        format!("agent-native-scale-{index}-{ordinal}"),
                        &id,
                        Default::default(),
                        ordinal as u32 + 1,
                        NOW,
                    );
                    if ordinal == 0 {
                        agent.conversation_id =
                            Some(app.plans[task].agents.sole().conversation_id().to_owned());
                    } else {
                        for item in 0..ITEMS {
                            agent.thread.post_user(
                                format!("run {index} agent {ordinal} retained message {item}"),
                                None,
                                NOW,
                            );
                        }
                    }
                    agent
                })
                .collect();
            let active = ActiveRun::reattach(&record, ".build/plan.md".into());
            app.projects.bind_entity(id.clone(), project.clone());
            app.persist_run_record(&id, &active).unwrap();
            app.runs.insert(id, active);
        }
        app.initialize_snapshot_projection();
        app.delivery_queue.clear_queued();
        assert_eq!(app.plans.len(), TASKS);
        assert_eq!(app.runs.len(), RUNS);
        assert_eq!(
            app.runs.values().map(|run| run.agents.len()).sum::<usize>(),
            RUNS * agents_per_run
        );
        Self {
            _directory: directory,
            state: Arc::new(Mutex::new(app)),
            task: tasks[0].clone(),
            run: "run-native-scale-0".into(),
        }
    }

    fn commit(&self, plan: bool, sample: usize) {
        let clock = Arc::clone(&self.state.lock().unwrap().frame_clock);
        let timer = clock.frame("agent.choose");
        timer.sample_tool(if plan {
            "plan_commit"
        } else {
            "linked_run_commit"
        });
        let mut app = timer.lock(&self.state);
        if plan {
            let mut active = app.take_plan(&self.task).unwrap();
            active.last_summary = Some(format!("plan sample {sample}"));
            app.finish_plan_mutation(self.task.clone(), active).unwrap();
        } else {
            let mut active = app.take_run(&self.run).unwrap();
            active.last_summary = Some(format!("run sample {sample}"));
            app.finish_run_mutation(self.run.clone(), active).unwrap();
        }
    }

    fn assert_publication(&self) {
        let app = self.state.lock().unwrap();
        let view = app.read_view.load();
        let task = view.get_owned("task.get", &self.task).unwrap();
        assert_eq!(task["summary"], format!("plan sample {}", SAMPLES - 1));
        let lineage = task["implementation_lineage"].as_array().unwrap();
        assert_eq!(lineage.len(), RUNS / TASKS);
        assert!(lineage.iter().any(|run| run["run_id"] == self.run));
        assert!(view.get_owned("task.stages", &self.task).is_some());
        assert!(view.get_owned("agent.list", &self.run).is_some());
    }
}

#[test]
fn owner_commit_native_snapshot_scale() {
    for agents_per_run in [1, 4, 16, 32] {
        let fixture = Fixture::new(agents_per_run);
        // Keep initialization and first-use allocation outside measured commits.
        fixture.commit(true, usize::MAX);
        fixture.commit(false, usize::MAX);
        let clock = Arc::clone(&fixture.state.lock().unwrap().frame_clock);
        let phases = clock.capture_phases();
        let load_before = std::fs::read_to_string("/proc/loadavg").unwrap();
        for sample in 0..SAMPLES {
            fixture.commit(true, sample);
            fixture.commit(false, sample);
        }
        fixture.assert_publication();
        let measured = phases.snapshot();
        for label in ["agent.choose:plan_commit", "agent.choose:linked_run_commit"] {
            assert_eq!(measured[label]["frame.held"]["count"], SAMPLES);
            assert!(
                measured[label]["publication.native_owners"]["count"]
                    .as_u64()
                    .is_some_and(|count| count >= SAMPLES as u64),
                "missing native preparation samples: {measured}"
            );
        }
        eprintln!(
            "{}",
            json!({"benchmark":"owner_commit_native_snapshot_scale",
                "tasks":TASKS,"runs":RUNS,"agents_per_run":agents_per_run,
                "configured_agents":TASKS+RUNS*agents_per_run,"resident_items_per_conversation":ITEMS,
                "live_sessions":0,"plan_commits":SAMPLES,"linked_run_commits":SAMPLES,
                "phases_ms":measured,"load_before":load_before.trim(),
                "load_after":std::fs::read_to_string("/proc/loadavg").unwrap().trim()})
        );
    }
}

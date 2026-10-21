//! Measure actual owner commits, including the new native snapshot preparation.
use super::*;

const TASKS: usize = 16;
const RUNS: usize = 32;
const ITEMS: usize = 16;
const SAMPLES: usize = 5;
const STAGES: usize = 4;
const COMMENTS: usize = 2;
const NOW: &str = "2026-10-21T12:00:00Z";

struct Fixture {
    _directory: tempfile::TempDir,
    state: Arc<Mutex<AppState>>,
    task: String,
    run: String,
}

impl Fixture {
    fn new(agents_per_run: usize, shared_task_conversation: bool) -> Self {
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
                seed_stages(&app, directory.path(), &id, &mut plan);
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
            if index == 0 {
                record.created_at = NOW.into();
            }
            app.board
                .attention_mut()
                .record_created_if_absent(&id, record.created_at.clone());
            record.stages = stage_progress();
            record.agents = (0..agents_per_run)
                .map(|ordinal| {
                    let mut agent = crate::agent::Agent::new(
                        format!("agent-native-scale-{index}-{ordinal}"),
                        &id,
                        Default::default(),
                        ordinal as u32 + 1,
                        NOW,
                    );
                    if shared_task_conversation && ordinal == 0 {
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

    fn commit(
        &self,
        plan: bool,
        sample: usize,
    ) -> crate::app::snapshot_projection::work_counts::detail_scans::Counts {
        let (_, counts) =
            crate::app::snapshot_projection::work_counts::detail_scans::measure(|| {
                self.commit_measured(plan, sample);
            });
        counts
    }

    fn commit_measured(&self, plan: bool, sample: usize) {
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

    fn assert_publication(&self, enabled: bool) {
        let app = self.state.lock().unwrap();
        let view = app.read_view.load();
        assert_eq!(
            app.plans[&self.task].last_summary,
            Some(format!("plan sample {}", SAMPLES - 1))
        );
        assert!(view.get_owned("agent.list", &self.run).is_some());
        if !enabled {
            assert!(view.get_owned("task.get", &self.task).is_none());
            assert!(view.get_owned("task.stages", &self.task).is_none());
            return;
        }
        let task = view.get_owned("task.get", &self.task).unwrap();
        assert_eq!(task["summary"], format!("plan sample {}", SAMPLES - 1));
        let lineage = task["implementation_lineage"].as_array().unwrap();
        assert_eq!(lineage.len(), RUNS / TASKS);
        assert!(lineage.iter().any(|run| run["run_id"] == self.run));
        assert_eq!(task["docs_available"], true);
        let stages = view.get_owned("task.stages", &self.task).unwrap();
        let rows = stages["stages"].as_array().unwrap();
        assert_eq!(rows.len(), STAGES);
        for stage in rows {
            assert_eq!(stage["open_comments"], COMMENTS);
            assert_eq!(stage["comments"].as_array().unwrap().len(), COMMENTS);
        }
        assert_eq!(rows[0]["execution"], "complete");
        assert_eq!(rows[1]["execution"], "building");
        assert_eq!(rows[2]["execution"], "pending");
        assert!(view.get_owned("agent.list", &self.run).is_some());
    }
}

fn seed_stages(
    app: &AppState,
    root: &Path,
    task: &str,
    plan: &mut crate::orchestrator::ActivePlan,
) {
    let worktree = root.join("native-stage-docs").join(task);
    std::fs::create_dir_all(worktree.join(".build/plan")).unwrap();
    for index in 0..STAGES {
        let id = format!("stage-native-scale-{index}");
        let path = format!(".build/plan/stage-{index}.md");
        let body = format!(
            "# Stage {index}\n\n{}",
            "Implement the retained task stage and verify its output.\n".repeat(20)
        );
        std::fs::write(worktree.join(&path), &body).unwrap();
        plan.stages.push(crate::plan::StageDoc {
            id: id.clone(),
            title: format!("Retained task stage {index}"),
            path: path.clone(),
            summary: format!(
                "Stage {index} preserves committed task behavior and reviewer comments"
            ),
            state: crate::plan::StageDocState::Approved,
        });
        for comment in 0..COMMENTS {
            plan.agents.sole_thread_mut().post_doc_comment(
                task,
                &id,
                &path,
                Some(crate::thread::DocAnchor {
                    line_start: Some(3),
                    line_end: Some(3),
                    snippet: "Implement the retained task stage".into(),
                    ..Default::default()
                }),
                format!(
                    "Stage {index} reviewer observation {comment}: retain this published boundary"
                ),
                NOW,
            );
        }
    }
    app.store
        .as_ref()
        .unwrap()
        .ingest_plan_docs(task, &worktree, &plan.plan_path)
        .unwrap();
}

fn stage_progress() -> Vec<crate::run::StageProgress> {
    let mut complete = crate::run::StageProgress::dispatched("stage-native-scale-0");
    complete.state = crate::run::StageProgressState::Completed;
    complete.start_sha = Some("a".repeat(40));
    complete.built_sha = Some("b".repeat(40));
    complete.completion_sha = Some("b".repeat(40));
    let mut building = crate::run::StageProgress::dispatched("stage-native-scale-1");
    building.start_sha = Some("b".repeat(40));
    vec![complete, building]
}

// The control omits only new native task/projection work and its linked-task
// edges, including same-mode initialization. It is not a checkout of main:
// document observation and unrelated changes remain identical in both modes.
fn measure_scale(
    agents_per_run: usize,
    shared_task_conversation: bool,
    enabled: bool,
) -> Vec<Value> {
    AppState::with_native_task_projection_control(enabled, || {
        let fixture = Fixture::new(agents_per_run, shared_task_conversation);
        // Keep initialization and first-use allocation outside measured commits.
        fixture.commit(true, usize::MAX);
        fixture.commit(false, usize::MAX);
        let clock = Arc::clone(&fixture.state.lock().unwrap().frame_clock);
        let phases = clock.capture_phases();
        let load_before = std::fs::read_to_string("/proc/loadavg").unwrap();
        let mut scans = Vec::new();
        let mut selected = Vec::new();
        for sample in 0..SAMPLES {
            for plan in [true, false] {
                let counts = fixture.commit(plan, sample);
                scans.push(json!({"sample":sample,"owner":if plan {"plan"} else {"linked_run"},
                    "registry_scans":counts.registry_scans,"registry_rows":counts.registry_rows,
                    "previous_bases":counts.previous_bases,"base_renders":counts.base_renders,
                    "variant_renders":counts.variant_renders,"tombstone_keys":counts.tombstone_keys,
                    "indexed_values":counts.indexed_values,"materialized_nodes":counts.materialized_nodes}));
                selected.push(json!([
                    counts.registry_scans,
                    counts.registry_rows,
                    counts.previous_bases,
                    counts.base_renders,
                    counts.variant_renders,
                    counts.tombstone_keys
                ]));
            }
        }
        fixture.assert_publication(enabled);
        let measured = phases.snapshot();
        for label in ["agent.choose:plan_commit", "agent.choose:linked_run_commit"] {
            assert_eq!(measured[label]["frame.held"]["count"], SAMPLES);
            if enabled {
                assert!(
                    measured[label]["publication.native_owners"]["count"]
                        .as_u64()
                        .is_some_and(|count| count >= SAMPLES as u64),
                    "missing native preparation samples: {measured}"
                );
            } else {
                assert!(measured[label].get("publication.native_owners").is_none());
            }
        }
        eprintln!(
            "{}",
            json!({"benchmark":"owner_commit_native_snapshot_scale",
            "native_task_projection":enabled,"shared_task_conversation":shared_task_conversation,
            "control_excludes":["new task detail/stage/implementation chunks", "linked-task edges", "project columns publication"],
            "tasks":TASKS,"runs":RUNS,"agents_per_run":agents_per_run,
            "configured_agents":TASKS+RUNS*agents_per_run,
            "ordinary_resident_messages_per_conversation":ITEMS,
            "task_resident_items":ITEMS+STAGES*COMMENTS,"private_run_resident_items":ITEMS,
            "live_sessions":0,"stages_per_task":STAGES,"comments_per_stage":COMMENTS,
            "plan_commits":SAMPLES,"linked_run_commits":SAMPLES,"detail_scans":scans,
            "phases_ms":measured,"load_before":load_before.trim(),
            "load_after":std::fs::read_to_string("/proc/loadavg").unwrap().trim()})
        );
        selected
    })
}

#[test]
fn owner_commit_native_snapshot_scale() {
    for shared_task_conversation in [true, false] {
        for (index, agents_per_run) in [1, 4, 16, 32].into_iter().enumerate() {
            // Alternate order to make a systematic warm-host bias visible.
            let first_enabled = index % 2 == 0;
            let first = measure_scale(agents_per_run, shared_task_conversation, first_enabled);
            let second = measure_scale(agents_per_run, shared_task_conversation, !first_enabled);
            eprintln!(
                "{}",
                json!({"benchmark":"owner_commit_native_snapshot_scope_comparison",
                "shared_task_conversation":shared_task_conversation,"agents_per_run":agents_per_run,
                "same_workspace_work":first==second})
            );
            if shared_task_conversation {
                assert_eq!(first, second,
                    "native task projection changed selected workspace work at {agents_per_run} agents/run");
            }
        }
    }
}

fn complete_independent_stage(
    enabled: bool,
) -> crate::app::snapshot_projection::work_counts::detail_scans::Counts {
    AppState::with_native_task_projection_control(enabled, || {
        let fixture = Fixture::new(4, false);
        let clock = Arc::clone(&fixture.state.lock().unwrap().frame_clock);
        let (_, counts) =
            crate::app::snapshot_projection::work_counts::detail_scans::measure(|| {
                let timer = clock.frame("agent.choose");
                let mut app = timer.lock(&fixture.state);
                let mut active = app.take_run(&fixture.run).unwrap();
                active.run.state = RunState::Review;
                active.stages[1].state = crate::run::StageProgressState::Completed;
                active.stages[1].built_sha = Some("c".repeat(40));
                active.stages[1].completion_sha = Some("c".repeat(40));
                app.finish_run_mutation(fixture.run.clone(), active)
                    .unwrap();
            });
        if enabled {
            let app = fixture.state.lock().unwrap();
            let view = app.read_view.load();
            let stages = view.get_owned("task.stages", &fixture.task).unwrap();
            assert_eq!(stages["implementation_id"], fixture.run);
            assert_eq!(stages["stages"][1]["execution"], "complete");
            assert_eq!(stages["stages"][1]["completion_sha"], "c".repeat(40));
            let task = view.get_owned("task.get", &fixture.task).unwrap();
            assert_eq!(task["current_implementation_id"], fixture.run);
            assert_eq!(task["current_implementation"]["state"], "review");
            assert!(
                task["execution_context"].is_null(),
                "private run agent cannot inherit Task conversation"
            );
        }
        counts
    })
}

#[test]
fn independent_task_dependency_updates_native_reads_without_widening_workspace_work() {
    let control = complete_independent_stage(false);
    let enabled = complete_independent_stage(true);
    eprintln!(
        "independent linked Task selected workspace work: control={control:?} enabled={enabled:?}"
    );
    assert_eq!(
        enabled.base_renders, control.base_renders,
        "linked Task metadata widened workspace base rendering"
    );
    assert_eq!(
        enabled.variant_renders, control.variant_renders,
        "linked Task metadata widened named workspace rendering"
    );
}

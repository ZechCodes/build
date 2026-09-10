//! State-independent support for the scripted QA agent.

use crate::mcp::{CommentResolution, DoneOutputs, DonePhase, DoneReport, DoneStatus};
use crate::orchestrator::{ActivePlan, ActiveRun};
use crate::plan::{StageDoc, StageManifestEntry};
use crate::run::{RunState, StageProgressState, ValidationReport};
use crate::templates::STAGES_MANIFEST_PATH;
/// Write a file under `dir`, creating parent dirs — the QA agent's file writer.
pub(super) fn write_in_dir(dir: &std::path::Path, rel: &str, contents: &str) -> Result<(), String> {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(path, contents).map_err(|e| e.to_string())
}

use crate::app::{append_plan_stage_announcements, err, AppState};

impl AppState {
    // ---- the scripted QA agent ------------------------------------------------

    /// Simulate a plan session: write the two-stage plan docs + manifest into
    /// the issue's scratch docs dir and report `done(phase=plan)`, so the
    /// orchestrator ingests them into the canonical store exactly as a real
    /// harness would over MCP.
    pub(in crate::app) fn qa_simulate_plan(
        &self,
        project_id: &str,
        active: &mut ActivePlan,
    ) -> Result<(), String> {
        let previous_stage_ids: Vec<String> =
            active.stages.iter().map(|stage| stage.id.clone()).collect();
        let docs_dir = active
            .workspace
            .as_ref()
            .ok_or("QA plan: no planning workspace")?
            .docs_dir
            .clone();
        let goal = active.plan.goal.clone();
        write_in_dir(
            &docs_dir,
            ".build/plan/01-first-half.md",
            &format!("# Stage: First half\n\n1. Implement the first half of: {goal}\n"),
        )?;
        write_in_dir(
            &docs_dir,
            ".build/plan/02-second-half.md",
            &format!("# Stage: Second half\n\n1. Implement the second half of: {goal}\n"),
        )?;
        let stages = vec![
            StageManifestEntry {
                id: "first-half".to_string(),
                title: "First half".to_string(),
                path: ".build/plan/01-first-half.md".to_string(),
                summary: "First half.".to_string(),
            },
            StageManifestEntry {
                id: "second-half".to_string(),
                title: "Second half".to_string(),
                path: ".build/plan/02-second-half.md".to_string(),
                summary: "Second half.".to_string(),
            },
        ];
        let manifest = serde_json::to_string_pretty(&stages).map_err(|e| e.to_string())?;
        write_in_dir(&docs_dir, STAGES_MANIFEST_PATH, &manifest)?;
        let store = self.require_store()?;
        self.orch_for(project_id)?
            .on_plan_done(
                active,
                store,
                DoneReport {
                    phase: DonePhase::Plan,
                    status: DoneStatus::Completed,
                    summary: format!("Planned: {goal}"),
                    outputs: DoneOutputs {
                        plan_path: Some(STAGES_MANIFEST_PATH.to_string()),
                        stages: Some(stages),
                        ..DoneOutputs::default()
                    },
                },
            )
            .map_err(err)?;
        let new_stages: Vec<(usize, StageDoc)> = active
            .stages
            .iter()
            .enumerate()
            .filter(|(_, stage)| !previous_stage_ids.contains(&stage.id))
            .map(|(index, stage)| (index, stage.clone()))
            .collect();
        let plan_id = active.plan.id.0.clone();
        append_plan_stage_announcements(active.agents.sole_thread_mut(), &plan_id, &new_stages);
        Ok(())
    }

    /// Simulate a per-stage plan-revision session: rewrite the stage doc in the
    /// scratch docs dir and resolve every open comment on the revised stage.
    pub(in crate::app) fn qa_simulate_plan_stage_revise(
        &self,
        project_id: &str,
        active: &mut ActivePlan,
    ) -> Result<(), String> {
        let stage_id = active
            .revising_stage_id
            .clone()
            .ok_or("QA plan revise: no stage revision in flight")?;
        let docs_dir = active
            .workspace
            .as_ref()
            .ok_or("QA plan revise: no planning workspace")?
            .docs_dir
            .clone();
        let index = active.stage_doc_index(&stage_id)?;
        let stage_path = active.stages[index].path.clone();
        let mut contents = std::fs::read_to_string(docs_dir.join(&stage_path))
            .map_err(|e| format!("QA plan revise: could not read stage doc: {e}"))?;
        contents.push_str("\n(revised)\n");
        write_in_dir(&docs_dir, &stage_path, &contents)?;
        let resolutions: Vec<CommentResolution> = active
            .open_comments_for(&stage_id)
            .into_iter()
            .map(|c| CommentResolution {
                comment_id: c.id.clone(),
                response: "QA: addressed.".to_string(),
            })
            .collect();
        let store = self.require_store()?;
        self.orch_for(project_id)?
            .on_plan_done(
                active,
                store,
                DoneReport {
                    phase: DonePhase::Revise,
                    status: DoneStatus::Completed,
                    summary: format!("Revised stage {stage_id}"),
                    outputs: DoneOutputs {
                        comment_resolutions: Some(resolutions),
                        ..DoneOutputs::default()
                    },
                },
            )
            .map_err(err)
    }

    /// Simulate a mid-run stage-revision agent: rewrite the stage doc in the
    /// RUN's worktree, resolve the plan's open comments, and route the
    /// `done(revise)` through the cross-entity consume seam.
    pub(in crate::app) fn qa_simulate_run_stage_revise(
        &self,
        project_id: &str,
        active: &mut ActiveRun,
        plan: &mut ActivePlan,
    ) -> Result<(), String> {
        let stage_id = active
            .revising_stage_id
            .clone()
            .ok_or("QA run revise: no stage revision in flight")?;
        let index = plan.stage_doc_index(&stage_id)?;
        let stage_path = plan.stages[index].path.clone();
        let worktree = active.worktree.path.clone();
        let mut contents = std::fs::read_to_string(worktree.join(&stage_path))
            .map_err(|e| format!("QA run revise: could not read stage doc: {e}"))?;
        contents.push_str("\n(revised mid-run)\n");
        write_in_dir(&worktree, &stage_path, &contents)?;
        let resolutions: Vec<CommentResolution> = plan
            .open_comments_for(&stage_id)
            .into_iter()
            .map(|c| CommentResolution {
                comment_id: c.id.clone(),
                response: "QA: addressed.".to_string(),
            })
            .collect();
        let store = self.require_store()?;
        self.orch_for(project_id)?
            .consume_run_stage_revision(
                active,
                plan,
                store,
                &DoneReport {
                    phase: DonePhase::Revise,
                    status: DoneStatus::Completed,
                    summary: format!("Revised stage {stage_id} mid-run"),
                    outputs: DoneOutputs {
                        comment_resolutions: Some(resolutions),
                        ..DoneOutputs::default()
                    },
                },
            )
            .map_err(err)
    }

    /// Simulate a run's build agent: write the result file and report
    /// `done(phase=build)`.
    pub(in crate::app) fn qa_simulate_build(
        &self,
        project_id: &str,
        active: &mut ActiveRun,
        plan_docs: &[StageDoc],
    ) -> Result<(), String> {
        let content = format!("Implemented: {}\n", active.run.goal);
        write_in_dir(&active.worktree.path, "result.txt", &content)?;
        self.orch_for(project_id)?
            .on_run_done(
                active,
                plan_docs,
                DoneReport {
                    phase: DonePhase::Build,
                    status: DoneStatus::Completed,
                    summary: format!("Built: {}", active.run.goal),
                    outputs: DoneOutputs::default(),
                },
            )
            .map_err(err)?;
        Ok(())
    }

    /// Simulate one stage's build session and — since that hands off to a
    /// validation session — the validation too, so one call lands the stage on
    /// a verdict exactly as two real `done` reports would.
    /// The build→validate hand-off turn each `on_run_done` returns is dropped
    /// on purpose here: the scripted agent plays BOTH sides, so it validates
    /// the stage itself in the next arm rather than asking a harness to. That
    /// means NO test driven by this simulator exercises the hand-off delivery —
    /// `a_built_stage_queues_its_validation_turn_for_the_worktrees_agent` and
    /// `a_done_over_the_socket_delivers_the_validation_turn_to_the_same_agent`
    /// switch the simulator off precisely so the real path is covered.
    pub(in crate::app) fn qa_simulate_stage_build(
        &self,
        project_id: &str,
        active: &mut ActiveRun,
        plan_docs: &[StageDoc],
    ) -> Result<(), String> {
        let stage_id = active
            .current_stage_id
            .clone()
            .ok_or("QA stage build: no current stage")?;
        if active.stage_progress(&stage_id).map(|p| p.state) == Some(StageProgressState::Building) {
            let content = format!("Implemented stage {stage_id}: {}\n", active.run.goal);
            write_in_dir(
                &active.worktree.path,
                &format!("result-{stage_id}.txt"),
                &content,
            )?;
            self.orch_for(project_id)?
                .on_run_done(
                    active,
                    plan_docs,
                    DoneReport {
                        phase: DonePhase::Build,
                        status: DoneStatus::Completed,
                        summary: format!("Built stage {stage_id}"),
                        outputs: DoneOutputs::default(),
                    },
                )
                .map_err(err)?;
        }
        if active.stage_progress(&stage_id).map(|p| p.state) == Some(StageProgressState::Validating)
        {
            self.orch_for(project_id)?
                .on_run_done(
                    active,
                    plan_docs,
                    DoneReport {
                        phase: DonePhase::Validate,
                        status: DoneStatus::Completed,
                        summary: format!("Validated stage {stage_id}"),
                        outputs: DoneOutputs {
                            validation: Some(ValidationReport {
                                passed: true,
                                findings: "QA validation: pass.".to_string(),
                                notes_for_next_stage: "QA notes for the next stage.".to_string(),
                            }),
                            ..DoneOutputs::default()
                        },
                    },
                )
                .map_err(err)?;
        }
        Ok(())
    }

    /// Drive the QA harness while a run is `Building`: a fresh stage runs its
    /// build+validate; a post-review change or a single-doc-plan build runs
    /// the plain build. Bounded so a non-converging chain fails loudly.
    pub(in crate::app) fn qa_drive_run(
        &self,
        project_id: &str,
        active: &mut ActiveRun,
        plan_docs: &[StageDoc],
    ) -> Result<(), String> {
        if !self.qa_agent {
            return Ok(());
        }
        let max_hops = plan_docs.len() + 2;
        for _ in 0..max_hops {
            if active.run.state != RunState::Building {
                return Ok(());
            }
            let mid_stage = active
                .current_stage_id
                .as_ref()
                .and_then(|sid| active.stage_progress(sid))
                .map(|p| {
                    matches!(
                        p.state,
                        StageProgressState::Building | StageProgressState::Validating
                    )
                })
                .unwrap_or(false);
            if !plan_docs.is_empty() && mid_stage {
                self.qa_simulate_stage_build(project_id, active, plan_docs)?;
            } else {
                self.qa_simulate_build(project_id, active, plan_docs)?;
            }
        }
        Err("QA run did not converge".to_string())
    }
}

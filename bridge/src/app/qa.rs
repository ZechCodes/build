//! State-independent support for the scripted QA agent.

use crate::mcp::{DoneReport, DoneStatus};
use crate::orchestrator::{ActivePlan, ActiveRun};
use crate::plan::{StageDoc, StageManifestEntry};
use crate::run::{RunState, StageProgressState};
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
    /// the task's scratch docs dir and report Complete, so the orchestrator
    /// reads the manifest and ingests the docs exactly as it would for a real
    /// harness.
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
                DoneReport::new(DoneStatus::Completed, format!("Planned: {goal}")),
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

    /// Simulate a run's build agent: write the result file and report
    /// Complete.
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
                DoneReport::new(DoneStatus::Completed, format!("Built: {}", active.run.goal)),
            )
            .map_err(err)?;
        Ok(())
    }

    /// Simulate one stage's build session: write its result file and report
    /// Complete, which completes the stage.
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
                DoneReport::new(DoneStatus::Completed, format!("Built stage {stage_id}")),
            )
            .map_err(err)?;
        Ok(())
    }

    /// Drive the QA harness while a run is `Building`: a fresh stage runs its
    /// build; a post-review change or a single-doc-plan build runs the plain
    /// build. Bounded so a non-converging chain fails loudly.
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
                .map(|p| p.state == StageProgressState::Building)
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

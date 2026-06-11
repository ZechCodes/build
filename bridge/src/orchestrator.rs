//! The task-spine: where lifecycle, worktrees, PTY sessions, `done` reports, and
//! the diff come together.
//!
//! The orchestrator owns project-level configuration (the repo, where worktrees
//! go, the harness adapter, the prompt templates) and drives one [`ActiveTask`]
//! at a time through the lifecycle. The caller owns each `ActiveTask` and hands it
//! back by `&mut` for each transition, so the orchestrator never hides state.
//!
//! The two pipes from the scope are both here: Build → agent is `write_prompt`
//! into the warm PTY; agent → Build is [`on_done`](Orchestrator::on_done), the
//! typed event the MCP server forwards.

use std::path::{Path, PathBuf};
use std::process::Command;

use portable_pty::PtySize;

use crate::diff::{diff_against_base, DiffError, WorktreeDiff};
use crate::mcp::{DonePhase, DoneReport, DoneStatus};
use crate::pty::{HarnessSpec, PtyError, PtySession};
use crate::task::{IllegalTransition, Task, TaskEvent, TaskId, TaskKind, TaskState};
use crate::templates::{self, Templates, Vars, DEFAULT_PLAN_PATH};
use crate::worktree::{slugify, Worktree, WorktreeError, WorktreeManager};

#[derive(Debug, thiserror::Error)]
pub enum OrchestratorError {
    #[error(transparent)]
    Worktree(#[from] WorktreeError),
    #[error(transparent)]
    Pty(#[from] PtyError),
    #[error(transparent)]
    Diff(#[from] DiffError),
    #[error(transparent)]
    Transition(#[from] IllegalTransition),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("git command failed: {0}")]
    Git(String),
}

/// One task in flight: its lifecycle state, its worktree, and its warm session.
pub struct ActiveTask {
    pub task: Task,
    pub worktree: Worktree,
    /// Where the plan lives — convention by default, updated from `done` outputs.
    pub plan_path: String,
    /// The most recent `done` summary, surfaced on cards.
    pub last_summary: Option<String>,
    /// The warm PTY session for the current phase (None before dispatch/after end).
    session: Option<PtySession>,
}

impl ActiveTask {
    /// Subscribe to the live terminal stream, if a session is warm.
    pub fn subscribe(&self) -> Option<tokio::sync::broadcast::Receiver<Vec<u8>>> {
        self.session.as_ref().map(|s| s.subscribe())
    }

    /// Write raw bytes (attached-terminal keystrokes) to the warm session.
    pub fn write_input(&self, bytes: &[u8]) -> Result<(), OrchestratorError> {
        if let Some(session) = &self.session {
            session.write_input(bytes)?;
        }
        Ok(())
    }
}

/// Owns project configuration and drives tasks through the lifecycle.
pub struct Orchestrator {
    repo_path: PathBuf,
    worktrees: WorktreeManager,
    harness: HarnessSpec,
    templates: Templates,
    pty_size: PtySize,
}

impl Orchestrator {
    pub fn new(
        repo_path: impl Into<PathBuf>,
        worktrees_root: impl Into<PathBuf>,
        harness: HarnessSpec,
        templates: Templates,
    ) -> Self {
        let repo_path = repo_path.into();
        let worktrees = WorktreeManager::new(repo_path.clone(), worktrees_root);
        Orchestrator {
            repo_path,
            worktrees,
            harness,
            templates,
            pty_size: PtySize {
                rows: 40,
                cols: 120,
                pixel_width: 0,
                pixel_height: 0,
            },
        }
    }

    /// Dispatch a goal: create the worktree, scaffold `.build/`, transition out of
    /// `Created`, and spawn the first phase session with its prompt. A standard
    /// task starts planning; a quick task goes straight to building.
    pub fn dispatch(
        &self,
        id: TaskId,
        goal: impl Into<String>,
        kind: TaskKind,
        base_branch: &str,
    ) -> Result<ActiveTask, OrchestratorError> {
        let goal = goal.into();
        let slug = slugify(&goal);
        let worktree = self.worktrees.create(&slug, base_branch)?;
        self.scaffold_build_dir(&worktree, &id)?;

        let mut task = Task::new(id, goal.clone(), kind);
        task.apply(TaskEvent::Dispatch)?;

        let mut active = ActiveTask {
            task,
            worktree,
            plan_path: DEFAULT_PLAN_PATH.to_string(),
            last_summary: None,
            session: None,
        };

        // Standard → Planning (plan prompt); Quick → Building (build prompt).
        let prompt = match active.task.state {
            TaskState::Planning => self.render(&self.templates.plan, &active, ""),
            TaskState::Building => self.render(&self.templates.build, &active, ""),
            ref other => unreachable!("dispatch left task in {other:?}"),
        };
        active.session = Some(self.spawn(&active.worktree, &prompt)?);
        Ok(active)
    }

    /// Consume an agent's `done` report (the MCP server forwards these), mapping
    /// it to the matching lifecycle event.
    pub fn on_done(
        &self,
        active: &mut ActiveTask,
        report: DoneReport,
    ) -> Result<(), OrchestratorError> {
        if let Some(path) = &report.outputs.plan_path {
            active.plan_path = path.clone();
        }
        active.last_summary = Some(report.summary.clone());

        let event = match (report.phase, report.status) {
            (_, DoneStatus::Blocked) => TaskEvent::Blocked,
            (_, DoneStatus::Failed) => TaskEvent::Failed,
            (DonePhase::Plan, DoneStatus::Completed) => TaskEvent::PlanReady,
            (DonePhase::Build | DonePhase::Revise, DoneStatus::Completed) => TaskEvent::BuildReady,
        };
        active.task.apply(event)?;
        Ok(())
    }

    /// The quiescence timer fired without a `done`: demote to `idle_unreported`.
    pub fn on_idle(&self, active: &mut ActiveTask) -> Result<(), OrchestratorError> {
        active.task.apply(TaskEvent::WentIdle)?;
        Ok(())
    }

    /// Approve the plan and start the build in a **fresh** session — if a cold
    /// agent can't execute the plan, the plan wasn't done.
    pub fn approve_plan(&self, active: &mut ActiveTask) -> Result<(), OrchestratorError> {
        active.task.apply(TaskEvent::ApprovePlan)?;
        self.end_session(active);
        let prompt = self.render(&self.templates.build, active, "");
        active.session = Some(self.spawn(&active.worktree, &prompt)?);
        Ok(())
    }

    /// Submit a batch of plan notes: revise in the still-warm planning session.
    pub fn send_notes(
        &self,
        active: &mut ActiveTask,
        notes: &str,
    ) -> Result<(), OrchestratorError> {
        active.task.apply(TaskEvent::SendNotes)?;
        let prompt = self.render(&self.templates.revise, active, notes);
        self.prompt_warm_session(active, &prompt)?;
        Ok(())
    }

    /// Submit a batch of diff comments: correct in the still-warm build session.
    pub fn request_changes(
        &self,
        active: &mut ActiveTask,
        comments: &str,
    ) -> Result<(), OrchestratorError> {
        active.task.apply(TaskEvent::RequestChanges)?;
        let prompt = self.render(&self.templates.review_changes, active, comments);
        self.prompt_warm_session(active, &prompt)?;
        Ok(())
    }

    /// Reply to a blocked/failed/idle card with a human follow-up; resume the phase.
    pub fn reply(&self, active: &mut ActiveTask, message: &str) -> Result<(), OrchestratorError> {
        active.task.apply(TaskEvent::Reply)?;
        self.prompt_warm_session(active, message)?;
        Ok(())
    }

    /// The worktree's current diff against base — progress fact while building, the
    /// full review surface at the gate.
    pub fn diff(&self, active: &ActiveTask) -> Result<WorktreeDiff, OrchestratorError> {
        Ok(diff_against_base(
            &active.worktree.path,
            &active.worktree.base_branch,
        )?)
    }

    /// Approve the diff and merge: commit any outstanding work on the task branch,
    /// merge it into the base branch, release the session, and remove the worktree.
    pub fn approve_merge(&self, active: &mut ActiveTask) -> Result<(), OrchestratorError> {
        active.task.apply(TaskEvent::ApproveMerge)?;
        self.end_session(active);

        // The agent's work may be uncommitted; commit it on the task branch so the
        // merge carries it. Empty trees are skipped.
        self.commit_all(&active.worktree.path, &active.task.goal)?;
        self.merge_into_base(&active.worktree.branch)?;
        self.worktrees
            .remove(&active.worktree, /* keep_branch */ false)?;
        Ok(())
    }

    /// Abandon: release the session and remove the worktree, keeping the branch.
    pub fn abandon(&self, active: &mut ActiveTask) -> Result<(), OrchestratorError> {
        active.task.apply(TaskEvent::Abandon)?;
        self.end_session(active);
        self.worktrees
            .remove(&active.worktree, /* keep_branch */ true)?;
        Ok(())
    }

    // --- internals -------------------------------------------------------------

    fn render(&self, template: &str, active: &ActiveTask, comments: &str) -> String {
        templates::render(
            template,
            &Vars {
                goal: &active.task.goal,
                plan_path: &active.plan_path,
                comments,
                base_branch: &active.worktree.base_branch,
            },
        )
    }

    fn spawn(&self, worktree: &Worktree, prompt: &str) -> Result<PtySession, OrchestratorError> {
        let session = PtySession::spawn(&self.harness, Some(worktree.path.clone()), self.pty_size)?;
        session.write_prompt(prompt)?;
        Ok(session)
    }

    fn prompt_warm_session(
        &self,
        active: &ActiveTask,
        prompt: &str,
    ) -> Result<(), OrchestratorError> {
        if let Some(session) = &active.session {
            session.write_prompt(prompt)?;
        }
        Ok(())
    }

    fn end_session(&self, active: &mut ActiveTask) {
        if let Some(session) = active.session.take() {
            let _ = session.kill();
        }
    }

    /// Write the per-task MCP config under `.build/` so it never trips plan-scope
    /// enforcement, pointing the harness at this task's `done` server.
    fn scaffold_build_dir(
        &self,
        worktree: &Worktree,
        id: &TaskId,
    ) -> Result<(), OrchestratorError> {
        let build_dir = worktree.path.join(".build");
        std::fs::create_dir_all(&build_dir)?;
        let mcp = serde_json::json!({
            "mcpServers": {
                "build": {
                    "command": "build-bridge",
                    "args": ["mcp", "--task", id.0]
                }
            }
        });
        std::fs::write(
            build_dir.join("mcp.json"),
            serde_json::to_string_pretty(&mcp)?,
        )?;
        Ok(())
    }

    fn commit_all(&self, worktree_path: &Path, goal: &str) -> Result<(), OrchestratorError> {
        self.git(worktree_path, &["add", "-A"])?;
        // Only commit if something is staged.
        let status = self.git(worktree_path, &["status", "--porcelain"])?;
        if !status.trim().is_empty() {
            self.git(worktree_path, &["commit", "-m", &format!("Build: {goal}")])?;
        }
        Ok(())
    }

    fn merge_into_base(&self, branch: &str) -> Result<(), OrchestratorError> {
        self.git(&self.repo_path, &["merge", "--no-edit", branch])?;
        Ok(())
    }

    fn git(&self, dir: &Path, args: &[&str]) -> Result<String, OrchestratorError> {
        let out = Command::new("git").args(args).current_dir(dir).output()?;
        if !out.status.success() {
            return Err(OrchestratorError::Git(format!(
                "git {args:?}: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::DoneOutputs;

    fn init_repo() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        let git = |args: &[&str]| {
            assert!(Command::new("git")
                .args(args)
                .current_dir(&repo)
                .status()
                .unwrap()
                .success());
        };
        git(&["init", "-b", "main"]);
        git(&["config", "user.email", "t@build.ing"]);
        git(&["config", "user.name", "T"]);
        std::fs::write(repo.join("README.md"), "# project\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-m", "initial"]);
        (dir, repo)
    }

    /// A warm "harness" that simply stays alive (it ignores the prompt). The test
    /// plays the agent: it writes files and forwards `done` reports.
    fn warm_harness() -> HarnessSpec {
        HarnessSpec::new("sh").arg("-c").arg("sleep 30")
    }

    fn orchestrator(dir: &tempfile::TempDir, repo: &Path) -> Orchestrator {
        Orchestrator::new(
            repo.to_path_buf(),
            dir.path().join("worktrees"),
            warm_harness(),
            Templates::default(),
        )
    }

    fn done(phase: DonePhase, status: DoneStatus, plan_path: Option<&str>) -> DoneReport {
        DoneReport {
            phase,
            status,
            summary: "summary".into(),
            outputs: DoneOutputs {
                plan_path: plan_path.map(String::from),
            },
        }
    }

    #[tokio::test]
    async fn full_spine_plan_build_merge() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);

        // Dispatch → Planning, worktree + MCP config scaffolded.
        let mut t = orch
            .dispatch(
                TaskId::new("t1"),
                "Add a greeting",
                TaskKind::Standard,
                "main",
            )
            .unwrap();
        assert_eq!(t.task.state, TaskState::Planning);
        assert!(t.worktree.path.join(".build/mcp.json").exists());
        assert!(t.subscribe().is_some(), "a warm session streams output");

        // The agent writes the plan, then reports done → PlanReview.
        std::fs::write(
            t.worktree.path.join(".build/plan.md"),
            "# Plan\n1. add greeting.txt\n",
        )
        .unwrap();
        orch.on_done(
            &mut t,
            done(
                DonePhase::Plan,
                DoneStatus::Completed,
                Some(".build/plan.md"),
            ),
        )
        .unwrap();
        assert_eq!(t.task.state, TaskState::PlanReview);
        assert_eq!(t.plan_path, ".build/plan.md");

        // Plan-phase enforcement: only `.build/` touched so far.
        assert!(!orch.diff(&t).unwrap().touched_outside_plan_scope());

        // Approve → fresh build session.
        orch.approve_plan(&mut t).unwrap();
        assert_eq!(t.task.state, TaskState::Building);

        // The agent writes code, then reports done → Review.
        std::fs::write(t.worktree.path.join("greeting.txt"), "hello\n").unwrap();
        orch.on_done(&mut t, done(DonePhase::Build, DoneStatus::Completed, None))
            .unwrap();
        assert_eq!(t.task.state, TaskState::Review);

        // The full diff at the gate shows the code.
        let review_diff = orch.diff(&t).unwrap();
        assert!(review_diff.files().iter().any(|f| f.path == "greeting.txt"));

        // Approve & merge → Merged, worktree gone, base branch has the file.
        orch.approve_merge(&mut t).unwrap();
        assert_eq!(t.task.state, TaskState::Merged);
        assert!(!t.worktree.path.exists(), "worktree removed");
        assert!(
            repo.join("greeting.txt").exists(),
            "merged into base working tree"
        );
        // The plan is kept through merge (intent as infrastructure).
        assert!(
            repo.join(".build/plan.md").exists(),
            "plan kept through merge"
        );
    }

    #[tokio::test]
    async fn quick_task_skips_planning() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);

        let mut t = orch
            .dispatch(TaskId::new("q1"), "fix typo", TaskKind::Quick, "main")
            .unwrap();
        assert_eq!(t.task.state, TaskState::Building, "no plan phase");

        std::fs::write(t.worktree.path.join("fix.txt"), "fixed\n").unwrap();
        orch.on_done(&mut t, done(DonePhase::Build, DoneStatus::Completed, None))
            .unwrap();
        assert_eq!(t.task.state, TaskState::Review);

        orch.approve_merge(&mut t).unwrap();
        assert_eq!(t.task.state, TaskState::Merged);
        assert!(repo.join("fix.txt").exists());
    }

    #[tokio::test]
    async fn blocked_then_reply_resumes_building() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let mut t = orch
            .dispatch(TaskId::new("b1"), "do work", TaskKind::Quick, "main")
            .unwrap();

        orch.on_done(&mut t, done(DonePhase::Build, DoneStatus::Blocked, None))
            .unwrap();
        assert!(matches!(t.task.state, TaskState::Blocked(_)));
        assert_eq!(t.last_summary.as_deref(), Some("summary"));

        orch.reply(&mut t, "use the staging credentials").unwrap();
        assert_eq!(t.task.state, TaskState::Building);
    }

    #[tokio::test]
    async fn abandon_removes_worktree_keeps_branch() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let mut t = orch
            .dispatch(TaskId::new("a1"), "scrap this", TaskKind::Standard, "main")
            .unwrap();
        let branch = t.worktree.branch.clone();

        orch.abandon(&mut t).unwrap();
        assert_eq!(t.task.state, TaskState::Abandoned);
        assert!(!t.worktree.path.exists());

        let r = git2::Repository::open(&repo).unwrap();
        assert!(
            r.find_branch(&branch, git2::BranchType::Local).is_ok(),
            "branch kept after abandon"
        );
    }
}

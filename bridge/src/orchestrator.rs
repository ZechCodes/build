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
    /// Reattach a task recovered from the durable store after a daemon restart:
    /// the worktree survived on disk, the PTY session did not. The caller (boot
    /// recovery) has already moved a working state to `Interrupted`.
    pub fn reattach(
        task: Task,
        worktree: Worktree,
        plan_path: String,
        last_summary: Option<String>,
    ) -> Self {
        ActiveTask {
            task,
            worktree,
            plan_path,
            last_summary,
            session: None,
        }
    }

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

    /// Whether the phase's harness process has exited (crashed or finished without
    /// a `done`). `false` when no session is live (nothing to watch).
    pub fn harness_exited(&self) -> bool {
        self.session.as_ref().is_some_and(PtySession::has_exited)
    }

    /// How long the phase's PTY has been silent, if a session is live — the
    /// quiescence signal that demotes to `idle_unreported` when no `done` arrives.
    pub fn harness_idle_for(&self) -> Option<std::time::Duration> {
        self.session.as_ref().map(PtySession::idle_for)
    }

    /// The harness's OS process id, if a session is live and running.
    pub fn harness_pid(&self) -> Option<u32> {
        self.session.as_ref().and_then(PtySession::pid)
    }
}

/// How the orchestrator launches an agent for a phase.
#[derive(Clone)]
pub enum Agent {
    /// A warm interactive session: spawn the binary, then write the prompt to its
    /// PTY. Supports in-session revision rounds (the QA harness uses this).
    Warm(HarnessSpec),
    /// One-shot: build the full spawn command from the rendered prompt (e.g.
    /// `claude -p "<prompt>"`). The agent runs, does the work, reports `done`, and
    /// exits — no warm session.
    OneShot(std::sync::Arc<dyn Fn(&str) -> HarnessSpec + Send + Sync>),
}

/// Owns project configuration and drives tasks through the lifecycle.
pub struct Orchestrator {
    repo_path: PathBuf,
    worktrees: WorktreeManager,
    agent: Agent,
    templates: Templates,
    pty_size: PtySize,
}

impl Orchestrator {
    pub fn new(
        repo_path: impl Into<PathBuf>,
        worktrees_root: impl Into<PathBuf>,
        agent: Agent,
        templates: Templates,
    ) -> Self {
        let repo_path = repo_path.into();
        let worktrees = WorktreeManager::new(repo_path.clone(), worktrees_root);
        Orchestrator {
            repo_path,
            worktrees,
            agent,
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

    /// Submit a batch of plan notes: re-plan against them in a **fresh** session.
    /// The existing plan file is the agent's starting point (the revise template
    /// points at it), so a cold agent can revise it — same discipline as
    /// `approve_plan` starting the build cold.
    pub fn send_notes(
        &self,
        active: &mut ActiveTask,
        notes: &str,
    ) -> Result<(), OrchestratorError> {
        active.task.apply(TaskEvent::SendNotes)?;
        self.end_session(active);
        let prompt = self.render(&self.templates.revise, active, notes);
        active.session = Some(self.spawn(&active.worktree, &prompt)?);
        Ok(())
    }

    /// Submit a batch of diff comments: address them in a **fresh** build session.
    /// Valid both from `review` (agent done) and `building` (agent still running) —
    /// any running session is ended first, so a change request redirects the build
    /// at any time. The agent's in-progress work stays in the worktree as the
    /// starting point.
    pub fn request_changes(
        &self,
        active: &mut ActiveTask,
        comments: &str,
    ) -> Result<(), OrchestratorError> {
        active.task.apply(TaskEvent::RequestChanges)?;
        self.end_session(active);
        let prompt = self.render(&self.templates.review_changes, active, comments);
        active.session = Some(self.spawn(&active.worktree, &prompt)?);
        Ok(())
    }

    /// Reply to a blocked/failed/idle card with a human follow-up; resume the phase.
    pub fn reply(&self, active: &mut ActiveTask, message: &str) -> Result<(), OrchestratorError> {
        active.task.apply(TaskEvent::Reply)?;
        self.prompt_warm_session(active, message)?;
        Ok(())
    }

    /// Re-dispatch an interrupted phase in a **fresh** session. The daemon that
    /// spawned the original session died; the worktree (the agent's real state)
    /// is the starting point, exactly like `approve_plan` starting a cold build.
    pub fn resume(&self, active: &mut ActiveTask) -> Result<(), OrchestratorError> {
        active.task.apply(TaskEvent::Reply)?;
        self.end_session(active);
        let prompt = match active.task.state {
            TaskState::Planning => self.render(&self.templates.plan, active, ""),
            TaskState::Building => self.render(&self.templates.build, active, ""),
            ref other => unreachable!("reply left task in {other:?}"),
        };
        active.session = Some(self.spawn(&active.worktree, &prompt)?);
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
        self.merge_into_base(&active.worktree.branch, &active.worktree.base_branch)?;
        self.worktrees
            .remove(&active.worktree, /* keep_branch */ false)?;
        Ok(())
    }

    /// Commit any outstanding work on the task branch (the implicit commit step
    /// every finish action shares). Keeps the worktree; no lifecycle change.
    pub fn commit(&self, active: &ActiveTask) -> Result<(), OrchestratorError> {
        self.commit_all(&active.worktree.path, &active.task.goal)
    }

    /// Commit, then push the task branch to its `origin`. Keeps the worktree, so
    /// the agent can keep working / the user can open a PR. Errors if there is no
    /// push destination configured.
    pub fn push(&self, active: &ActiveTask) -> Result<(), OrchestratorError> {
        self.commit_all(&active.worktree.path, &active.task.goal)?;
        self.git(
            &active.worktree.path,
            &["push", "-u", "origin", &active.worktree.branch],
        )?;
        Ok(())
    }

    /// Approve & merge (as [`approve_merge`](Self::approve_merge)) and then push
    /// the updated base branch to `origin`.
    pub fn merge_and_push(&self, active: &mut ActiveTask) -> Result<(), OrchestratorError> {
        let base = active.worktree.base_branch.clone();
        self.approve_merge(active)?;
        self.git(&self.repo_path, &["push", "origin", &base])?;
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
        let session = match &self.agent {
            Agent::Warm(spec) => {
                let s = PtySession::spawn(spec, Some(worktree.path.clone()), self.pty_size)?;
                s.write_prompt(prompt)?;
                s
            }
            Agent::OneShot(build) => {
                // The prompt is baked into the command (e.g. `claude -p`); nothing
                // is written to stdin.
                let spec = build(prompt);
                PtySession::spawn(&spec, Some(worktree.path.clone()), self.pty_size)?
            }
        };
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
            // Kill AND reap: kill alone leaves a zombie per phase transition,
            // which over a long-lived daemon exhausts the process table.
            session.kill_and_reap();
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
        // Absolute path to this binary so the harness can spawn it regardless of PATH.
        let exe = std::env::current_exe()
            .ok()
            .and_then(|p| p.to_str().map(String::from))
            .unwrap_or_else(|| "build-bridge".to_string());
        let mcp = serde_json::json!({
            "mcpServers": {
                "build": {
                    "command": exe,
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

    /// Merge the task branch into `base_branch` via the primary checkout. The
    /// primary repo is the user's live checkout, so first verify it actually has
    /// the base branch checked out — merging into whatever happens to be at HEAD
    /// would land the task on the wrong branch (and a later push of the base
    /// branch would silently publish nothing).
    fn merge_into_base(&self, branch: &str, base_branch: &str) -> Result<(), OrchestratorError> {
        let head = self
            .git(&self.repo_path, &["symbolic-ref", "--short", "HEAD"])?
            .trim()
            .to_string();
        if head != base_branch {
            return Err(OrchestratorError::Git(format!(
                "primary checkout is on {head:?}, not the base branch {base_branch:?} — \
                 check out {base_branch:?} (or commit/stash your work) and approve again"
            )));
        }
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
            Agent::Warm(warm_harness()),
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
    async fn send_notes_revises_then_returns_to_plan_review() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let mut t = orch
            .dispatch(
                TaskId::new("p1"),
                "add greeting",
                TaskKind::Standard,
                "main",
            )
            .unwrap();

        // First plan → PlanReview.
        std::fs::write(t.worktree.path.join(".build/plan.md"), "# Plan v1\n").unwrap();
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

        // Request updates → back to Planning in a fresh revise session.
        orch.send_notes(&mut t, "tighten step 2 and add error handling")
            .unwrap();
        assert_eq!(t.task.state, TaskState::Planning);
        assert!(t.subscribe().is_some(), "a fresh revise session is warm");

        // The agent revises the plan and reports done again → PlanReview.
        std::fs::write(t.worktree.path.join(".build/plan.md"), "# Plan v2\n").unwrap();
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
    }

    #[tokio::test]
    async fn request_changes_respawns_build_from_review_and_while_building() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let mut t = orch
            .dispatch(TaskId::new("c1"), "do work", TaskKind::Quick, "main")
            .unwrap();
        assert_eq!(t.task.state, TaskState::Building);

        // While the agent is still building, a change request redirects it: the task
        // stays Building with a fresh session.
        orch.request_changes(&mut t, "use the repository pattern")
            .unwrap();
        assert_eq!(t.task.state, TaskState::Building);
        assert!(t.subscribe().is_some(), "a fresh session is warm");

        // The agent finishes → Review; a change request from Review re-spawns build.
        std::fs::write(t.worktree.path.join("out.txt"), "v1\n").unwrap();
        orch.on_done(&mut t, done(DonePhase::Build, DoneStatus::Completed, None))
            .unwrap();
        assert_eq!(t.task.state, TaskState::Review);
        orch.request_changes(&mut t, "rename out.txt to result.txt")
            .unwrap();
        assert_eq!(t.task.state, TaskState::Building);
        // The in-progress work is still in the worktree for the new session.
        assert!(t.worktree.path.join("out.txt").exists());
    }

    #[tokio::test]
    async fn commit_keeps_worktree_push_and_merge_push_reach_origin() {
        let (dir, repo) = init_repo();
        // A bare origin, wired as the repo's remote (worktrees share it).
        let origin = dir.path().join("origin.git");
        assert!(Command::new("git")
            .args(["init", "--bare", "-b", "main", origin.to_str().unwrap()])
            .status()
            .unwrap()
            .success());
        assert!(Command::new("git")
            .args([
                "-C",
                repo.to_str().unwrap(),
                "remote",
                "add",
                "origin",
                origin.to_str().unwrap(),
            ])
            .status()
            .unwrap()
            .success());
        let git_origin = |args: &[&str]| {
            let out = Command::new("git")
                .arg("--git-dir")
                .arg(&origin)
                .args(args)
                .output()
                .unwrap();
            (
                out.status.success(),
                String::from_utf8_lossy(&out.stdout).into_owned(),
            )
        };

        let orch = orchestrator(&dir, &repo);
        let mut t = orch
            .dispatch(TaskId::new("g1"), "do work", TaskKind::Quick, "main")
            .unwrap();
        std::fs::write(t.worktree.path.join("f.txt"), "hi\n").unwrap();
        orch.on_done(&mut t, done(DonePhase::Build, DoneStatus::Completed, None))
            .unwrap();
        assert_eq!(t.task.state, TaskState::Review);

        // Commit: keeps the worktree and the Review state.
        orch.commit(&t).unwrap();
        assert!(t.worktree.path.exists());
        assert_eq!(t.task.state, TaskState::Review);

        // Push: the feature branch lands in origin; worktree still there.
        orch.push(&t).unwrap();
        assert!(
            git_origin(&["rev-parse", &t.worktree.branch]).0,
            "feature branch pushed"
        );
        assert!(t.worktree.path.exists());

        // Merge + push: base updated in origin, task merged, worktree gone.
        orch.merge_and_push(&mut t).unwrap();
        assert_eq!(t.task.state, TaskState::Merged);
        assert!(!t.worktree.path.exists());
        let (ok, tree) = git_origin(&["ls-tree", "--name-only", "main"]);
        assert!(
            ok && tree.contains("f.txt"),
            "base pushed with the work: {tree:?}"
        );
    }

    #[tokio::test]
    async fn resume_redispatches_an_interrupted_build_in_a_fresh_session() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let dispatched = orch
            .dispatch(TaskId::new("r1"), "do work", TaskKind::Quick, "main")
            .unwrap();

        // Simulate a daemon restart: only the durable core survives, and boot
        // recovery marked the working phase interrupted.
        let mut task = dispatched.task.clone();
        task.apply(TaskEvent::Interrupt).unwrap();
        let mut revived = ActiveTask::reattach(
            task,
            dispatched.worktree.clone(),
            dispatched.plan_path.clone(),
            None,
        );
        assert_eq!(
            revived.task.state,
            TaskState::Interrupted(crate::task::Phase::Build)
        );
        assert!(
            revived.subscribe().is_none(),
            "no live session after reattach"
        );

        orch.resume(&mut revived).unwrap();
        assert_eq!(revived.task.state, TaskState::Building);
        assert!(
            revived.subscribe().is_some(),
            "a fresh build session is warm"
        );
    }

    #[tokio::test]
    async fn resume_redispatches_an_interrupted_plan() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let dispatched = orch
            .dispatch(TaskId::new("r2"), "plan work", TaskKind::Standard, "main")
            .unwrap();

        let mut task = dispatched.task.clone();
        task.apply(TaskEvent::Interrupt).unwrap();
        let mut revived = ActiveTask::reattach(
            task,
            dispatched.worktree.clone(),
            dispatched.plan_path.clone(),
            Some("earlier summary".into()),
        );
        orch.resume(&mut revived).unwrap();
        assert_eq!(revived.task.state, TaskState::Planning);
        assert!(
            revived.subscribe().is_some(),
            "a fresh plan session is warm"
        );
        assert_eq!(revived.last_summary.as_deref(), Some("earlier summary"));
    }

    #[tokio::test]
    async fn approve_merge_refuses_when_primary_checkout_is_not_on_base() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let mut t = orch
            .dispatch(TaskId::new("m1"), "do work", TaskKind::Quick, "main")
            .unwrap();
        std::fs::write(t.worktree.path.join("f.txt"), "hi\n").unwrap();
        orch.on_done(&mut t, done(DonePhase::Build, DoneStatus::Completed, None))
            .unwrap();

        // The user wandered off base in their live checkout.
        assert!(Command::new("git")
            .args(["checkout", "-b", "user-feature"])
            .current_dir(&repo)
            .status()
            .unwrap()
            .success());

        let error = orch.approve_merge(&mut t).expect_err("must refuse");
        let message = error.to_string();
        assert!(
            message.contains("user-feature") && message.contains("main"),
            "error names both branches: {message}"
        );
        assert!(
            !repo.join("f.txt").exists(),
            "nothing was merged into the wrong branch"
        );

        // Back on base, the merge goes through (the state machine already moved to
        // Merged on the first attempt, so drive the git tail directly).
        assert!(Command::new("git")
            .args(["checkout", "main"])
            .current_dir(&repo)
            .status()
            .unwrap()
            .success());
        orch.commit(&t).unwrap();
        orch.merge_into_base(&t.worktree.branch, &t.worktree.base_branch)
            .unwrap();
        assert!(repo.join("f.txt").exists());
    }

    #[tokio::test]
    async fn ending_a_session_reaps_the_harness_instead_of_leaving_a_zombie() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let mut t = orch
            .dispatch(TaskId::new("z1"), "plan work", TaskKind::Standard, "main")
            .unwrap();
        let pid = t.harness_pid().expect("warm harness is running");
        orch.on_done(
            &mut t,
            done(
                DonePhase::Plan,
                DoneStatus::Completed,
                Some(".build/plan.md"),
            ),
        )
        .unwrap();

        // approve_plan ends the plan session; the old harness must be fully reaped
        // (no zombie), not just killed.
        orch.approve_plan(&mut t).unwrap();
        let stat = Command::new("ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
            .unwrap();
        let stat = String::from_utf8_lossy(&stat.stdout).trim().to_string();
        assert!(
            !stat.starts_with('Z'),
            "old harness pid {pid} is a zombie (stat {stat:?})"
        );
    }

    #[tokio::test]
    async fn harness_exit_and_idle_are_observable_for_the_quiescence_monitor() {
        let (dir, repo) = init_repo();
        // A harness that exits immediately — the crash/exit-without-done case.
        let orch = Orchestrator::new(
            repo.to_path_buf(),
            dir.path().join("worktrees"),
            Agent::Warm(HarnessSpec::new("sh").arg("-c").arg("exit 0")),
            Templates::default(),
        );
        let mut t = orch
            .dispatch(TaskId::new("i1"), "do work", TaskKind::Quick, "main")
            .unwrap();
        assert_eq!(t.task.state, TaskState::Building);

        // The child exits promptly; poll until has_exited observes it.
        let mut exited = false;
        for _ in 0..50 {
            if t.harness_exited() {
                exited = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert!(exited, "a dead harness must be observable");
        assert!(t.harness_idle_for().is_some());

        // The monitor's transition: WentIdle demotes to idle_unreported.
        orch.on_idle(&mut t).unwrap();
        assert_eq!(
            t.task.state,
            TaskState::IdleUnreported(crate::task::Phase::Build)
        );
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

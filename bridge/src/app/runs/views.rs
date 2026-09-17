use crate::app::{require_str, view_thread_detail, AppState, DigestScope};
use crate::orchestrator::ActiveRun;
use crate::run::{RunState, StageProgress, StageProgressState};
use crate::thread::ThreadDetail;
use serde_json::{json, Value};

pub(in crate::app) fn diff_json(diff: &crate::diff::WorktreeDiff) -> Value {
    let files: Vec<Value> = diff
        .files()
        .iter()
        .map(|file| json!({ "path": file.path, "status": format!("{:?}", file.status) }))
        .collect();
    json!({
        "stat": diff.stat().to_json(),
        "files": files,
        "patch": diff.patch(),
    })
}

/// Modification times for changed paths that still exist in a checkout.
/// Deleted paths are omitted because neither Git nor the filesystem retains
/// their last worktree modification time.
pub(in crate::app) fn diff_file_edited_at(
    worktree_path: &std::path::Path,
    diff: &crate::diff::WorktreeDiff,
) -> serde_json::Map<String, Value> {
    diff.files()
        .iter()
        .filter_map(|file| {
            let edited_at = crate::diff::file_edited_at(worktree_path, &file.path)?;
            Some((file.path.clone(), json!(edited_at)))
        })
        .collect()
}

pub(in crate::app) fn worktree_diff_json(
    worktree_path: &std::path::Path,
    diff: &crate::diff::WorktreeDiff,
) -> Value {
    let mut value = diff_json(diff);
    value["file_edited_at"] = Value::Object(diff_file_edited_at(worktree_path, diff));
    value
}

/// The wire view of a run stage's execution progress: id, sub-state, immutable
/// commit boundaries, publication evidence, and its validation report if any.
pub(in crate::app) fn run_stage_json(progress: &StageProgress) -> Value {
    json!({
        "id": progress.stage_id,
        "state": run_stage_progress_str(&progress.state),
        "start_sha": progress.start_sha,
        "built_sha": progress.built_sha,
        "completion_sha": progress.completion_sha,
        "publication": progress.publication,
        "invalidation_reason": progress.invalidation_reason,
        "validation": progress.validation.as_ref().map(|v| json!({
            "passed": v.passed,
            "findings": v.findings,
            "notes_for_next_stage": v.notes_for_next_stage,
        })),
    })
}

/// The wire string for a run state.
pub(in crate::app) fn run_state_str(state: &RunState) -> String {
    match state {
        RunState::Created => "created",
        RunState::Building => "building",
        RunState::StageGate => "stage_gate",
        RunState::Review => "review",
        RunState::Blocked => "blocked",
        RunState::Failed => "failed",
        RunState::IdleUnreported => "idle_unreported",
        RunState::Interrupted => "interrupted",
        RunState::Merged => "merged",
        RunState::Abandoned => "abandoned",
        RunState::Archived => "archived",
    }
    .to_string()
}

/// The wire string for a run-side stage progress state.
pub(in crate::app) fn run_stage_progress_str(state: &StageProgressState) -> String {
    match state {
        StageProgressState::Building => "building",
        StageProgressState::Built => "built",
        StageProgressState::Validating => "validating",
        StageProgressState::Validated { passed: true } => "validated_passed",
        StageProgressState::Validated { passed: false } => "validated_failed",
    }
    .to_string()
}

impl AppState {
    pub(crate) fn run_get(&mut self, params: &Value) -> Result<Value, String> {
        let run_id = require_str(params, "run_id")?;
        let active = self.runs.get(&run_id).ok_or("unknown run_id")?;
        // Which conversation, and how much of it: the rail's open bubble names
        // the agent, and the client's cursor says what it already holds.
        // Neither → the thread the view builds itself, cut the same way.
        let detail_thread = self.detail_thread_value(&run_id, params)?;
        let mut view = self.run_view(
            &run_id,
            active,
            view_thread_detail(&detail_thread, params),
            DigestScope::Detail,
        );
        if let Some(thread) = detail_thread {
            view.as_object_mut()
                .expect("run_view returns an object")
                .insert("thread".to_string(), thread);
        }
        Ok(view)
    }

    /// The wire view of a run (spec §board.list): identity + plan link, state,
    /// branch/base, per-stage execution progress (with validation), model, and
    /// timestamps. The live diffstat rides along in `board.list`.
    /// `thread_detail` picks a bounded digest (list surfaces) or the full
    /// conversation (detail surfaces + mutation responses).
    pub(in crate::app) fn run_view(
        &self,
        run_id: &str,
        active: &ActiveRun,
        thread_detail: ThreadDetail,
        scope: DigestScope,
    ) -> Value {
        let project_id = self
            .projects
            .project_id_of(run_id)
            .unwrap_or_default()
            .to_string();
        let project = self
            .projects
            .iter()
            .find(|p| p.id == project_id)
            .map(|p| p.name.clone())
            .unwrap_or_default();
        let unread = self.unread_for(run_id, self.conversation_thread_for_run(active));
        // A branch with no agents has no conversation yet, and an empty one is
        // what says so: the client paints the new-agent view under it.
        let no_conversation = crate::thread::Thread::default();
        let conversation = self
            .conversation_thread_for_run(active)
            .unwrap_or(&no_conversation);
        json!({
            "run_id": run_id,
            "implementation_id": run_id,
            "issue_id": active.run.plan_id.as_ref().map(|p| p.0.clone()),
            "plan_id": active.run.plan_id.as_ref().map(|p| p.0.clone()),
            "goal": active.run.goal,
            "state": run_state_str(&active.run.state),
            // Event-driven, and `needs_attention` is the same fact under the
            // name the SPA already reads.
            "needs_attention": unread.is_unread(),
            "unread": unread.is_unread(),
            "unread_count": unread.count,
            "unread_reason": unread.reason,
            // Told the entry to stop asking. The badge above is already zeroed
            // by it; this is what the inbox renders the control from.
            "muted": self.is_muted(run_id),
            // Cleared out of the inbox until the conversation asks again. Mute
            // silences a row that stays; this one is not in the list at all.
            "dismissed": self.is_dismissed(run_id),
            "attention": self.attention_json(run_id),
            "branch": active.worktree.branch(),
            "base_branch": active.worktree.base_branch,
            "base_sha": active.base_sha,
            "worktree_path": active.worktree.path.display().to_string(),
            "summary": active.last_summary,
            "last_error": active.last_error,
            "project": project,
            "project_id": project_id,
            "harness": if self.qa_agent { self.harness.as_str() } else { active.model_choice.provider.label() },
            "provider": active.model_choice.provider,
            "model": active.model_choice.model,
            "effort": active.model_choice.effort,
            "thread": match thread_detail {
                ThreadDetail::Digest => conversation.digest_value(),
                ThreadDetail::Full => conversation.wire_value(),
                ThreadDetail::Page(limit) => self.first_thread_page(conversation, limit),
            },
            // The rail's bubble strip — see `plan_view`.
            "agents": self.agent_digests(run_id, scope),
            // Review prioritization: an overlay on the diff, never a gate.
            "triage": self.triage_json(active),
            "triage_enabled": self.triage_enabled,
            "auto_advance": active.auto_advance,
            "current_stage_id": active.current_stage_id,
            "adopted": active.adopted,
            "recovery": active.recovery,
            "can_finish": active.run.state == RunState::Merged
                || (active.run.state == RunState::Review && active.worktree.path.exists()),
            "created_at": self.board.attention().clock(run_id).created_at,
            "updated_at": self.board.attention().clock(run_id).updated_at,
            "state_changed_at": self.board.attention().clock(run_id).state_changed_at,
            "stages": active
                .stages
                .iter()
                .map(run_stage_json)
                .collect::<Vec<_>>(),
        })
    }

    /// The run's triage pass, with the one thing the SPA cannot derive: whether
    /// the diff has moved since the pass read it.
    ///
    /// `stale` is derived here and never stored — the diff moves under a triage
    /// constantly, and a stored flag would be a second thing to keep true. A
    /// stale pass still ships: an ordering from the previous revision beats no
    /// ordering at all while the re-triage runs, and the SPA labels it.
    pub(in crate::app) fn triage_json(&self, active: &ActiveRun) -> Value {
        let Some(triage) = &active.triage else {
            return Value::Null;
        };
        let current_revision = self
            .conversation_thread_for_run(active)
            .and_then(|thread| thread.current_revision(crate::thread::ArtifactKind::Diff))
            .map(|revision| revision.content_hash.clone());
        json!({
            "based_on": triage.based_on,
            "hunks": triage.hunks,
            // Where the reviewer already disagreed with the pass. Renders as
            // the level they chose, over the one the agent chose.
            "overrides": triage.overrides,
            // No revision recorded yet means nothing has been observed to move.
            "stale": current_revision.is_some_and(|current| current != triage.based_on),
        })
    }

    /// The live run that already owns the checkout a client names by
    /// `worktree_id`. Adoption takes the worktree off the external list, so the
    /// id is re-derived from each run's canonical worktree root — the same way
    /// the scanner minted it. A terminal run has let go of the checkout, so it
    /// does not answer here.
    pub(in crate::app) fn run_owning_worktree_id(
        &self,
        project_id: &str,
        worktree_id: &str,
    ) -> Option<String> {
        self.runs
            .iter()
            .find(|(run_id, active)| {
                !active.run.state.is_terminal()
                    && self.projects.project_id_of(run_id) == Some(project_id)
                    && crate::worktree::external_worktree_id(&Self::canonical_root(
                        &active.worktree.path,
                    )) == worktree_id
            })
            .map(|(run_id, _)| run_id.clone())
    }
}

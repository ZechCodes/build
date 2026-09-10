use crate::app::{
    append_user_thread_messages, err, issue_session, parse_thread_inputs, parse_viewing_context,
    plan_stage_json, plan_state_str, require_str, scheduler_request, stage_doc_state_str,
    thread_detail, AppState, PlanSessionOpening, NEW_THREAD_MESSAGES_PROMPT,
};
use crate::operation::OperationReceipt;
use crate::plan::{ImplementationIntent, PlanState, StageDocState};
use crate::run::RunState;
use crate::store::now_rfc3339;
use crate::thread::ThreadDetail;
use serde_json::{json, Value};

/// One stage comment on the wire, read off the conversation that holds it.
pub(in crate::app) fn comment_json(comment: &crate::thread::DocComment) -> Value {
    json!({
        "id": comment.id,
        "stage_id": comment.stage_id,
        "path": comment.path,
        "anchor": comment.anchor.as_ref().map(|anchor| json!({
            "heading_path": anchor.heading_path,
            "snippet": anchor.snippet,
            "line_start": anchor.line_start,
            "line_end": anchor.line_end,
        })),
        "body": comment.body,
        "state": match comment.state {
            crate::thread::DocCommentState::Open => "open",
            crate::thread::DocCommentState::Addressed => "addressed",
        },
        "agent_reply": comment.agent_reply,
    })
}

/// Parse the optional `anchor` param of `plan.comment_add`: `null`/absent is a
/// general comment; present, it must carry a string-array `heading_path` and a
/// string `snippet` (capped server-side at 400 chars). Optional `line_start` /
/// `line_end` say where the passage sat when it was selected.
pub(in crate::app) fn parse_comment_anchor(
    value: Option<&Value>,
) -> Result<Option<crate::thread::DocAnchor>, String> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(v) => {
            let heading_path = v
                .get("heading_path")
                .and_then(Value::as_array)
                .ok_or("anchor.heading_path must be an array of strings")?
                .iter()
                .map(|entry| {
                    entry.as_str().map(str::to_string).ok_or_else(|| {
                        "anchor.heading_path must be an array of strings".to_string()
                    })
                })
                .collect::<Result<Vec<String>, String>>()?;
            let snippet = v
                .get("snippet")
                .and_then(Value::as_str)
                .ok_or("anchor.snippet must be a string")?;
            let snippet: String = snippet.chars().take(400).collect();
            Ok(Some(crate::thread::DocAnchor {
                heading_path,
                snippet,
                line_start: parse_anchor_line(v, "line_start")?,
                line_end: parse_anchor_line(v, "line_end")?,
            }))
        }
    }
}

/// One optional line number of a comment anchor. Absent and `null` both mean
/// the reviewer selected a passage without line context; anything else must be
/// a line number.
pub(in crate::app) fn parse_anchor_line(
    anchor: &Value,
    field: &str,
) -> Result<Option<u32>, String> {
    match anchor.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .and_then(|line| u32::try_from(line).ok())
            .map(Some)
            .ok_or_else(|| format!("anchor.{field} must be a line number")),
    }
}

/// A batch of plan notes asked: the revision session they go to.
pub(in crate::app) struct PlanNotesSent {
    pub(in crate::app) plan_id: String,
    pub(in crate::app) project_id: String,
    pub(in crate::app) detail: ThreadDetail,
}

impl PlanSessionOpening for PlanNotesSent {
    fn open(
        self: Box<Self>,
        state: &mut AppState,
        workspace: crate::orchestrator::PlanWorkspace,
    ) -> Result<Value, String> {
        state.settle_plan_session(&self.plan_id, self.detail, |state, active| {
            let turn = state
                .orch_for(&self.project_id)?
                .open_plan_notes(active, workspace, NEW_THREAD_MESSAGES_PROMPT)
                .map_err(err)?;
            state.queue_plan_turn(&self.plan_id, active, turn);
            if state.qa_agent {
                state.qa_simulate_plan(&self.project_id, active)?;
            }
            Ok(())
        })
    }
}

/// One stage's open comments asked: the revision session they are rendered
/// into.
pub(in crate::app) struct StageNotesSent {
    pub(in crate::app) plan_id: String,
    pub(in crate::app) project_id: String,
    pub(in crate::app) stage_id: String,
    pub(in crate::app) detail: ThreadDetail,
}

impl PlanSessionOpening for StageNotesSent {
    fn open(
        self: Box<Self>,
        state: &mut AppState,
        workspace: crate::orchestrator::PlanWorkspace,
    ) -> Result<Value, String> {
        state.settle_plan_session(&self.plan_id, self.detail, |state, active| {
            let turn = state
                .orch_for(&self.project_id)?
                .open_plan_stage_notes(active, workspace, &self.stage_id)
                .map_err(err)?;
            state.queue_plan_turn(&self.plan_id, active, turn);
            if state.qa_agent {
                state.qa_simulate_plan_stage_revise(&self.project_id, active)?;
            }
            Ok(())
        })
    }
}

/// A freeform message asked: the session that hears it, drafting or resumed.
pub(in crate::app) struct PlanMessaged {
    pub(in crate::app) plan_id: String,
    pub(in crate::app) project_id: String,
    pub(in crate::app) detail: ThreadDetail,
}

impl PlanSessionOpening for PlanMessaged {
    fn open(
        self: Box<Self>,
        state: &mut AppState,
        workspace: crate::orchestrator::PlanWorkspace,
    ) -> Result<Value, String> {
        state.settle_plan_session(&self.plan_id, self.detail, |state, active| {
            let turn = state
                .orch_for(&self.project_id)?
                .open_plan_message(active, workspace, NEW_THREAD_MESSAGES_PROMPT)
                .map_err(err)?;
            state.queue_plan_turn(&self.plan_id, active, turn);
            if state.qa_agent && active.plan.state == PlanState::Drafting {
                if active.revising_stage_id.is_some() {
                    state.qa_simulate_plan_stage_revise(&self.project_id, active)?;
                } else {
                    state.qa_simulate_plan(&self.project_id, active)?;
                }
            }
            Ok(())
        })
    }
}

pub(in crate::app) fn attach_plan_operation_turn(
    state: &mut AppState,
    receipt: &OperationReceipt,
) -> Result<(), String> {
    state.delivery_queue.attach_plan_operation(receipt)
}

impl AppState {
    /// Read the single (non-staged) plan doc from the canonical store — never
    /// from a worktree (the worktree is disposable; the store is the truth).
    pub(in crate::app) fn plan_doc(&mut self, params: &Value) -> Result<Value, String> {
        let plan_id = require_str(params, "plan_id")?;
        let active = self.plans.get(&plan_id).ok_or("unknown plan_id")?;
        if active.is_multi_stage() {
            return Err("multi-stage plan: use plan.stages / plan.stage_doc".to_string());
        }
        let plan_path = active.plan_path.clone();
        let contents = self
            .require_store()?
            .read_plan_doc(&plan_id, &plan_path)
            .ok_or_else(|| format!("plan doc not available: {plan_path}"))?;
        Ok(json!({ "plan_path": plan_path, "contents": contents }))
    }

    /// The stage board for a plan: manifest order, doc sub-state, and every
    /// comment (open and addressed) per stage. Read-only.
    pub(in crate::app) fn plan_stages(&mut self, params: &Value) -> Result<Value, String> {
        let plan_id = require_str(params, "plan_id")?;
        let active = self.plans.get(&plan_id).ok_or("unknown plan_id")?;
        if !active.is_multi_stage() {
            return Err("not a multi-stage plan".to_string());
        }
        let stages: Vec<Value> = active
            .stages
            .iter()
            .map(|doc| {
                let mut view = plan_stage_json(active, doc);
                let comments: Vec<Value> = active
                    .agents
                    .sole_thread()
                    .doc_comments()
                    .iter()
                    .filter(|comment| comment.stage_id == doc.id)
                    .map(comment_json)
                    .collect();
                view.as_object_mut()
                    .expect("plan_stage_json returns an object")
                    .insert("comments".to_string(), json!(comments));
                view
            })
            .collect();
        Ok(json!({ "issue_id": plan_id, "plan_id": plan_id, "stages": stages }))
    }

    /// Read one stage's plan doc from the canonical store.
    pub(in crate::app) fn plan_stage_doc(&mut self, params: &Value) -> Result<Value, String> {
        let plan_id = require_str(params, "plan_id")?;
        let stage_id = require_str(params, "stage_id")?;
        let active = self.plans.get(&plan_id).ok_or("unknown plan_id")?;
        if !active.is_multi_stage() {
            return Err("not a multi-stage plan".to_string());
        }
        let index = active.stage_doc_index(&stage_id)?;
        let doc = &active.stages[index];
        // Defense in depth against a corrupted manifest: never read outside the
        // plan dir regardless of what the record says.
        if !doc.path.starts_with(".build/plan/")
            || !crate::plan::is_worktree_contained_path(&doc.path)
        {
            return Err(format!(
                "stage doc path escapes .build/plan/: {:?}",
                doc.path
            ));
        }
        let path = doc.path.clone();
        let contents = self
            .require_store()?
            .read_plan_doc(&plan_id, &path)
            .ok_or_else(|| format!("stage doc not available: {path}"))?;
        Ok(json!({
            "issue_id": plan_id,
            "plan_id": plan_id,
            "stage_id": stage_id,
            "path": path,
            "contents": contents,
        }))
    }

    /// Approve the plan (the last human gate): the planning session ends and
    /// its scratch docs are dropped; the store docs are canonical.
    pub(in crate::app) fn plan_approve(&mut self, params: &Value) -> Result<Value, String> {
        let plan_id = require_str(params, "plan_id")?;
        let project_id = self.project_of(&plan_id)?;
        let mut active = self.take_plan(&plan_id)?;
        let session = issue_session(&active);
        let outcome = self
            .orch_for(&project_id)
            .and_then(|orch| orch.approve_plan(&mut active).map_err(err));
        if outcome.is_ok() {
            self.retire_issue_session(session);
            active.agents.sole_thread_mut().push_event(
                crate::thread::ThreadEventKind::Approved,
                Some("Plan approved".to_string()),
                None,
                None,
                now_rfc3339(),
            );
        }
        let (view, persisted) = self.answer_plan_mutation(plan_id, active, thread_detail(params));
        outcome?;
        persisted?;
        Ok(view)
    }

    /// Send a batch of plan notes back to a fresh revision session.
    pub(in crate::app) fn plan_send_notes(&mut self, params: &Value) -> Result<Value, String> {
        let plan_id = require_str(params, "plan_id")?;
        let messages = parse_thread_inputs(params, crate::thread::ArtifactKind::Plan, "comments")?;
        let project_id = self.project_of(&plan_id)?;
        let mut active = self.take_plan(&plan_id)?;
        append_user_thread_messages(active.agents.sole_thread_mut(), messages);
        // Pure legality first, before any disk is asked for — and the notes are
        // durable either way: an illegal revise leaves what the reviewer wrote
        // on the conversation.
        let gated =
            crate::plan::plan_transition(&active.plan.state, crate::plan::PlanEvent::SendNotes)
                .map_err(|error| error.to_string());
        let title = active.plan.goal.clone();
        let persisted = self.finish_plan_mutation(plan_id.clone(), active);
        gated?;
        persisted?;
        let job = self.reserve_plan_workspace(
            &plan_id,
            project_id.clone(),
            title,
            Box::new(PlanNotesSent {
                plan_id: plan_id.clone(),
                project_id,
                detail: thread_detail(params),
            }),
        )?;
        Ok(self.defer_job(job))
    }

    pub(in crate::app) fn plan_stage_approve(&mut self, params: &Value) -> Result<Value, String> {
        let plan_id = require_str(params, "plan_id")?;
        let stage_id = require_str(params, "stage_id")?;
        let project_id = self.project_of(&plan_id)?;
        let mut active = self.take_plan(&plan_id)?;
        let stage_title = active
            .stages
            .iter()
            .find(|stage| stage.id == stage_id)
            .map(|stage| stage.title.clone())
            .unwrap_or_else(|| stage_id.clone());
        let outcome = self
            .orch_for(&project_id)
            .and_then(|orch| orch.approve_plan_stage(&mut active, &stage_id).map_err(err));
        if outcome.is_ok() {
            active.agents.sole_thread_mut().push_event(
                crate::thread::ThreadEventKind::StageApproved,
                Some(format!("Approved stage “{stage_title}”")),
                None,
                None,
                now_rfc3339(),
            );
        }
        let (view, persisted) =
            self.answer_plan_mutation(plan_id.clone(), active, thread_detail(params));
        outcome?;
        persisted?;
        // Implement All remains armed while it waits on an unapproved stage.
        // Approval is durable before this scheduler hop, so a restart can
        // safely observe the approved doc and resume the same intent.
        let waiting_runs: Vec<String> = self
            .runs
            .iter()
            .filter(|(_, run)| {
                run.auto_advance
                    && run.run.state == RunState::StageGate
                    && run.run.plan_id.as_ref().map(|id| id.0.as_str()) == Some(plan_id.as_str())
            })
            .map(|(run_id, _)| run_id.clone())
            .collect();
        for run_id in waiting_runs {
            self.auto_advance_run(&run_id);
        }
        if self
            .plans
            .get(&plan_id)
            .is_some_and(|issue| issue.plan.implementation_intent != ImplementationIntent::None)
        {
            // The approval this frame just made is what an armed Implement All
            // was parked on, so this hop cuts the whole implementation
            // checkout. It goes to the drain like every other frame's git —
            // and the Issue view it answers with is the one this verb was
            // going to answer with anyway, read after the hop rather than
            // before it.
            return self.implement_issue(
                &plan_id,
                &scheduler_request(&plan_id, params),
                Some(stage_id),
            );
        }
        Ok(view)
    }

    /// Send a stage's open comments to a fresh plan-revision session (the open
    /// comments ARE the payload).
    pub(in crate::app) fn plan_stage_send_notes(
        &mut self,
        params: &Value,
    ) -> Result<Value, String> {
        let plan_id = require_str(params, "plan_id")?;
        let stage_id = require_str(params, "stage_id")?;
        let project_id = self.project_of(&plan_id)?;
        let active = self.plans.get(&plan_id).ok_or("unknown plan_id")?;
        // Pure legality first — nothing is scaffolded for a revise that will be
        // refused, and this changes nothing to have to put back.
        crate::orchestrator::gate_plan_stage_notes(active, &stage_id).map_err(err)?;
        let title = active.plan.goal.clone();
        let job = self.reserve_plan_workspace(
            &plan_id,
            project_id.clone(),
            title,
            Box::new(StageNotesSent {
                plan_id: plan_id.clone(),
                project_id,
                stage_id,
                detail: thread_detail(params),
            }),
        )?;
        Ok(self.defer_job(job))
    }

    /// A freeform human message to the plan's agent.
    pub(in crate::app) fn plan_message(&mut self, params: &Value) -> Result<Value, String> {
        let plan_id = require_str(params, "plan_id")?;
        let message = require_str(params, "message")?;
        let viewing_context = parse_viewing_context(params.get("viewing_context"))?;
        let project_id = self.project_of(&plan_id)?;
        // The user's own words, so the anchor gets its chance — before the
        // record leaves its map (see `note_user_message`).
        self.note_user_message(&plan_id);
        let mut active = self.take_plan(&plan_id)?;
        active.agents.sole_thread_mut().post_user_with_context(
            &message,
            None,
            viewing_context,
            now_rfc3339(),
        );
        // Pure legality first, and the message is durable either way: a plan
        // that refuses the freeform channel still heard what was said.
        let gated = crate::orchestrator::gate_plan_message(&active, NEW_THREAD_MESSAGES_PROMPT)
            .map_err(err);
        let title = active.plan.goal.clone();
        let persisted = self.finish_plan_mutation(plan_id.clone(), active);
        gated?;
        persisted?;
        let job = self.reserve_plan_workspace(
            &plan_id,
            project_id.clone(),
            title,
            Box::new(PlanMessaged {
                plan_id: plan_id.clone(),
                project_id,
                detail: thread_detail(params),
            }),
        )?;
        Ok(self.defer_job(job))
    }

    pub(in crate::app) fn plan_abandon(&mut self, params: &Value) -> Result<Value, String> {
        let plan_id = require_str(params, "plan_id")?;
        let project_id = self.project_of(&plan_id)?;
        let mut active = self.take_plan(&plan_id)?;
        let session = issue_session(&active);
        let outcome = self
            .orch_for(&project_id)
            .and_then(|orch| orch.abandon_plan(&mut active).map_err(err));
        if outcome.is_ok() {
            self.retire_issue_session(session);
            active.agents.sole_thread_mut().push_event(
                crate::thread::ThreadEventKind::Abandoned,
                Some("Plan abandoned".to_string()),
                None,
                None,
                now_rfc3339(),
            );
        }
        let (view, persisted) = self.answer_plan_mutation(plan_id, active, thread_detail(params));
        outcome?;
        persisted?;
        Ok(view)
    }

    /// `issue.archive` — Done, for an issue: file it away without changing its
    /// lifecycle state or deleting canonical docs/run history. Repeating the
    /// request preserves the first archive timestamp.
    ///
    /// Never refused for what was or was not built: an issue the user is done
    /// with is done, and an issue no branch ever implemented says so as a
    /// warning on the row (`finish.warnings`) for them to confirm through.
    pub(in crate::app) fn plan_archive(&mut self, params: &Value) -> Result<Value, String> {
        let plan_id = require_str(params, "plan_id")?;
        let mut active = self.take_plan(&plan_id)?;
        if active.plan.archived_at.is_none() {
            active.plan.archived_at = Some(now_rfc3339());
        }
        let (view, persisted) = self.answer_plan_mutation(plan_id, active, thread_detail(params));
        persisted?;
        Ok(view)
    }

    /// Delete an abandoned plan from the board: drop its record + canonical
    /// docs and the in-memory bookkeeping. Valid only for `Abandoned` plans (a
    /// live or approved plan must be abandoned first), and refused while any
    /// non-terminal run still implements it (that run would lose its docs).
    pub(in crate::app) fn plan_delete(&mut self, params: &Value) -> Result<Value, String> {
        let plan_id = require_str(params, "plan_id")?;
        let active = self.plans.get(&plan_id).ok_or("unknown plan_id")?;
        if active.plan.state != PlanState::Abandoned {
            return Err(format!(
                "plan.delete: plan is {} — only an abandoned plan can be deleted",
                plan_state_str(&active.plan.state)
            ));
        }
        if self.runs.values().any(|r| {
            r.run.plan_id.as_ref().map(|p| &p.0) == Some(&plan_id) && !r.run.state.is_terminal()
        }) {
            return Err("plan.delete: a non-terminal run still implements this plan".to_string());
        }
        if let Some(store) = &self.store {
            store
                .delete_plan(&plan_id)
                .map_err(|e| format!("plan store: {e}"))?;
        }
        self.plans.remove(&plan_id);
        self.projects.unbind_entity(&plan_id);
        self.board.attention_mut().remove_entity_clocks(&plan_id);
        self.reap_orphaned_terminals();
        Ok(json!({ "ok": true }))
    }

    /// Comment on one stage document.
    ///
    /// The comment IS a post on the Issue agent's conversation, anchored to the
    /// passage it is about — the same path a diff comment takes. There is no
    /// second record: the agent reads it with the tool it reads its messages
    /// with, and deleting the post deletes the comment.
    pub(in crate::app) fn plan_comment_add(&mut self, params: &Value) -> Result<Value, String> {
        let plan_id = require_str(params, "plan_id")?;
        let stage_id = require_str(params, "stage_id")?;
        let body = require_str(params, "body")?;
        let viewing_context = parse_viewing_context(params.get("viewing_context"))?;
        let mut active = self.take_plan(&plan_id)?;
        let mut minted: Option<crate::thread::DocComment> = None;
        let outcome = (|| -> Result<(), String> {
            if body.trim().is_empty() {
                return Err("comment body must not be empty".to_string());
            }
            if active.plan.state.is_terminal() {
                return Err("cannot comment on a terminal plan".to_string());
            }
            let index = active.stage_doc_index(&stage_id)?;
            let doc_state = active.stages[index].state;
            if !matches!(doc_state, StageDocState::Planned | StageDocState::Approved) {
                return Err(format!(
                    "comments are only accepted on planned/approved stages (stage is {})",
                    stage_doc_state_str(&doc_state)
                ));
            }
            let anchor = parse_comment_anchor(params.get("anchor"))?;
            let path = active.stages[index].path.clone();
            let id = active
                .agents
                .sole_thread_mut()
                .post_doc_comment_with_context(
                    &plan_id,
                    &stage_id,
                    &path,
                    anchor,
                    body.clone(),
                    viewing_context,
                    now_rfc3339(),
                );
            minted = active
                .agents
                .sole_thread()
                .doc_comments()
                .into_iter()
                .find(|comment| comment.id == id);
            Ok(())
        })();
        let persisted = self.finish_plan_mutation(plan_id, active);
        outcome?;
        persisted?;
        let comment = minted.expect("outcome Ok implies a comment was posted");
        Ok(json!({ "comment": comment_json(&comment) }))
    }

    pub(in crate::app) fn plan_comment_delete(&mut self, params: &Value) -> Result<Value, String> {
        let plan_id = require_str(params, "plan_id")?;
        let comment_id = require_str(params, "comment_id")?;
        let mut active = self.take_plan(&plan_id)?;
        let outcome = (|| -> Result<(), String> {
            let known = active
                .agents
                .sole_thread()
                .doc_comments()
                .into_iter()
                .find(|comment| comment.id == comment_id)
                .ok_or_else(|| format!("unknown comment_id: {comment_id}"))?;
            if known.state != crate::thread::DocCommentState::Open {
                return Err("only open comments can be deleted".to_string());
            }
            active
                .agents
                .sole_thread_mut()
                .remove_doc_comment(&comment_id)
                .map(|_| ())
                .ok_or_else(|| format!("unknown comment_id: {comment_id}"))
        })();
        let persisted = self.finish_plan_mutation(plan_id, active);
        outcome?;
        persisted?;
        Ok(json!({ "ok": true }))
    }
}

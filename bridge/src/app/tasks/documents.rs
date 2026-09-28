use crate::app::{require_str, AppState};
use crate::operation::OperationReceipt;
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

pub(in crate::app) fn attach_plan_operation_turn(
    state: &mut AppState,
    receipt: &OperationReceipt,
) -> Result<(), String> {
    state.delivery_queue.attach_plan_operation(receipt)
}

impl AppState {
    /// Read the single (non-staged) plan doc from the canonical store — never
    /// from a worktree (the worktree is disposable; the store is the truth).
    pub(crate) fn plan_doc(&mut self, params: &Value) -> Result<Value, String> {
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

    /// Read one stage's plan doc from the canonical store.
    pub(crate) fn plan_stage_doc(&mut self, params: &Value) -> Result<Value, String> {
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
            "task_id": plan_id,
            "plan_id": plan_id,
            "stage_id": stage_id,
            "path": path,
            "contents": contents,
        }))
    }
}

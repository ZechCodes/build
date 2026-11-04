//! Opted-in task detail windows; legacy readers retain their full response.
use super::{read::TaskReadContext, read_source::TaskReadSource, views, StoredAnswer};
use crate::app::{optional_nonempty_string, require_str};
use crate::store::sync::{TASK_TIMELINE_DEFAULT_LIMIT, TASK_TIMELINE_MAX_LIMIT};
use crate::store::StoreError;
use serde_json::{json, Value};

pub(super) fn requested(params: &Value) -> Result<bool, String> {
    match params.get("timeline_paged") {
        None | Some(Value::Bool(false)) => {
            if ["before", "limit", "timeline_anchor"]
                .iter()
                .any(|key| params.get(key).is_some())
            {
                return Err("before, limit and timeline_anchor require timeline_paged:true".into());
            }
            Ok(false)
        }
        Some(Value::Bool(true)) => Ok(true),
        _ => Err("timeline_paged must be true or false".into()),
    }
}

impl TaskReadContext {
    pub(super) fn tasks_get_paged(
        &self,
        source: &impl TaskReadSource,
        params: &Value,
    ) -> Result<Value, String> {
        let id = require_str(params, "task_id")?;
        let limit = params
            .get("limit")
            .map(|n| {
                n.as_u64()
                    .filter(|n| (1..=TASK_TIMELINE_MAX_LIMIT as u64).contains(n))
                    .map(|n| n as usize)
                    .ok_or("timeline page limit must be 1 to 100")
            })
            .transpose()?
            .unwrap_or(TASK_TIMELINE_DEFAULT_LIMIT);
        // Empty strings are malformed cursors too; the store owns their typed
        // refusal. Other parameter types retain the ordinary validation path.
        let before = match params.get("before") {
            Some(Value::String(before)) => Some(before.as_str()),
            _ => optional_nonempty_string(params, "before")?,
        };
        let anchor = optional_nonempty_string(params, "timeline_anchor")?;
        if before.is_some() && anchor.is_some() {
            return Err("timeline_anchor and before are mutually exclusive".into());
        }
        let task = source
            .load_tracker_task_bounded(&id)
            .stored()?
            .ok_or_else(|| format!("unknown task_id: {id}"))?;
        let project = self
            .project_paths
            .iter()
            .find(|(_, path)| **path == task.project_path)
            .map(|(id, _)| id.clone())
            .ok_or_else(|| format!("unknown task_id: {id}"))?;
        let page = match anchor {
            Some(anchor) => source.task_timeline_anchor_page(&id, anchor, limit),
            None => source.task_timeline_page(&id, before, limit),
        }
        .map_err(timeline_read_refusal)?;
        let task = self
            .identities
            .read_task_bounded(source, task, &page.timeline)?;
        let mut row = views::task_json(&project, &task);
        let summary = source
            .task_summary_by_number(&task.project_path, task.number)
            .stored()?
            .ok_or("invalid task summary: task projection is incomplete")?;
        for key in [
            "unread_count",
            "unread_agent_comments",
            "unread_mentions",
            "unread_service_mentions",
            "session_started_ms",
            "last_activity_ms",
            "done_at",
        ] {
            if let Some(value) = summary.row.get(key) {
                row[key] = value.clone();
            }
        }
        row["last_reviewed_snapshot_id"] = json!(page.last_reviewed_snapshot_id);
        super::read::extend_task_summary(source, &task, &mut row)?;
        let (linked, truncated) = source
            .load_linked_review_summaries(&task.project_path, &id)
            .stored()?;
        if !linked.is_empty() {
            row["linked_reviews"] = json!(linked);
        }
        if truncated {
            row["linked_reviews_truncated"] = json!(true);
        }
        let answer = json!({"task":row,"timeline":page.timeline,"has_more":page.has_more,"next_cursor":page.next_cursor,"timeline_incarnation":page.timeline_incarnation,"timeline_read_through":page.timeline_read_through});
        if serde_json::to_vec(&answer)
            .map_err(|e| e.to_string())?
            .len()
            > 1024 * 1024
        {
            return Err("bounded task response exceeds 1048576 bytes".into());
        }
        Ok(answer)
    }
}

// Preserve the domain refusal through existing legacy String adapters. The
// shared structured carrier is decoded by ApiError::classify on both RPC lanes.
fn timeline_read_refusal(error: StoreError) -> String {
    match error {
        StoreError::TaskTimelineCursorInvalid => crate::api::v1::reviews::errors::encode(
            "invalid_params",
            StoreError::TaskTimelineCursorInvalid.to_string(),
            json!({"reason":"task_timeline_cursor_invalid"}),
        ),
        other => other.to_string(),
    }
}

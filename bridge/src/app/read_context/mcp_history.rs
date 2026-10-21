//! Capture the published read root, never a fresh application identity graph.
use super::*;
use crate::app::conversations::sql_read::{McpHistoryQuery, McpHistoryWork};
use crate::tracker::TaskComment;

pub(in crate::app) struct McpHistoryContext(Arc<ReadContext>);

impl ReadService {
    pub(in crate::app) fn mcp_history_context(&self) -> McpHistoryContext {
        McpHistoryContext(
            self.context
                .load_full()
                .expect("read context initialized before MCP admission"),
        )
    }
}

impl McpHistoryContext {
    pub(in crate::app) fn history(
        &self,
        transaction: Option<&ReadTransaction<'_>>,
        owner: &str,
        params: &Value,
        query: McpHistoryQuery,
        fallback: Option<(&crate::app::read_view::history::SharedHistory, u64)>,
    ) -> Result<McpHistoryWork, String> {
        self.0
            .threads
            .prepare_mcp_history(transaction, owner, params, query, fallback)
    }

    pub(in crate::app) fn comment(
        &self,
        transaction: &ReadTransaction<'_>,
        project: &str,
        id: &str,
    ) -> Result<(Value, TaskComment, TaskReadFence), String> {
        let unknown = || format!("There is no comment {id} on this project's tasks.");
        let comment = transaction
            .load_tracker_comment(id)
            .map_err(|error| error.to_string())?
            .ok_or_else(unknown)?;
        let params = serde_json::json!({"task_id":comment.task_id});
        let fence = self
            .0
            .tasks
            .validate_scope(transaction, "tasks.get", &params)
            .map_err(|_| unknown())?;
        if fence.project_id() != project {
            return Err(unknown());
        }
        let task = transaction
            .load_tracker_task(&comment.task_id)
            .map_err(|error| error.to_string())?
            .ok_or_else(unknown)?;
        Ok((
            AppState::comment_read_value(&task, &comment),
            comment,
            fence,
        ))
    }
}

impl ReadService {
    pub(in crate::app) fn validate_mcp_comment(&self, fence: &TaskReadFence) -> Result<(), String> {
        self.context
            .load_full()
            .expect("initialized read admission remains available")
            .tasks
            .validate_fence(fence)
    }
}

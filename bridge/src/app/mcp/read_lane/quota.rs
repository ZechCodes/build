//! Bound each SQL read shape before it can join the shared reader pool queue.
use super::service::McpReadService;
use crate::mcp::BridgeAction;
use crate::store::{ReadClass, ReadPool};
use tokio::sync::OwnedSemaphorePermit;

impl McpReadService {
    pub(super) async fn take_class(
        &self,
        pool: &ReadPool,
        action: &BridgeAction,
    ) -> OwnedSemaphorePermit {
        let waiting = pool.acquire_class(class(action));
        #[cfg(test)]
        return match self.sql_probe.as_ref() {
            Some(probe) => probe.observe_wait(action.tool_name(), waiting).await,
            None => waiting.await,
        };
        #[cfg(not(test))]
        waiting.await
    }
}

fn class(action: &BridgeAction) -> ReadClass {
    match action {
        BridgeAction::TrackerGetTask { .. } => ReadClass::TaskDetails,
        BridgeAction::TrackerListTasks { .. } => ReadClass::TaskList,
        BridgeAction::TrackerTaskDependents { .. } => ReadClass::TaskDependents,
        BridgeAction::SearchConversation { .. } => ReadClass::HistorySearch,
        BridgeAction::ReadConversation { .. } => ReadClass::HistoryRead,
        BridgeAction::TrackerReadComment { .. } => ReadClass::Comment,
        BridgeAction::TrackerGetReview { .. } => ReadClass::ReviewSummary,
        BridgeAction::TrackerReadReview { .. } => ReadClass::ReviewContents,
        // Adding a SQL-backed adapter must also choose a bounded class;
        // a missing mapping must never silently bypass the quota.
        _ => unreachable!("every SQL-backed MCP read must have a query class"),
    }
}

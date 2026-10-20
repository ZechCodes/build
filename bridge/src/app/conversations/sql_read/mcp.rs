//! Owned MCP history work from the same canonical/admitted SQL resolver.
use super::*;
use crate::thread::{ConversationQuery, ThreadItem};
use serde_json::json;

pub(in crate::app) enum McpHistoryQuery {
    Search(ConversationQuery),
    CatchUp(usize),
}

pub(in crate::app) struct McpHistoryWork {
    thread: Thread,
    items: Vec<ThreadItem>,
    query: McpHistoryQuery,
    owner: String,
    agent: String,
    conversation: String,
}

impl McpHistoryWork {
    pub(in crate::app) fn render(self) -> Value {
        match self.query {
            McpHistoryQuery::Search(query) => json!({
                "hits": self.thread.search_items(&self.items, &query),
                "threads_searched": [self.thread.id],
            }),
            McpHistoryQuery::CatchUp(limit) => json!({
                "entity_id": self.owner,
                "agent_id": self.agent,
                "conversation_id": self.conversation,
                "transcript": Thread::catch_up_markdown_from(self.items.iter(), limit),
            }),
        }
    }
}

impl ThreadReadContext {
    pub(in crate::app) fn prepare_mcp_history(
        &self,
        transaction: Option<&ReadTransaction<'_>>,
        owner: &str,
        params: &Value,
        query: McpHistoryQuery,
        fallback: Option<(&crate::app::read_view::history::SharedHistory, u64)>,
    ) -> Result<McpHistoryWork, String> {
        let (thread, fence) = self.resolve(transaction, owner, params)?;
        let items = match (&query, transaction) {
            (McpHistoryQuery::Search(_), Some(read)) => read
                .thread_items(&thread.agent.id)
                .map_err(conversation_error)?,
            (McpHistoryQuery::CatchUp(limit), Some(read)) => {
                match read.thread_message_page(&thread.agent.id, *limit) {
                    Ok(items) => items,
                    Err(error) => {
                        let Some((history, floor)) = fallback else {
                            return Err(conversation_error(error));
                        };
                        let Some(items) = history.resident_fallback_items(&thread, floor) else {
                            return Err(conversation_error(error));
                        };
                        eprintln!(
                            "catch-up packet for {}: {error}; the resident tail is what it carries",
                            thread.agent.id
                        );
                        items
                    }
                }
            }
            (_, None) => thread.items.clone(),
        };
        // SQL metadata intentionally has no resident items. Format supplied
        // messages directly instead of filtering them through its tail floor.
        Ok(McpHistoryWork {
            thread: thread.into_owned(),
            items,
            query,
            owner: owner.into(),
            agent: fence.agent,
            conversation: fence.conversation,
        })
    }
}

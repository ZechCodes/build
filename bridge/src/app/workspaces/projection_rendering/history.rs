//! Updated histories and unread cuts share one immutable candidate lifetime.
use super::*;
use crate::app::read_view::history::SharedHistory;
use crate::app::workspaces::ProjectionOwnership;
use crate::thread::{Thread, UnreadSummary};
use std::sync::Arc;

type HistoryKey = (String, String, String, u64);

#[derive(Default)]
pub(super) struct Histories {
    updated: Mutex<BTreeMap<HistoryKey, Arc<SharedHistory>>>,
    unread: Mutex<BTreeMap<(HistoryKey, u64), UnreadSummary>>,
}

fn key(history_id: &str, thread: &Thread) -> HistoryKey {
    (
        history_id.to_owned(),
        thread.agent.id.clone(),
        thread.id.clone(),
        thread.generation_revision,
    )
}

impl AppState {
    fn projection_history_context(&self, thread: &Thread) -> Option<Arc<ProjectionOwnership>> {
        let context = self.snapshot_ownership_context()?;
        let owner = context.rendering.agent_owner(&thread.agent.id)?;
        let canonical = self.entity_agents(owner).ok()?.by_id(&thread.agent.id)?;
        std::ptr::eq(&canonical.thread, thread).then_some(context)
    }

    pub(in crate::app) fn updated_snapshot_history(
        &self,
        history_id: &str,
        previous: &SharedHistory,
        thread: &Thread,
    ) -> Arc<SharedHistory> {
        let Some(context) = self.projection_history_context(thread) else {
            return Arc::new(previous.updated(thread));
        };
        let key = key(history_id, thread);
        if let Some(history) = context
            .rendering
            .histories
            .updated
            .lock()
            .unwrap()
            .get(&key)
            .cloned()
        {
            return history;
        }
        let history = Arc::new(previous.updated(thread));
        context
            .rendering
            .histories
            .updated
            .lock()
            .unwrap()
            .insert(key, Arc::clone(&history));
        history
    }

    pub(in crate::app) fn snapshot_unread_history(
        &self,
        history_id: &str,
        thread: &Thread,
        cursor: u64,
    ) -> UnreadSummary {
        let context = self.projection_history_context(thread);
        let key = (key(history_id, thread), cursor);
        if let Some(summary) = context.as_ref().and_then(|context| {
            context
                .rendering
                .histories
                .unread
                .lock()
                .unwrap()
                .get(&key)
                .copied()
        }) {
            return summary;
        }
        // Preserve the existing resident reason while counting the whole
        // retained history, including separately committed historical rows.
        let mut summary = thread.unread_since(cursor);
        if let Some(history) = self.read_view.load().history(history_id) {
            summary.count = self
                .updated_snapshot_history(history_id, history, thread)
                .unread_since(cursor)
                .count;
        }
        if let Some(context) = context {
            context
                .rendering
                .histories
                .unread
                .lock()
                .unwrap()
                .insert(key, summary);
        }
        summary
    }
}

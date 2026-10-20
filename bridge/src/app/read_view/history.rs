//! Immutable conversation snapshots. Only changed item paths are copied when
//! publishing a new generation of the view; readers borrow every history row.

use crate::app::{thread_cursor, thread_detail};
use crate::store::Store;
use crate::thread::{
    cut_activity_runs, PageCut, RunCensus, Thread, ThreadDetail, ThreadItem, UnreadSummary,
};
use serde_json::{json, Value};
use std::sync::Arc;

mod committed;
mod tree;
use tree::{insert, remove, Node};

#[derive(Clone)]
pub(in crate::app) struct SharedHistory {
    metadata: Value,
    root: Option<Arc<Node>>,
    generation: u64,
    thread_id: String,
    last_sequence: u64,
    publication_sequence: u64,
}

impl SharedHistory {
    /// Warm all rows before publication. Resident copies win over the stored
    /// rows, including in-place mutations in an old creation sequence.
    pub fn warm(thread: &Thread, store: Option<&Store>) -> Result<Self, String> {
        let mut history = Self::empty(thread);
        if thread.total_item_count() > thread.items.len() as u64 {
            let store = store.ok_or("this conversation's history is not stored")?;
            for item in store
                .thread_items(&thread.agent.id)
                .map_err(|error| format!("conversation store: {error}"))?
            {
                history.root = Some(insert(&history.root, Arc::new(item)));
            }
        }
        history.install(thread, true);
        Ok(history)
    }

    /// Call only with a successfully saved canonical conversation. An older
    /// generation is never allowed to replace a reset conversation.
    pub fn updated(&self, thread: &Thread) -> Self {
        let older_revision = thread.generation_revision == self.generation
            && thread.id == self.thread_id
            && thread.publication_sequence() < self.publication_sequence;
        if thread.generation_revision < self.generation || older_revision {
            return self.clone();
        }
        let reset = thread.generation_revision != self.generation || thread.id != self.thread_id;
        let mut next = if reset {
            Self::empty(thread)
        } else {
            self.clone()
        };
        next.install(thread, reset);
        next
    }

    fn empty(thread: &Thread) -> Self {
        Self {
            metadata: Value::Null,
            root: None,
            generation: thread.generation_revision,
            thread_id: thread.id.clone(),
            last_sequence: 0,
            publication_sequence: 0,
        }
    }

    fn install(&mut self, thread: &Thread, all: bool) {
        for item in &thread.items {
            if all || item.latest_sequence() > self.last_sequence {
                self.root = Some(insert(&self.root, Arc::new(item.clone())));
            }
        }
        if self
            .root
            .as_ref()
            .is_some_and(|node| node.count > thread.total_item_count())
        {
            let resident: std::collections::BTreeSet<_> =
                thread.items.iter().map(ThreadItem::sequence).collect();
            let removed: Vec<_> = self
                .items()
                .into_iter()
                .map(ThreadItem::sequence)
                .filter(|sequence| {
                    *sequence >= thread.resident_from_sequence() && !resident.contains(sequence)
                })
                .collect();
            for sequence in removed {
                self.root = remove(&self.root, sequence);
            }
        }
        // The formatter supplies the exact existing metadata and counters,
        // without serializing or cloning a resident conversation's items.
        let cut = PageCut::<&ThreadItem> {
            items: Vec::new(),
            digests: Vec::new(),
        };
        self.metadata = thread.wire_value_of_page(&cut, false);
        self.last_sequence = thread.last_sequence().max(Node::last_sequence(&self.root));
        self.metadata["thread_last_sequence"] = json!(self.last_sequence);
        self.publication_sequence = thread.publication_sequence().max(self.last_sequence);
    }

    pub fn render(&self, params: &Value) -> Value {
        if let Some(after) = thread_cursor(params) {
            let mut items = Vec::new();
            Node::collect_after(&self.root, after, &mut items);
            return self.wire(items, true);
        }
        match thread_detail(params) {
            ThreadDetail::Page(limit) => self.page(limit),
            _ => self.wire(self.items(), false),
        }
    }

    fn wire(&self, items: Vec<&ThreadItem>, delta: bool) -> Value {
        let mut value = self.metadata.clone();
        let object = value.as_object_mut().expect("thread metadata is an object");
        for key in ["oldest_sequence", "has_more", "activity_digests"] {
            object.remove(key);
        }
        if !delta {
            object.remove("thread_total");
            object.remove("thread_last_sequence");
        }
        value["items"] = json!(items);
        value
    }

    fn page(&self, limit: usize) -> Value {
        let mut span = Vec::new();
        Node::collect_page(&self.root, &mut 0, limit, &mut span);
        let census = |from, through| {
            Ok::<_, std::convert::Infallible>(
                span.iter()
                    .filter(|item| (from..=through).contains(&item.sequence()))
                    .fold(RunCensus::default(), |mut census, item| {
                        census.rows += u64::from(item.is_activity());
                        census.tool_calls += u64::from(item.is_tool_call());
                        census
                    }),
            )
        };
        let cut = cut_activity_runs(span.clone(), limit, census).expect("infallible memory census");
        let oldest = cut.items.first().map(|item| item.sequence());
        let mut value = self.metadata.clone();
        value["items"] = json!(cut.items);
        value["activity_digests"] = json!(cut.digests);
        value["oldest_sequence"] = json!(oldest);
        value["has_more"] = json!(oldest.is_some_and(|oldest| self.root.as_ref().is_some_and(
            |node| node.count > cut.items.len() as u64 && self.first_sequence() < oldest
        )));
        value
    }

    pub fn unread_since(&self, cursor: u64) -> UnreadSummary {
        Node::unread_after(&self.root, cursor)
    }

    fn first_sequence(&self) -> u64 {
        let mut node = self.root.as_deref();
        while let Some(current) = node {
            if current.left.is_none() {
                return current.item.sequence();
            }
            node = current.left.as_deref();
        }
        0
    }

    fn items(&self) -> Vec<&ThreadItem> {
        let mut items = Vec::new();
        Node::collect(&self.root, &mut items);
        items
    }

    fn item(&self, sequence: u64) -> Option<&Arc<ThreadItem>> {
        let mut node = self.root.as_ref();
        while let Some(current) = node {
            match sequence.cmp(&current.item.sequence()) {
                std::cmp::Ordering::Equal => return Some(&current.item),
                std::cmp::Ordering::Less => node = current.left.as_ref(),
                std::cmp::Ordering::Greater => node = current.right.as_ref(),
            }
        }
        None
    }
}

#[cfg(test)]
mod tests;

/// Existing row summaries share the same committed tip as the history. These
/// delivery-only edits do not change attention membership or unread counts.
pub(in crate::app) fn patch_thread_digests(
    value: &mut Value,
    delta: &crate::store::CommittedThreadItems,
) -> bool {
    match value {
        Value::Object(object) => {
            let matching = object.get("thread_id").and_then(Value::as_str)
                == Some(delta.thread_id.as_str())
                && object
                    .get("thread_generation_revision")
                    .and_then(Value::as_u64)
                    == Some(delta.generation_revision);
            let changed = matching
                && object
                    .get("last_sequence")
                    .and_then(Value::as_u64)
                    .is_some_and(|sequence| sequence < delta.sequence);
            if changed {
                object.insert("last_sequence".into(), json!(delta.sequence));
            }
            object.values_mut().fold(changed, |changed, value| {
                patch_thread_digests(value, delta) | changed
            })
        }
        Value::Array(values) => values.iter_mut().fold(false, |changed, value| {
            patch_thread_digests(value, delta) | changed
        }),
        _ => false,
    }
}

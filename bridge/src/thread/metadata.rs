use super::{
    default_query_limit, excerpt_around, LastToolCall, Thread, ThreadEvent, ThreadItem,
    UnreadSummary, DEFAULT_QUERY_LIMIT, MAX_QUERY_LIMIT,
};
use serde::Deserialize;
use serde::Serialize;
use std::path::PathBuf;

/// What one item says about itself so a later search can find it.
///
/// Derived once, when the item is written, from the text and the links it
/// carries — never re-derived by a reader, so a search costs a scan of small
/// vectors rather than a re-parse of every body ever posted.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemMetadata {
    /// Commit shas named here, lowercase, in the order they appear.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commits: Vec<String>,
    /// Worktree-relative paths that name real files in the checkout, plus the
    /// paths of links and anchors the bridge already validated.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
    /// Stage ids this item links to.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stages: Vec<String>,
}

/// Where the conversation's checkout is, when the bridge has told the thread.
///
/// Deliberately outside equality and outside the persisted record: two
/// conversations holding the same items are the same conversation wherever
/// they happen to be checked out, and a path on this device means nothing to
/// the next one.
#[derive(Debug, Clone, Default)]
pub struct WorktreeScope(pub(super) Option<PathBuf>);

/// What a cold session asks its own conversation for. Every field narrows;
/// nothing widens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationQuery {
    /// Case-insensitive substring of the item's text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// A path, or the tail of one: matches any indexed file containing it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// A sha of any length: matches an indexed sha that it prefixes, or that
    /// prefixes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    /// A stage id, exactly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage: Option<String>,
    /// `user`, `agent` or `event`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// Only items created after this sequence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since_sequence: Option<u64>,
    /// How many hits at most, newest first.
    #[serde(default = "default_query_limit")]
    pub limit: usize,
}

impl LastToolCall {
    pub(super) fn of(event: &ThreadEvent) -> Self {
        LastToolCall {
            sequence: event.sequence,
            created_at: event.created_at.clone(),
            summary: event.summary.clone(),
            outcome: event.outcome,
        }
    }
}

impl Default for ConversationQuery {
    fn default() -> Self {
        ConversationQuery {
            text: None,
            file: None,
            commit: None,
            stage: None,
            role: None,
            since_sequence: None,
            limit: DEFAULT_QUERY_LIMIT,
        }
    }
}

impl ConversationQuery {
    /// The limit actually applied: never zero, never unbounded.
    pub fn effective_limit(&self) -> usize {
        self.limit.clamp(1, MAX_QUERY_LIMIT)
    }

    fn matches(&self, item: &ThreadItem) -> bool {
        if self
            .since_sequence
            .is_some_and(|since| item.sequence() <= since)
        {
            return false;
        }
        if self
            .role
            .as_deref()
            .is_some_and(|role| !role.eq_ignore_ascii_case(item.role_token()))
        {
            return false;
        }
        let metadata = item.metadata();
        if let Some(file) = &self.file {
            let file = file.to_lowercase();
            if !metadata
                .files
                .iter()
                .any(|indexed| indexed.to_lowercase().contains(&file))
            {
                return false;
            }
        }
        if let Some(commit) = &self.commit {
            let commit = commit.to_lowercase();
            if !metadata
                .commits
                .iter()
                .any(|indexed| indexed.starts_with(&commit) || commit.starts_with(indexed))
            {
                return false;
            }
        }
        if let Some(stage) = &self.stage {
            if !metadata
                .stages
                .iter()
                .any(|indexed| indexed.eq_ignore_ascii_case(stage))
            {
                return false;
            }
        }
        if let Some(text) = &self.text {
            if !item
                .searchable_text()
                .to_lowercase()
                .contains(&text.to_lowercase())
            {
                return false;
            }
        }
        true
    }
}

/// One search result: enough to decide whether to go read the item, never the
/// item itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationHit {
    /// Which conversation this came out of — a search may span more than one.
    pub thread_id: String,
    pub sequence: u64,
    /// `user`, `agent` or `event`.
    pub role: String,
    /// The event kind, for an event. Absent on a message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    pub created_at: String,
    pub excerpt: String,
    #[serde(default, skip_serializing_if = "ItemMetadata::is_empty")]
    pub metadata: ItemMetadata,
}

impl UnreadSummary {
    pub fn is_unread(&self) -> bool {
        self.count > 0
    }
}

impl Thread {
    /// The latest time somebody spoke, extended to the instant an in-flight
    /// turn stopped when an attention event ended it.
    pub fn conversation_activity_at(&self) -> Option<&str> {
        self.conversation_activity_at_summary.as_deref()
    }
    /// The items this query names, newest first, bounded by its limit.
    ///
    /// The point of the tool this serves: a session that lost its context asks
    /// what was decided about one thing, instead of replaying the whole log.
    pub fn search(&self, query: &ConversationQuery) -> Vec<ConversationHit> {
        self.search_items(&self.items, query)
    }
    /// The same search over items the caller read for itself — how a search
    /// reaches history no load left resident: the answer is about the whole
    /// conversation, so the tail is not enough to look through.
    pub fn search_items(
        &self,
        items: &[ThreadItem],
        query: &ConversationQuery,
    ) -> Vec<ConversationHit> {
        items
            .iter()
            .rev()
            .filter(|item| query.matches(item))
            .take(query.effective_limit())
            .map(|item| ConversationHit {
                thread_id: self.id.clone(),
                sequence: item.sequence(),
                role: item.role_token().to_string(),
                kind: match item {
                    ThreadItem::Event(event) => Some(event.event.as_str().to_string()),
                    ThreadItem::Message(_) => None,
                },
                created_at: item.created_at().to_string(),
                excerpt: excerpt_around(&item.searchable_text(), query.text.as_deref()),
                metadata: item.metadata().clone(),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests;

//! Durable conversation threads paired with plans and run diffs.
//!
//! A thread belongs to a stable logical agent identity (the Build-owned plan or
//! run), while individual harness processes are recorded as session lineage.
//! Messages cost agent tokens; events and revision links do not.

mod conversation;
mod items;
mod metadata;
mod paging;
mod rendering;

use conversation::empty_agent;
pub use conversation::{
    ArtifactRevision, CompletionReport, SessionInstance, SessionLineage, SessionStart, Thread,
};
#[cfg(test)]
pub use items::items_serialized;
use items::{completion_text, count_serialized_items, doc_comment_of, excerpt_around};
pub use items::{
    numbered_message_options, AgentIdentity, ArtifactKind, DocAnchor, DocComment, DocCommentState,
    EventClass, MessageAnchor, MessageAttachment, MessageOption, MessageOptionDraft,
    MessageOutcome, MessageRole, MessageSource, OptionChoice, ThreadDetail, ThreadEvent,
    ThreadEventDraft, ThreadEventKind, ThreadItem, ThreadLink, ThreadMessage, ToolCallOutcome,
    AGENT_MESSAGE_REASON, EVENT_ROLE, MAX_MESSAGE_OPTIONS, MAX_OPTION_LABEL_CHARS,
    MAX_OPTION_MESSAGE_BYTES,
};
pub use metadata::{ConversationHit, ConversationQuery, ItemMetadata, WorktreeScope};
use paging::default_query_limit;
pub use paging::{
    activity_rows_read, cut_activity_runs, page_activity_budget, run_items_shipped,
    wire_value_activity_page, ActivityDigest, LastToolCall, PageCut, RunCensus, UnreadSummary,
    DEFAULT_ACTIVITY_PAGE, DEFAULT_QUERY_LIMIT, DEFAULT_THREAD_PAGE, MAX_ACTIVITY_PAGE,
    MAX_QUERY_LIMIT, MAX_THREAD_PAGE, PAGE_ACTIVITY_PER_MESSAGE, PAGE_ACTIVITY_RUN_CAP,
};
use rendering::{page_span, snapshot_contents};

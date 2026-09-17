use super::{count_serialized_items, ArtifactKind, MessageAttachment, Thread, ThreadItem};
use serde_json::json;
use serde_json::Value;

impl Thread {
    /// Public thread payload for the SPA. Historical snapshots are deliberately
    /// excluded so board polling never ships every old patch over E2EE.
    pub fn wire_value(&self) -> Value {
        count_serialized_items(self.items.len());
        json!({
            "id": self.id,
            "agent": self.agent,
            "sessions": self.sessions,
            "items": self.items,
            "revisions": self.revision_summaries(),
            "last_completion": self.last_completion,
        })
    }
    /// Snapshot-free revision listing shared by the full and cursored wire
    /// views.
    pub(super) fn revision_summaries(&self) -> Vec<Value> {
        self.revisions
            .iter()
            .map(|revision| {
                json!({
                    "id": revision.id,
                    "artifact": revision.artifact,
                    "content_hash": revision.content_hash,
                    "created_at": revision.created_at,
                    "snapshot_available": revision.snapshot.is_some(),
                })
            })
            .collect()
    }
    /// Bounded summary for list surfaces: identity plus counters and the
    /// latest event, never message bodies or the item array, so the polled
    /// board payload stops growing with conversation length.
    pub fn digest_value(&self) -> Value {
        let latest_event = self.items.iter().rev().find_map(|item| match item {
            ThreadItem::Event(event) => Some(event),
            ThreadItem::Message(_) => None,
        });
        json!({
            "id": self.id,
            "agent": self.agent,
            "item_count": self.total_item_count(),
            "last_sequence": self.last_sequence(),
            "last_event": latest_event.map(|event| json!({
                "event": event.event,
                "created_at": event.created_at,
            })),
        })
    }
    /// Cursor view for the detail polls: only items created — or mutated in
    /// place — strictly after `after_sequence`, plus `thread_total` /
    /// `thread_last_sequence` so the client can detect a gap (bridge restart,
    /// dropped delta) and refetch in full. Built directly (never by trimming a
    /// full `wire_value`) so the per-poll serialization cost is bounded like
    /// the wire. Sessions, revisions and last_completion are small and
    /// bounded, so they always ship whole.
    pub fn wire_value_after(&self, after_sequence: u64) -> Value {
        self.wire_value_of_delta(&self.resident_after(after_sequence))
    }
    /// The same cursor view, completed with items read back out of the store:
    /// the history under the tail this process loaded, which memory has no
    /// answer for and which a client whose cursor predates a restart is still
    /// owed. Items the tail already holds are taken from the tail, not from
    /// `history` — memory is the fresher copy of those.
    pub fn wire_value_after_including_history(
        &self,
        after_sequence: u64,
        history: &[ThreadItem],
    ) -> Value {
        let mut delta: Vec<&ThreadItem> = history
            .iter()
            .filter(|item| {
                item.sequence() < self.resident_from_sequence
                    && item.latest_sequence() > after_sequence
            })
            .chain(self.resident_after(after_sequence))
            .collect();
        // The store hands its rows back in mutation order; a conversation's own
        // order is creation order, which is what a client merges against.
        delta.sort_by_key(|item| item.sequence());
        self.wire_value_of_delta(&delta)
    }
    /// The delta shape, around items the caller already chose.
    pub(super) fn wire_value_of_delta(&self, delta: &[&ThreadItem]) -> Value {
        count_serialized_items(delta.len());
        json!({
            "id": self.id,
            "agent": self.agent,
            "sessions": self.sessions,
            "items": delta,
            "revisions": self.revision_summaries(),
            "last_completion": self.last_completion,
            "thread_total": self.total_item_count(),
            "thread_last_sequence": self.last_sequence(),
        })
    }
    /// What a resumed agent is handed to rebuild the conversation: the last
    /// `limit` **messages** to and from the agent, and nothing else on the
    /// thread.
    ///
    /// Events are left out by construction rather than by tuning a ratio. They
    /// are Build's observations about the agent, and the limit counts messages
    /// so that a session which emitted hundreds of tool calls before restarting
    /// still hands its replacement what the human said — the exact context the
    /// packet exists to carry.
    ///
    /// An outcome is not one of those observations: it is the agent's own
    /// report, so it is a message, and it is carried with the outcome named on
    /// its line. That is what tells a replacement why its predecessor blocked.
    pub fn catch_up_markdown(&self, limit: usize) -> String {
        catch_up_lines(self.items.iter(), limit)
    }
    /// Context safe to freeze beside a new managed operation. Another managed
    /// operation owns its own delivery, so it must never leak into this one's
    /// provider turn merely because it was accepted first.
    pub fn operation_prior_context(&self, limit: usize) -> String {
        catch_up_lines(
            self.items.iter().filter(|item| {
                !matches!(item, ThreadItem::Message(message) if message.operation_id.is_some())
            }),
            limit,
        )
    }
    /// The same packet, completed with messages read back out of the store —
    /// the conversation under the tail an activity-heavy session left behind.
    ///
    /// Stored rows are admitted only below `resident_from_sequence`, the way
    /// the forward cursor admits them: the tail is the fresher copy of
    /// everything it still holds, so a message in both is carried once, from
    /// memory. The chain then runs exactly the filter-take-reverse the
    /// tail-only packet runs, so the limit still counts messages and still
    /// keeps the newest of them.
    pub fn catch_up_markdown_including_history(
        &self,
        history: &[ThreadItem],
        limit: usize,
    ) -> String {
        catch_up_lines(
            history
                .iter()
                .filter(|item| item.sequence() < self.resident_from_sequence)
                .chain(self.items.iter()),
            limit,
        )
    }
}

/// How far back a page reaches, walking newest→older: to the `limit`-th
/// message, and no further.
///
/// The whole span, not what ships: activity between two messages is the page's
/// too, and [`cut_activity_runs`] decides how much of each run the wire
/// carries. A conversation shorter than the limit is reached whole, which is
/// what makes the oldest walked item either a message or the start of the
/// conversation — and so what makes a run on a page always countable.
pub(super) fn page_span<'a>(
    newest_first: impl Iterator<Item = &'a ThreadItem>,
    limit: usize,
) -> usize {
    let mut taken = 0;
    let mut counted = 0;
    for item in newest_first {
        if counted == limit {
            break;
        }
        taken += 1;
        if item.counts_toward_page() {
            counted += 1;
        }
    }
    taken
}

/// The packet's lines, off whatever conversation the caller assembled: the
/// newest `limit` messages, oldest-first.
pub(super) fn catch_up_lines<'a>(
    items: impl DoubleEndedIterator<Item = &'a ThreadItem>,
    limit: usize,
) -> String {
    let mut lines: Vec<String> = items
        .rev()
        .filter_map(|item| match item {
            ThreadItem::Message(message) => Some(format!(
                "- {}{}{}: {}{}{}",
                message.role.as_str(),
                sender_note(message.from_agent.as_deref()),
                match message.reported_outcome() {
                    Some(outcome) => format!(" [{}]", outcome.as_str()),
                    None => String::new(),
                },
                message.body.replace('\n', " "),
                attachment_note(&message.attachments),
                viewing_context_note(message.viewing_context.as_deref())
            )),
            ThreadItem::Event(_) => None,
        })
        .take(limit)
        .collect();
    lines.reverse();
    lines.join("\n")
}

/// Who sent a message, when it was not the human. The packet is markdown, so
/// a sender only reaches a cold agent if it is written into the line — and the
/// role beside it says `user` for the same words, because that is the side of
/// the conversation they arrived on.
pub(super) fn sender_note(from_agent: Option<&super::AgentIdentity>) -> String {
    from_agent.map_or_else(String::new, |sender| format!(" [from agent {}]", sender.id))
}

/// Render message context exactly once in markdown catch-up. On the wire it
/// remains structured metadata on the message itself.
pub(super) fn viewing_context_note(context: Option<&super::ViewingContext>) -> String {
    context.map_or_else(String::new, |context| {
        let json = serde_json::to_string(context).expect("viewing context always serializes");
        format!(" [viewing context: {json}]")
    })
}

/// The trailer that names a message's files in prose form. The catch-up packet
/// is markdown, not JSON, so a path only reaches a resumed agent if it is
/// written into the line.
pub(super) fn attachment_note(attachments: &[MessageAttachment]) -> String {
    if attachments.is_empty() {
        return String::new();
    }
    let paths: Vec<&str> = attachments
        .iter()
        .map(|attachment| attachment.path.as_str())
        .collect();
    format!(" [attached files, open them: {}]", paths.join(", "))
}

pub(super) fn snapshot_contents(contents: &str, max_bytes: usize) -> String {
    if contents.len() <= max_bytes {
        return contents.to_string();
    }
    let mut boundary = max_bytes;
    while !contents.is_char_boundary(boundary) {
        boundary -= 1;
    }
    let mut snapshot = contents[..boundary].to_string();
    snapshot.push_str("\n\n[Snapshot truncated by Build]");
    snapshot
}

impl ArtifactKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ArtifactKind::Plan => "plan",
            ArtifactKind::Diff => "diff",
            ArtifactKind::Doc => "doc",
        }
    }
}

#[cfg(test)]
mod tests;

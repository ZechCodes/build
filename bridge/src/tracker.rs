//! The per-project issue tracker's records (spec: Issues).
//!
//! An issue, a comment on one, and an event about one. Small, bounded values
//! that are read and written whole — which is why the store keeps each one's
//! serde shape in a `record` column rather than normalizing it into a table.
//!
//! NOT the plan flow. Build's `issues` table and its `issue.*` / `plan.*` verbs
//! are the retired plan-and-stages document flow, which shares the English word
//! and nothing else. Everything here is namespaced `tracker_*` in the store and
//! `issues.*` on the wire so the two can never be reached for each other.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A tracker issue's id. Free of the plan flow, whose ids are `plan-`.
pub const ISSUE_ID_PREFIX: &str = "issue-";
/// A comment's.
pub const COMMENT_ID_PREFIX: &str = "ic-";
/// An event's.
pub const EVENT_ID_PREFIX: &str = "ie-";

/// The longest title an issue may carry. A title is a line; a paragraph belongs
/// in the body.
pub const MAX_TITLE_BYTES: usize = 200;
/// The longest body or comment. The bound a thread message already carries, so
/// an issue delivered into a conversation cannot be longer than the message
/// that carries it.
pub const MAX_BODY_BYTES: usize = 32_000;
/// How many labels one issue holds, and how long each may be.
pub const MAX_LABELS: usize = 20;
pub const MAX_LABEL_BYTES: usize = 40;
/// How many entries one of an issue's four link lists holds.
pub const MAX_LINKS_PER_KIND: usize = 20;

/// How many agents may watch one issue.
///
/// A bound rather than a belief that fifty is the right number: every change to
/// a tracked issue delivers one message per tracker, so an unbounded list is an
/// unbounded write and an unbounded number of agents woken by one edit.
pub const MAX_TRACKERS: usize = 50;

/// A fresh id: the ULID rule every other Build id uses, under this record's
/// own prefix, and **monotonic within this process**.
///
/// The plain rule is only time-ordered to the millisecond, and that is not
/// enough here. A timeline is ordered by `(when, id)`, and one write routinely
/// produces several events sharing one timestamp — an update that relabels and
/// moves stamps both with the same `now`. With 80 random bits as the tie-break,
/// those two would come back in a different order on different reads of the
/// same database, and a reader told not to re-sort would draw the move before
/// the relabel.
///
/// So a mint that lands in a millisecond already used keeps that millisecond
/// and increments the random half instead, which is the standard monotonic
/// ULID rule: ids minted later in this process always sort later, and the
/// tie-break is real.
fn mint_id(prefix: &str) -> String {
    use std::sync::Mutex;
    static LAST: Mutex<(u128, u128)> = Mutex::new((0, 0));
    let now = crate::agent::now_ms();
    let fresh = uuid::Uuid::new_v4().as_u128() & ((1u128 << 80) - 1);
    let mut last = LAST.lock().unwrap_or_else(|held| held.into_inner());
    let (at, randomness) = match *last {
        // Same millisecond, or a clock that stepped back: keep the reading the
        // last id used and take the next value after it, so the order a caller
        // minted in is the order the ids sort in either way.
        (stamped, previous) if now <= stamped => (stamped, previous.saturating_add(1)),
        _ => (now, fresh),
    };
    *last = (at, randomness);
    drop(last);
    format!("{prefix}{}", crate::agent::ulid_body_of(at, randomness))
}

pub fn new_issue_id() -> String {
    mint_id(ISSUE_ID_PREFIX)
}

pub fn new_comment_id() -> String {
    mint_id(COMMENT_ID_PREFIX)
}

pub fn new_event_id() -> String {
    mint_id(EVENT_ID_PREFIX)
}

/// Whether an issue is still open. Independent of [`Issue::status`]: one says
/// where the card is on the board, the other whether anyone is still expected
/// to do something about it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueState {
    #[default]
    Open,
    Closed,
}

impl IssueState {
    pub fn as_str(self) -> &'static str {
        match self {
            IssueState::Open => "open",
            IssueState::Closed => "closed",
        }
    }

    /// The state a wire word names, or `None` for a word that is neither.
    pub fn parse(word: &str) -> Option<IssueState> {
        match word {
            "open" => Some(IssueState::Open),
            "closed" => Some(IssueState::Closed),
            _ => None,
        }
    }
}

/// How much this issue matters. `None` is a value and not an absence: an issue
/// nobody has prioritized says so.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IssuePriority {
    #[default]
    None,
    Low,
    Medium,
    High,
    Urgent,
}

impl IssuePriority {
    pub fn as_str(self) -> &'static str {
        match self {
            IssuePriority::None => "none",
            IssuePriority::Low => "low",
            IssuePriority::Medium => "medium",
            IssuePriority::High => "high",
            IssuePriority::Urgent => "urgent",
        }
    }

    pub fn parse(word: &str) -> Option<IssuePriority> {
        [
            IssuePriority::None,
            IssuePriority::Low,
            IssuePriority::Medium,
            IssuePriority::High,
            IssuePriority::Urgent,
        ]
        .into_iter()
        .find(|priority| priority.as_str() == word)
    }
}

/// Who did something: the human, or one agent by id.
///
/// A project agent is an `Agent` here like any other — its id already says what
/// it is (the `project-` prefix), so there is no second spelling for a reader to
/// have to reconcile. [`Assignee`] is the shape that has a third arm, because
/// "this project's agent" is a destination that may not exist yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Actor {
    User,
    Agent { agent_id: String },
}

impl Actor {
    /// The agent that acted, when one did rather than the human.
    pub fn agent_id(&self) -> Option<&str> {
        match self {
            Actor::User => None,
            Actor::Agent { agent_id } => Some(agent_id),
        }
    }
}

/// Who holds an issue. Assignment is dispatch, so this is also where the work
/// runs — see the spec's "Assignment is dispatch".
///
/// `ProjectAgent` is a destination rather than an identity: a project may not
/// have a conversation, let alone an agent on it, when the issue is handed to
/// it. The two creating kinds (`new_workspace`, `new_agent`) are not here at
/// all — they are how `issues.assign` is ASKED, and they resolve to `Agent`
/// before anything is stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Assignee {
    User,
    ProjectAgent,
    Agent { agent_id: String },
}

impl Assignee {
    pub fn agent_id(&self) -> Option<&str> {
        match self {
            Assignee::Agent { agent_id } => Some(agent_id),
            _ => None,
        }
    }
}

/// What an issue is about, in the repository and in Build.
///
/// Every list is ordered by when its entry was added, deduped, and capped at
/// [`MAX_LINKS_PER_KIND`]. `conversation_ids` holds conversation OWNER ids
/// (`run-…`), which is what `agent.list` and `thread.page` are addressed by, so
/// an issue page can open the conversation working it without a second lookup.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueLinks {
    #[serde(default)]
    pub workspace_ids: Vec<String>,
    #[serde(default)]
    pub branches: Vec<String>,
    #[serde(default)]
    pub commits: Vec<String>,
    #[serde(default)]
    pub conversation_ids: Vec<String>,
    #[serde(default)]
    pub parent_issue_id: Option<String>,
}

impl IssueLinks {
    /// Add one entry to one list, answering whether it was not already there.
    /// Full is not an error: a link list is a convenience, and refusing the
    /// twenty-first would refuse the whole call that carried it.
    pub fn add(list: &mut Vec<String>, value: &str) -> bool {
        if list.iter().any(|existing| existing == value) || list.len() >= MAX_LINKS_PER_KIND {
            return false;
        }
        list.push(value.to_string());
        true
    }

    pub fn links_workspace(&self, workspace_id: &str) -> bool {
        self.workspace_ids.iter().any(|id| id == workspace_id)
    }

    pub fn links_conversation(&self, entity_id: &str) -> bool {
        self.conversation_ids.iter().any(|id| id == entity_id)
    }

    pub fn links_commit(&self, sha: &str) -> bool {
        self.commits.iter().any(|commit| commit == sha)
    }
}

/// One tracker issue.
///
/// `project_path` and not a `proj-N` id, for the reason [`PersistedPlan`] and
/// [`PersistedRun`] carry a path too: an id is minted per boot from the config
/// that restored it, and the same repository can come back wearing another one.
/// The wire only ever carries `project_id`; the boundary resolves it both ways.
///
/// [`PersistedPlan`]: crate::store::PersistedPlan
/// [`PersistedRun`]: crate::store::PersistedRun
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Issue {
    pub id: String,
    pub project_path: String,
    /// Per-project, sequential from 1, minted inside the insert's own
    /// transaction. Never reused: nothing deletes an issue.
    pub number: u64,
    pub title: String,
    pub body: String,
    pub state: IssueState,
    /// The kanban column, as a slug — see [`COLUMNS`]. A string and not an
    /// enum, so a per-project column set later is a record change rather than a
    /// migration.
    pub status: String,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub priority: IssuePriority,
    #[serde(default)]
    pub assignee: Option<Assignee>,
    #[serde(default)]
    pub links: IssueLinks,
    /// The agents watching this issue (spec: Issues → Tracking).
    ///
    /// Ordered by when each started, deduped, capped at [`MAX_TRACKERS`].
    /// `default` because every issue filed before tracking existed has none,
    /// and an empty list is the right answer for them.
    #[serde(default)]
    pub trackers: Vec<String>,
    /// The files filed WITH the issue (spec: Issues → Attachments).
    ///
    /// The same record a message carries, because they are the same thing seen
    /// twice: a screenshot handed to an agent in a conversation and one handed
    /// to it on an issue are one kind of object, and two shapes for it would be
    /// two readers, two renders and two ways to get the mime wrong.
    ///
    /// `default` because every issue filed before attachments existed has none,
    /// and an empty list is the right answer for them.
    #[serde(default)]
    pub attachments: Vec<crate::thread::MessageAttachment>,
    /// Whether the USER is watching this issue (spec: Issues → Watching).
    ///
    /// Beside `trackers` rather than in it. An agent tracker gets every change
    /// delivered into its conversation and its turn started; the user's watch
    /// puts a row in the inbox. Two delivery mechanisms in one list would be a
    /// list every reader has to branch on, and `trackers` is already on the
    /// wire as agent ids that clients match by string.
    ///
    /// `default` false: an issue nobody has watched is one the inbox says
    /// nothing about, which is the point — an agent filing an issue for
    /// another agent must not put a row in front of the user.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub watched: bool,
    /// The last event the user has read on this issue, as an event id.
    ///
    /// Ids are time-ordered, so "after the mark" is a string comparison and
    /// needs no timestamps. `None` is an issue the user has never opened, and
    /// then everything since they started watching is unread.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_through: Option<String>,
    /// The last event the user cleared this issue's inbox row through.
    ///
    /// Done means "clear it until something else happens", which is what a
    /// conversation row's Done already means. Anything after this mark brings
    /// the row back, so the field is a point in the timeline rather than a
    /// flag somebody has to remember to unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dismissed_through: Option<String>,
    pub created_by: Actor,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default)]
    pub closed_at: Option<String>,
}

impl Issue {
    /// Start watching, answering whether this changed anything.
    ///
    /// A set: an agent already watching is not added twice, and saying so again
    /// is not a second fact for a timeline to carry. Past [`MAX_TRACKERS`] the
    /// request is refused rather than dropped — a tracker that was not added
    /// would believe it is being told about an issue it will never hear from
    /// again, which is worse than being told no.
    pub fn track(&mut self, agent_id: &str) -> Result<bool, String> {
        if self.trackers.iter().any(|tracking| tracking == agent_id) {
            return Ok(false);
        }
        if self.trackers.len() >= MAX_TRACKERS {
            return Err(format!(
                "issue #{} already has the most trackers it can carry ({MAX_TRACKERS})",
                self.number
            ));
        }
        self.trackers.push(agent_id.to_string());
        Ok(true)
    }

    /// Stop watching, answering whether this changed anything.
    pub fn untrack(&mut self, agent_id: &str) -> bool {
        let before = self.trackers.len();
        self.trackers.retain(|tracking| tracking != agent_id);
        self.trackers.len() != before
    }

    /// Start or stop the user watching. Answers whether anything changed, so
    /// a caller can skip writing an event for a watch that already stood.
    pub fn set_watched(&mut self, watching: bool) -> bool {
        if self.watched == watching {
            return false;
        }
        self.watched = watching;
        true
    }

    pub fn is_tracked_by(&self, agent_id: &str) -> bool {
        self.trackers.iter().any(|tracking| tracking == agent_id)
    }

    /// Everyone to tell about a change, which is every tracker except whoever
    /// made it.
    ///
    /// The exclusion is the rule the whole feature rests on: an agent woken to
    /// be told what it just did would answer its own message, and two agents
    /// each tracking the other's issue would do it forever.
    pub fn trackers_to_notify(&self, actor: &Actor) -> Vec<String> {
        let acted = actor.agent_id();
        self.trackers
            .iter()
            .filter(|tracking| Some(tracking.as_str()) != acted)
            .cloned()
            .collect()
    }

    /// A newly filed issue, before the store mints its number.
    pub fn drafted(project_path: &str, title: &str, created_by: Actor, now: &str) -> Issue {
        Issue {
            id: new_issue_id(),
            project_path: project_path.to_string(),
            number: 0,
            title: title.to_string(),
            body: String::new(),
            state: IssueState::Open,
            status: DEFAULT_STATUS.to_string(),
            labels: Vec::new(),
            priority: IssuePriority::None,
            assignee: None,
            links: IssueLinks::default(),
            trackers: Vec::new(),
            attachments: Vec::new(),
            watched: false,
            read_through: None,
            dismissed_through: None,
            created_by,
            created_at: now.to_string(),
            updated_at: now.to_string(),
            closed_at: None,
        }
    }

    pub fn is_open(&self) -> bool {
        self.state == IssueState::Open
    }
}

/// One comment on one issue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueComment {
    pub id: String,
    pub issue_id: String,
    pub author: Actor,
    pub body: String,
    /// A durable request for the user to read or answer this comment.
    #[serde(default, skip_serializing_if = "is_false")]
    pub mentions_user: bool,
    /// Typed references, fenced twice: shape by `validate_thread_links`, then
    /// ownership by the issue. See the spec's "Typed references".
    #[serde(default)]
    pub refs: Vec<crate::thread::ThreadLink>,
    /// The files said WITH the comment. `default` for the same reason the
    /// issue's are: every comment written before attachments existed has none.
    #[serde(default)]
    pub attachments: Vec<crate::thread::MessageAttachment>,
    pub created_at: String,
    /// How full the authoring AGENT's context was when it wrote this (#68),
    /// snapshotted at write time. Absent on the user's comments, on an agent's
    /// written before it had a reading, and on every comment before #68.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_context: Option<crate::thread::ContextReading>,
}

fn is_false(value: &bool) -> bool {
    !value
}

/// What happened to an issue. Comments and events interleave into the one
/// timeline `issues.get` answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueEventKind {
    Created,
    Assigned,
    Unassigned,
    Moved,
    Labelled,
    Linked,
    Closed,
    Reopened,
    Dispatched,
    /// An agent started watching this issue — by asking, or by being assigned
    /// it. The payload says which.
    Tracked,
    Untracked,
    /// The USER started or stopped watching. Separate from `tracked`, which is
    /// an agent: one puts a row in the inbox, the other wakes an agent, and a
    /// timeline that called both the same would be hiding which happened.
    Watched,
    Unwatched,
}

impl IssueEventKind {
    pub fn as_str(self) -> &'static str {
        match self {
            IssueEventKind::Created => "created",
            IssueEventKind::Assigned => "assigned",
            IssueEventKind::Unassigned => "unassigned",
            IssueEventKind::Moved => "moved",
            IssueEventKind::Labelled => "labelled",
            IssueEventKind::Linked => "linked",
            IssueEventKind::Closed => "closed",
            IssueEventKind::Reopened => "reopened",
            IssueEventKind::Dispatched => "dispatched",
            IssueEventKind::Tracked => "tracked",
            IssueEventKind::Untracked => "untracked",
            IssueEventKind::Watched => "watched",
            IssueEventKind::Unwatched => "unwatched",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueEvent {
    pub id: String,
    pub issue_id: String,
    pub at: String,
    pub actor: Actor,
    pub kind: IssueEventKind,
    /// What this kind needs said. An empty object where the kind is the whole
    /// fact.
    #[serde(default)]
    pub payload: Value,
}

impl IssueEvent {
    pub fn new(
        issue_id: &str,
        actor: Actor,
        kind: IssueEventKind,
        payload: Value,
        now: &str,
    ) -> IssueEvent {
        IssueEvent {
            id: new_event_id(),
            issue_id: issue_id.to_string(),
            at: now.to_string(),
            actor,
            kind,
            payload,
        }
    }
}

/// One entry of an issue's timeline: something said, or something that
/// happened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TimelineEntry {
    Comment(IssueComment),
    Event(IssueEvent),
}

impl TimelineEntry {
    /// What a timeline is ordered by: when it happened, then the id — which is
    /// time-ordered itself, so two things stamped in the same second still have
    /// one order every reader agrees on.
    pub fn ordering_key(&self) -> (&str, &str) {
        match self {
            TimelineEntry::Comment(comment) => (&comment.created_at, &comment.id),
            TimelineEntry::Event(event) => (&event.at, &event.id),
        }
    }
}

/// One kanban column: the slug that is stored, and the name that is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Column {
    pub id: &'static str,
    pub name: &'static str,
}

/// Phase 1's fixed columns, in board order.
pub const COLUMNS: [Column; 5] = [
    Column {
        id: "backlog",
        name: "Backlog",
    },
    Column {
        id: "ready",
        name: "Ready",
    },
    Column {
        id: "in_progress",
        name: "In progress",
    },
    Column {
        id: "in_review",
        name: "In review",
    },
    Column {
        id: "done",
        name: "Done",
    },
];

/// Where a new issue starts.
pub const DEFAULT_STATUS: &str = "backlog";
/// Where an agent's Complete moves the issue it holds.
pub const IN_REVIEW_STATUS: &str = "in_review";
/// The column that means an agent is finished with an issue, whether or not
/// anybody has closed it.
pub const DONE_STATUS: &str = "done";
/// Where a dispatch moves an issue that has not started.
pub const IN_PROGRESS_STATUS: &str = "in_progress";

/// The columns a dispatch may move an issue out of. Anywhere further along was
/// set deliberately, and a reassignment is not a reason to rewind it.
pub const DISPATCH_MOVES_FROM: [&str; 2] = [DEFAULT_STATUS, "ready"];

/// The columns in which an issue is still the assignee's to finish.
///
/// In review is NOT one of them. In review means the agent has reported
/// Complete and the work is ready to be looked at; whether it is done is
/// somebody else's call, so an issue sitting there is waiting on a reviewer
/// and not on the agent. Done and closed are finished with for the same
/// reason and more obviously.
pub const STILL_TO_FINISH: [&str; 3] = [DEFAULT_STATUS, "ready", IN_PROGRESS_STATUS];

/// The column a word names, by slug or by display name, case-insensitively.
/// `None` is a word that names no column this project has.
pub fn normalize_status(word: &str) -> Option<&'static str> {
    let word = word.trim();
    COLUMNS
        .iter()
        .find(|column| {
            column.id.eq_ignore_ascii_case(word) || column.name.eq_ignore_ascii_case(word)
        })
        .map(|column| column.id)
}

/// Every column's name, for a refusal that has to say what there was to choose
/// from.
pub fn column_names() -> String {
    COLUMNS
        .iter()
        .map(|column| column.id)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Labels as they are stored: trimmed, empties dropped, deduped
/// case-insensitively keeping the first spelling, and refused past the caps.
pub fn normalize_labels(labels: &[String]) -> Result<Vec<String>, String> {
    let mut kept: Vec<String> = Vec::new();
    for label in labels {
        let label = label.trim();
        if label.is_empty() {
            continue;
        }
        if label.len() > MAX_LABEL_BYTES {
            return Err(format!("label exceeds {MAX_LABEL_BYTES} bytes: {label}"));
        }
        if kept.iter().any(|seen| seen.eq_ignore_ascii_case(label)) {
            continue;
        }
        kept.push(label.to_string());
    }
    if kept.len() > MAX_LABELS {
        return Err(format!("an issue carries at most {MAX_LABELS} labels"));
    }
    Ok(kept)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_issue_id_is_time_ordered_and_cannot_be_read_as_a_plan() {
        let first = new_issue_id();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let second = new_issue_id();
        assert!(first.starts_with(ISSUE_ID_PREFIX));
        assert!(!first.starts_with("plan-"), "{first}");
        assert!(first < second, "{first} then {second}");
        assert_ne!(new_comment_id()[..3].to_string(), new_event_id()[..3]);
    }

    /// The property a timeline's tie-break rests on: a burst minted inside one
    /// millisecond still sorts in the order it was minted. Without it, two
    /// events stamped with the same `now` come back in a different order on
    /// different reads of the same database.
    #[test]
    fn a_burst_of_ids_minted_in_one_millisecond_still_sorts_in_mint_order() {
        let burst: Vec<String> = (0..500).map(|_| new_event_id()).collect();
        let mut sorted = burst.clone();
        sorted.sort();
        assert_eq!(burst, sorted, "ids minted later must sort later");
        let unique: std::collections::BTreeSet<&String> = burst.iter().collect();
        assert_eq!(unique.len(), burst.len(), "and none of them repeats");
    }

    /// The rule holds across the three prefixes too: a comment and an event
    /// written in one breath interleave by the order they were minted in.
    #[test]
    fn comments_and_events_minted_together_interleave_in_mint_order() {
        let comment = new_comment_id();
        let event = new_event_id();
        assert!(
            comment[COMMENT_ID_PREFIX.len()..] < event[EVENT_ID_PREFIX.len()..],
            "{comment} then {event}"
        );
    }

    #[test]
    fn a_column_is_named_by_its_slug_or_by_what_it_says() {
        assert_eq!(normalize_status("in_progress"), Some("in_progress"));
        assert_eq!(normalize_status("In progress"), Some("in_progress"));
        assert_eq!(normalize_status("  IN PROGRESS "), Some("in_progress"));
        assert_eq!(normalize_status("Backlog"), Some("backlog"));
        assert_eq!(normalize_status("icebox"), None);
        assert_eq!(normalize_status(""), None);
    }

    #[test]
    fn labels_keep_the_first_spelling_and_refuse_past_their_caps() {
        let kept = normalize_labels(&[
            "  bug ".into(),
            "".into(),
            "Bug".into(),
            "ui".into(),
            "   ".into(),
        ])
        .expect("ordinary labels normalize");
        assert_eq!(kept, vec!["bug".to_string(), "ui".to_string()]);

        let too_long = vec!["x".repeat(MAX_LABEL_BYTES + 1)];
        assert!(normalize_labels(&too_long).is_err());

        let too_many: Vec<String> = (0..MAX_LABELS + 1).map(|n| format!("label-{n}")).collect();
        assert!(normalize_labels(&too_many).is_err());
    }

    #[test]
    fn a_link_list_takes_each_entry_once_and_stops_at_its_cap() {
        let mut list = Vec::new();
        assert!(IssueLinks::add(&mut list, "ws-1"));
        assert!(!IssueLinks::add(&mut list, "ws-1"), "added twice");
        for n in 0..MAX_LINKS_PER_KIND {
            IssueLinks::add(&mut list, &format!("ws-fill-{n}"));
        }
        assert_eq!(list.len(), MAX_LINKS_PER_KIND);
        assert!(!IssueLinks::add(&mut list, "ws-over"), "past the cap");
    }

    fn issue() -> Issue {
        Issue::drafted("/repo", "one", Actor::User, "2026-09-20T15:00:00Z")
    }

    /// Tracking is a set, and saying a thing twice is not a second fact.
    #[test]
    fn tracking_twice_adds_one_tracker_and_reports_the_second_as_no_change() {
        let mut issue = issue();
        assert_eq!(issue.track("agent-1"), Ok(true));
        assert_eq!(issue.track("agent-1"), Ok(false), "already watching");
        assert_eq!(issue.track("agent-2"), Ok(true));
        assert_eq!(
            issue.trackers,
            vec!["agent-1".to_string(), "agent-2".into()]
        );
        assert!(issue.is_tracked_by("agent-2"));
    }

    /// Untracking what was never tracked changes nothing and says so.
    #[test]
    fn untracking_someone_who_was_not_watching_is_no_change() {
        let mut issue = issue();
        issue.track("agent-1").unwrap();
        assert!(!issue.untrack("agent-nobody"));
        assert!(issue.untrack("agent-1"));
        assert!(issue.trackers.is_empty());
        assert!(!issue.untrack("agent-1"), "and again is no change");
    }

    /// Past the cap the request is REFUSED rather than dropped: a tracker that
    /// was silently not added would believe it is being told about an issue it
    /// will never hear from again.
    #[test]
    fn the_tracker_list_refuses_past_its_cap_rather_than_dropping_quietly() {
        let mut issue = issue();
        for n in 0..MAX_TRACKERS {
            issue.track(&format!("agent-{n}")).expect("under the cap");
        }
        let refused = issue.track("agent-over").expect_err("past the cap");
        assert!(refused.contains(&MAX_TRACKERS.to_string()), "{refused}");
        assert_eq!(issue.trackers.len(), MAX_TRACKERS);
    }

    /// The rule the whole feature rests on: nobody is told what they just did.
    #[test]
    fn an_agents_own_change_is_never_delivered_back_to_it() {
        let mut issue = issue();
        issue.track("agent-1").unwrap();
        issue.track("agent-2").unwrap();

        let told = issue.trackers_to_notify(&Actor::Agent {
            agent_id: "agent-1".into(),
        });
        assert_eq!(told, vec!["agent-2".to_string()], "not the actor");

        let by_user = issue.trackers_to_notify(&Actor::User);
        assert_eq!(
            by_user,
            vec!["agent-1".to_string(), "agent-2".into()],
            "the human is no tracker, so every tracker hears a human's change"
        );
    }

    /// An actor and an assignee are two shapes on purpose: only a destination
    /// can be a project's agent, because the project may have no agent yet.
    #[test]
    fn an_assignee_may_be_the_projects_agent_and_an_actor_may_not() {
        let assigned = serde_json::to_value(Assignee::ProjectAgent).unwrap();
        assert_eq!(assigned, serde_json::json!({ "kind": "project_agent" }));
        let actor: Result<Actor, _> =
            serde_json::from_value(serde_json::json!({ "kind": "project_agent" }));
        assert!(actor.is_err(), "{actor:?}");
        assert_eq!(
            serde_json::to_value(Actor::Agent {
                agent_id: "agent-1".into()
            })
            .unwrap(),
            serde_json::json!({ "kind": "agent", "agent_id": "agent-1" })
        );
    }

    /// A timeline entry says which of the two it is on the wire, so a client
    /// reading one list never has to guess by looking for a field.
    #[test]
    fn a_timeline_entry_names_its_own_kind() {
        let event = TimelineEntry::Event(IssueEvent::new(
            "issue-1",
            Actor::User,
            IssueEventKind::Created,
            Value::Null,
            "2026-09-19T10:00:00Z",
        ));
        let wire = serde_json::to_value(&event).unwrap();
        assert_eq!(wire["type"], "event");
        assert_eq!(wire["kind"], "created");
        assert_eq!(event.ordering_key().0, "2026-09-19T10:00:00Z");
    }
}

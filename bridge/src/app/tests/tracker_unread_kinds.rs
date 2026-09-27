//! Which timeline entries count as unread (#183): comments, changes to who
//! holds an issue or where it stands, and agent-created asks. Other
//! bookkeeping does not.
//!
//! One table of cases, answered by the real `unread_since_mark`. With
//! `BUILD_PRINT_UNREAD_KINDS` set it prints them, and
//! `spa/test/issueUnreadKinds183.test.js` holds the SPA's fallback count to
//! the same answers.

use crate::app::tracker::unread_since_mark;
use crate::tracker::{Issue, IssueEventKind, TimelineEntry};
use serde_json::{json, Value};

/// Every kind there is. The predicate's match is exhaustive, so a new kind
/// cannot compile without a decision; this list puts each one in the table.
const EVERY_KIND: [IssueEventKind; 18] = [
    IssueEventKind::Created,
    IssueEventKind::Assigned,
    IssueEventKind::Unassigned,
    IssueEventKind::Moved,
    IssueEventKind::Labelled,
    IssueEventKind::Linked,
    IssueEventKind::Closed,
    IssueEventKind::Reopened,
    IssueEventKind::Dispatched,
    IssueEventKind::Tracked,
    IssueEventKind::Untracked,
    IssueEventKind::Watched,
    IssueEventKind::Unwatched,
    IssueEventKind::BranchDeleted,
    IssueEventKind::BranchKept,
    IssueEventKind::WorkspaceIdle,
    IssueEventKind::WorkspacePruned,
    IssueEventKind::WorkspaceReclaimed,
];

const COUNTED: [IssueEventKind; 5] = [
    IssueEventKind::Assigned,
    IssueEventKind::Unassigned,
    IssueEventKind::Moved,
    IssueEventKind::Closed,
    IssueEventKind::Reopened,
];

/// An id `at` steps along one ULID clock; step 0 is the read mark.
fn id(prefix: &str, at: u32) -> String {
    format!("{prefix}-01K5Z{at:021}")
}

const MARK: u32 = 10;

fn agent() -> Value {
    json!({ "kind": "agent", "agent_id": "agent-01K5ZFILER" })
}

fn event(kind: IssueEventKind, actor: Value, at: u32) -> Value {
    json!({ "type": "event", "id": id("ie", at), "issue_id": "issue-1", "at": "2026-09-27T02:00:00Z",
        "actor": actor, "kind": kind.as_str(), "payload": {} })
}

fn comment(author: Value, at: u32) -> Value {
    json!({ "type": "comment", "id": id("ic", at), "issue_id": "issue-1", "author": author,
        "body": "An update.", "created_at": "2026-09-27T02:00:00Z" })
}

/// One case: its name, the mark, the timeline, and what the bridge counts.
fn case(name: &str, read_through: Option<String>, timeline: Vec<Value>) -> Value {
    let issue: Issue = serde_json::from_value(json!({
        "id": "issue-1", "project_path": "/p", "number": 1, "title": "t", "body": "", "state": "open",
        "status": "ready", "watched": true, "read_through": read_through, "created_by": agent(),
        "created_at": "2026-09-27T01:00:00Z", "updated_at": "2026-09-27T02:00:00Z",
    }))
    .expect("an issue");
    let entries: Vec<TimelineEntry> = timeline
        .iter()
        .map(|entry| serde_json::from_value(entry.clone()).expect("a timeline entry"))
        .collect();
    let unread = unread_since_mark(&issue, &entries);
    json!({ "name": name, "read_through": issue.read_through, "timeline": timeline, "unread": unread })
}

fn table() -> Vec<Value> {
    let mark = || Some(id("ie", MARK));
    let mut mentioned_creation = event(IssueEventKind::Created, agent(), MARK + 1);
    mentioned_creation["mentions_user"] = json!(true);
    let mut cases: Vec<Value> = EVERY_KIND
        .iter()
        .map(|kind| {
            case(
                &format!("an agent's {}", kind.as_str()),
                mark(),
                vec![event(*kind, agent(), MARK + 1)],
            )
        })
        .collect();
    cases.extend([
        case(
            "an unread agent-created mention",
            None,
            vec![mentioned_creation.clone()],
        ),
        case(
            "a read agent-created mention",
            Some(id("ie", MARK + 1)),
            vec![mentioned_creation],
        ),
        case(
            "an agent filed and tracked it",
            mark(),
            vec![
                event(IssueEventKind::Created, agent(), MARK + 1),
                event(IssueEventKind::Tracked, agent(), MARK + 2),
            ],
        ),
        case(
            "an agent commented and moved it",
            mark(),
            vec![
                comment(agent(), MARK + 1),
                event(IssueEventKind::Moved, agent(), MARK + 2),
            ],
        ),
        case(
            "the user commented and moved it",
            mark(),
            vec![
                comment(json!({ "kind": "user" }), MARK + 1),
                event(IssueEventKind::Moved, json!({ "kind": "user" }), MARK + 2),
            ],
        ),
        case(
            "an agent commented before the mark",
            mark(),
            vec![comment(agent(), MARK - 1)],
        ),
        case(
            "never opened: filed, tracked, then a comment",
            None,
            vec![
                event(IssueEventKind::Created, agent(), 1),
                event(IssueEventKind::Tracked, agent(), 2),
                comment(agent(), 3),
            ],
        ),
    ]);
    cases
}

#[test]
fn an_agent_created_mention_counts_until_it_is_read() {
    let cases = table();
    assert_eq!(unread_of(&cases, "an unread agent-created mention"), 1);
    assert_eq!(unread_of(&cases, "a read agent-created mention"), 0);
}

fn unread_of(cases: &[Value], name: &str) -> u64 {
    cases
        .iter()
        .find(|case| case["name"] == json!(name))
        .and_then(|case| case["unread"].as_u64())
        .unwrap_or_else(|| panic!("the table has {name}"))
}

#[test]
fn only_news_and_agent_created_asks_count_as_unread() {
    let cases = table();
    for kind in EVERY_KIND {
        let want = u64::from(COUNTED.contains(&kind));
        assert_eq!(
            unread_of(&cases, &format!("an agent's {}", kind.as_str())),
            want,
            "{}",
            kind.as_str()
        );
    }
    assert_eq!(unread_of(&cases, "an agent filed and tracked it"), 0);
    assert_eq!(unread_of(&cases, "an agent commented and moved it"), 2);
    assert_eq!(unread_of(&cases, "the user commented and moved it"), 0);
    assert_eq!(unread_of(&cases, "an agent commented before the mark"), 0);
    assert_eq!(
        unread_of(&cases, "never opened: filed, tracked, then a comment"),
        1
    );

    if std::env::var_os("BUILD_PRINT_UNREAD_KINDS").is_some() {
        println!("BUILD_UNREAD_KINDS={}", json!({ "cases": cases }));
    }
}

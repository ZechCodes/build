//! The issue tracker's persistence (spec: Issues).
//!
//! Six methods, all whole-record: nothing above this line is handed SQL, a
//! connection, or a row. Replacing SQLite with something else — a service, a
//! file per issue, a git-backed store — is implementing these six against
//! something else, and the app layer does not change.
//!
//! Every write is one transaction, for the reason every other write here is:
//! an issue and the events that explain it land together or not at all. A
//! timeline that says an issue was moved by an issue that did not move is worse
//! than either half missing.

use super::{Store, StoreError};
use crate::tracker::{Issue, IssueComment, IssueEvent, IssueState, TimelineEntry};
use rusqlite::{OptionalExtension, Transaction};
use std::collections::HashMap;
use std::path::PathBuf;

/// The most issue ids one timeline read binds. SQLite refuses a statement with
/// more parameters than its build allows (999 before 3.32), so a long list is
/// read in chunks well under any of them.
const TIMELINE_IDS_PER_READ: usize = 500;

/// What a list read narrows to, answered in SQL off the hoisted columns.
///
/// Only the two that are columns. An assignee and a label live inside the
/// record — hoisting a label list would mean a join table, which phase 1 does
/// not need — so those two filters are the caller's, applied to what this
/// answers.
#[derive(Debug, Default, Clone, Copy)]
pub struct IssueFilter<'a> {
    pub state: Option<IssueState>,
    /// A column slug, already normalized by the boundary that took it.
    pub status: Option<&'a str>,
}

/// The `meta` key under which this store keeps the key its list cursors are
/// made under (#85). A `proj-N` id is minted per boot and a repository path
/// can be tracked by two stores, so neither says which store a cursor came
/// from; this key does.
const LIST_KEY: &str = "tracker_list_key";

/// Where a list read starts and how much of it to read: the issues numbered
/// below `below`, at most `take` of them, out of at most `scan` rows read.
/// The default is the whole list.
#[derive(Debug, Default, Clone, Copy)]
pub struct IssueSeek {
    pub below: Option<u64>,
    pub take: Option<usize>,
    pub scan: Option<usize>,
}

/// One stretch of a list, newest first. `scanned_to` is set when the read
/// stopped at its `scan` bound with rows still below: the number of the last
/// row it read, which the next stretch starts below. What it kept may then
/// be short of `take`, or nothing.
#[derive(Debug, Default)]
pub struct IssueStretch {
    pub issues: Vec<Issue>,
    pub scanned_to: Option<u64>,
}

impl IssueFilter<'_> {
    /// The `WHERE` tail this filter adds, and the values it binds after the
    /// project key. Built together so a clause can never outnumber its binds.
    fn clause(&self) -> (String, Vec<String>) {
        let mut sql = String::new();
        let mut binds = Vec::new();
        if let Some(state) = self.state {
            binds.push(state.as_str().to_string());
            sql.push_str(&format!(" AND state = ?{}", binds.len() + 1));
        }
        if let Some(status) = self.status {
            binds.push(status.to_string());
            sql.push_str(&format!(" AND status = ?{}", binds.len() + 1));
        }
        (sql, binds)
    }
}

impl Store {
    /// File a new issue, minting its per-project number inside the same
    /// transaction as the insert, and write the events that explain it.
    ///
    /// The number is `MAX(number) + 1` over the project under the `IMMEDIATE`
    /// transaction every store write already takes, so two writers cannot read
    /// the same maximum; the unique index is the backstop if one ever does.
    /// Answers the issue as it was stored — the draft handed in carries no
    /// number to be trusted.
    pub fn create_tracker_issue(
        &self,
        draft: Issue,
        events: &[IssueEvent],
    ) -> Result<Issue, StoreError> {
        self.in_transaction(|tx| {
            let number = next_issue_number(tx, &draft.project_path)?;
            let issue = Issue { number, ..draft };
            write_tracker_issue(tx, &issue)?;
            append_activity(tx, &[], events)?;
            Ok(issue)
        })
    }

    /// Write an issue back, with whatever it said and whatever happened to it
    /// in the same breath.
    ///
    /// One method rather than three, because every mutating verb is this shape:
    /// the record moved, and the timeline has to say why. Passing empty slices
    /// is a plain save.
    pub fn save_tracker_issue_activity(
        &self,
        issue: &Issue,
        comments: &[IssueComment],
        events: &[IssueEvent],
    ) -> Result<(), StoreError> {
        self.in_transaction(|tx| {
            write_tracker_issue(tx, issue)?;
            append_activity(tx, comments, events)
        })
    }

    /// One issue, or `None` for an id nothing answers to.
    pub fn load_tracker_issue(&self, issue_id: &str) -> Result<Option<Issue>, StoreError> {
        let conn = self.connection();
        let raw: Option<String> = conn
            .query_row(
                "SELECT record FROM tracker_issues WHERE id = ?1",
                [issue_id],
                |row| row.get(0),
            )
            .optional()?;
        raw.map(|raw| decode(&raw, "tracker_issues", issue_id))
            .transpose()
    }

    /// One project's issues, newest first — `number` descending, which is the
    /// order they are read in and the order the index is built in.
    pub fn list_tracker_issues(
        &self,
        project_path: &str,
        filter: IssueFilter<'_>,
    ) -> Result<Vec<Issue>, StoreError> {
        self.list_tracker_issues_below(project_path, filter, IssueSeek::default(), |_| true)
            .map(|stretch| stretch.issues)
    }

    /// One stretch of a project's list, newest first: the issues numbered
    /// below `seek.below` that `keep` keeps, no more than `seek.take` of them,
    /// out of no more than `seek.scan` rows read (#85).
    ///
    /// Read down the `(project, number)` index and stopped as soon as the
    /// stretch is full or the scan bound is reached, so a page costs the rows
    /// it answers and a bounded number `keep` passed over on the way, never
    /// the whole project: a label nobody carries costs one bound's worth of
    /// rows, not every issue. `keep` is the caller's half of the filter — an
    /// assignee, a label — applied here so the stretch counts only what it
    /// keeps.
    pub fn list_tracker_issues_below(
        &self,
        project_path: &str,
        filter: IssueFilter<'_>,
        seek: IssueSeek,
        mut keep: impl FnMut(&Issue) -> bool,
    ) -> Result<IssueStretch, StoreError> {
        let (tail, binds) = filter.clause();
        let mut values: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(project_path.to_string())];
        values.extend(
            binds
                .into_iter()
                .map(|bind| Box::new(bind) as Box<dyn rusqlite::ToSql>),
        );
        let below = match seek.below {
            Some(number) => {
                values.push(Box::new(i64::try_from(number).unwrap_or(i64::MAX)));
                format!(" AND number < ?{}", values.len())
            }
            None => String::new(),
        };
        let sql = format!(
            "SELECT id, number, record FROM tracker_issues WHERE project_key = ?1{tail}{below} \
             ORDER BY number DESC"
        );
        let conn = self.connection();
        let mut statement = conn.prepare(&sql)?;
        let mut rows = statement.query(rusqlite::params_from_iter(values.iter()))?;
        let mut stretch = IssueStretch::default();
        let mut scanned = 0;
        let mut last_read = None;
        while seek.take.is_none_or(|take| stretch.issues.len() < take) {
            let Some(row) = rows.next()? else { break };
            // A row past the bound is only fetched, never decoded: it says
            // there is more below, which is what the next stretch is for.
            if seek.scan.is_some_and(|scan| scanned >= scan) {
                stretch.scanned_to = last_read;
                break;
            }
            scanned += 1;
            let (id, number, raw): (String, i64, String) = (row.get(0)?, row.get(1)?, row.get(2)?);
            last_read = u64::try_from(number).ok();
            let issue = decode(&raw, "tracker_issues", &id)?;
            if keep(&issue) {
                stretch.issues.push(issue);
            }
        }
        Ok(stretch)
    }

    /// The key this store's list cursors are made under, minted the first
    /// time one is asked for and kept for the life of the store (#85).
    pub fn tracker_list_key(&self) -> Result<String, StoreError> {
        let conn = self.connection();
        let read = |conn: &rusqlite::Connection| {
            conn.query_row("SELECT value FROM meta WHERE key = ?1", [LIST_KEY], |row| {
                row.get::<_, String>(0)
            })
            .optional()
        };
        if let Some(key) = read(&conn)? {
            return Ok(key);
        }
        conn.execute(
            "INSERT OR IGNORE INTO meta (key, value) VALUES (?1, ?2)",
            rusqlite::params![LIST_KEY, uuid::Uuid::new_v4().simple().to_string()],
        )?;
        Ok(read(&conn)?.unwrap_or_default())
    }

    /// One comment by its id, and the issue it is on.
    ///
    /// A read of its own because a notice names one comment: an agent told
    /// "new comment ic-… on #53" that had to load the whole timeline to find
    /// it would be paying for the thing the notice exists to avoid.
    pub fn load_tracker_comment(
        &self,
        comment_id: &str,
    ) -> Result<Option<IssueComment>, StoreError> {
        let conn = self.connection();
        let mut statement =
            conn.prepare("SELECT id, record FROM tracker_comments WHERE id = ?1")?;
        let mut rows = statement.query([comment_id])?;
        let Some(row) = rows.next()? else {
            return Ok(None);
        };
        let id: String = row.get(0)?;
        let raw: String = row.get(1)?;
        decode(&raw, "tracker_comments", &id).map(Some)
    }

    /// One issue's comments and events as a single ascending timeline.
    ///
    /// Merged here rather than by the caller because the ordering rule is a
    /// property of the record — when it happened, then the id, which is
    /// time-ordered itself so two things stamped in the same second still have
    /// one order every reader agrees on.
    pub fn load_tracker_timeline(&self, issue_id: &str) -> Result<Vec<TimelineEntry>, StoreError> {
        let conn = self.connection();
        let mut entries = read_records(
            &conn,
            "SELECT id, record FROM tracker_comments WHERE issue_id = ?1",
            issue_id,
            "tracker_comments",
        )?
        .into_iter()
        .map(TimelineEntry::Comment)
        .collect::<Vec<_>>();
        entries.extend(
            read_records(
                &conn,
                "SELECT id, record FROM tracker_events WHERE issue_id = ?1",
                issue_id,
                "tracker_events",
            )?
            .into_iter()
            .map(TimelineEntry::Event),
        );
        in_timeline_order(&mut entries);
        Ok(entries)
    }

    /// Many issues' timelines, each exactly what [`Store::load_tracker_timeline`]
    /// answers for it, in two reads per [`TIMELINE_IDS_PER_READ`] issues rather
    /// than two per issue.
    ///
    /// What a list reads: a timeline read per listed issue held the app lock
    /// for as long as the project's history was long (#128). Every id asked
    /// for has an entry, empty when nothing answers to it.
    pub fn load_tracker_timelines(
        &self,
        issue_ids: &[String],
    ) -> Result<HashMap<String, Vec<TimelineEntry>>, StoreError> {
        let mut timelines: HashMap<String, Vec<TimelineEntry>> = HashMap::new();
        let mut unique = Vec::with_capacity(issue_ids.len());
        for id in issue_ids {
            if timelines.insert(id.clone(), Vec::new()).is_none() {
                unique.push(id.as_str());
            }
        }
        let conn = self.connection();
        for chunk in unique.chunks(TIMELINE_IDS_PER_READ) {
            // Comments before events, the way the one-issue read gathers
            // them, so the stable sort breaks any tie the same way.
            let comments = read_records_of_issues(&conn, "tracker_comments", chunk)?;
            gather(&mut timelines, comments, TimelineEntry::Comment);
            let events = read_records_of_issues(&conn, "tracker_events", chunk)?;
            gather(&mut timelines, events, TimelineEntry::Event);
        }
        drop(conn);
        for timeline in timelines.values_mut() {
            in_timeline_order(timeline);
        }
        Ok(timelines)
    }

    /// Take a project's whole tracker with it.
    ///
    /// Only reached by project deletion. Nothing else removes an issue: an
    /// issue is closed, so a number is never reused and a timeline never loses
    /// an entry.
    pub fn delete_tracker_issues_of_project(&self, project_path: &str) -> Result<(), StoreError> {
        self.in_transaction(|tx| {
            for table in ["tracker_comments", "tracker_events"] {
                tx.execute(
                    &format!(
                        "DELETE FROM {table} WHERE issue_id IN \
                         (SELECT id FROM tracker_issues WHERE project_key = ?1)"
                    ),
                    [project_path],
                )?;
            }
            tx.execute(
                "DELETE FROM tracker_issues WHERE project_key = ?1",
                [project_path],
            )?;
            Ok(())
        })
    }
}

/// The next number this project hands out. Read inside the caller's write
/// transaction, never before it.
fn next_issue_number(tx: &Transaction, project_path: &str) -> Result<u64, StoreError> {
    let highest: i64 = tx.query_row(
        "SELECT COALESCE(MAX(number), 0) FROM tracker_issues WHERE project_key = ?1",
        [project_path],
        |row| row.get(0),
    )?;
    Ok(highest as u64 + 1)
}

fn write_tracker_issue(tx: &Transaction, issue: &Issue) -> Result<(), StoreError> {
    tx.execute(
        "INSERT INTO tracker_issues
             (id, project_key, number, state, status, created_at, updated_at, record)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(id) DO UPDATE SET
             project_key = ?2, number = ?3, state = ?4, status = ?5,
             created_at = ?6, updated_at = ?7, record = ?8",
        rusqlite::params![
            issue.id,
            issue.project_path,
            issue.number as i64,
            issue.state.as_str(),
            issue.status,
            issue.created_at,
            issue.updated_at,
            serde_json::to_string(issue).expect("an issue always serializes"),
        ],
    )?;
    Ok(())
}

/// Append what was said and what happened. Both are inserts that ignore a
/// repeat of the same id: an append is idempotent, so a retry of a write whose
/// answer was lost adds nothing a second time.
fn append_activity(
    tx: &Transaction,
    comments: &[IssueComment],
    events: &[IssueEvent],
) -> Result<(), StoreError> {
    for comment in comments {
        tx.execute(
            "INSERT OR IGNORE INTO tracker_comments (id, issue_id, created_at, record)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![
                comment.id,
                comment.issue_id,
                comment.created_at,
                serde_json::to_string(comment).expect("a comment always serializes"),
            ],
        )?;
    }
    for event in events {
        tx.execute(
            "INSERT OR IGNORE INTO tracker_events (id, issue_id, at, record)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![
                event.id,
                event.issue_id,
                event.at,
                serde_json::to_string(event).expect("an event always serializes"),
            ],
        )?;
    }
    Ok(())
}

/// Every record of one issue's from one table, in the table's own order.
fn read_records<T: serde::de::DeserializeOwned>(
    conn: &rusqlite::Connection,
    sql: &str,
    issue_id: &str,
    table: &str,
) -> Result<Vec<T>, StoreError> {
    let mut statement = conn.prepare(sql)?;
    let rows: Vec<(String, String)> = statement
        .query_map([issue_id], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<_, _>>()?;
    drop(statement);
    rows.into_iter()
        .map(|(id, raw)| decode(&raw, table, &id))
        .collect()
}

/// Every record of several issues' from one table, each with the issue it is
/// on, in the table's own order.
fn read_records_of_issues<T: serde::de::DeserializeOwned>(
    conn: &rusqlite::Connection,
    table: &str,
    issue_ids: &[&str],
) -> Result<Vec<(String, T)>, StoreError> {
    let marks = vec!["?"; issue_ids.len()].join(", ");
    let sql = format!("SELECT issue_id, id, record FROM {table} WHERE issue_id IN ({marks})");
    let mut statement = conn.prepare(&sql)?;
    let rows: Vec<(String, String, String)> = statement
        .query_map(rusqlite::params_from_iter(issue_ids), |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?
        .collect::<Result<_, _>>()?;
    drop(statement);
    rows.into_iter()
        .map(|(issue_id, id, raw)| Ok((issue_id, decode(&raw, table, &id)?)))
        .collect()
}

/// File each record under the timeline of the issue it is on.
fn gather<T>(
    timelines: &mut HashMap<String, Vec<TimelineEntry>>,
    records: Vec<(String, T)>,
    entry: fn(T) -> TimelineEntry,
) {
    for (issue_id, record) in records {
        timelines.entry(issue_id).or_default().push(entry(record));
    }
}

/// A timeline's one ascending order: when it happened, then the id, which is
/// time-ordered itself so two things stamped in the same second still have one
/// order every reader agrees on. Stable, so the order entries arrive in only
/// decides between two with the same time and the same id.
fn in_timeline_order(entries: &mut [TimelineEntry]) {
    entries.sort_by(|left, right| left.ordering_key().cmp(&right.ordering_key()));
}

/// A stored record, or a corruption that names the row it came from.
///
/// Fail fast, the way every other record here does: a silently dropped issue
/// is work the user filed that Build would then say does not exist.
fn decode<T: serde::de::DeserializeOwned>(
    raw: &str,
    table: &str,
    id: &str,
) -> Result<T, StoreError> {
    serde_json::from_str(raw).map_err(|source| StoreError::Corrupt {
        path: PathBuf::from(format!("{table}/{id}")),
        source,
    })
}

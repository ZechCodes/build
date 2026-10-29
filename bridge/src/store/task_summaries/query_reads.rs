//! Indexed ordering and exact predicates over the maintained query projection.
use super::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskQueryOrder {
    #[default]
    Number,
    Updated,
    Priority,
    Anchor,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TaskQueryPosition {
    pub sort: i64,
    pub number: u64,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct TaskQueryFilter {
    pub watched: Option<bool>,
    pub unfinished: Option<bool>,
    pub blocked: Option<bool>,
    pub held: Option<bool>,
    pub undelivered: Option<bool>,
    pub attention_rule: Option<String>,
    pub linked_workspace_id: Option<String>,
    pub tracker_agent_id: Option<String>,
    pub exclude_review_mode: Option<String>,
    pub done_since: Option<i64>,
    pub exclude_assignee: Option<Value>,
}
impl TaskQueryOrder {
    fn column(self) -> &'static str {
        match self {
            Self::Number => "number",
            Self::Updated => "updated_ms",
            Self::Priority => "priority_rank",
            Self::Anchor => "anchor_ms",
        }
    }
    fn index(self) -> &'static str {
        match self {
            Self::Number => "tracker_task_query_number",
            Self::Updated => "tracker_task_query_updated",
            Self::Priority => "tracker_task_query_priority",
            Self::Anchor => "tracker_task_query_anchor",
        }
    }
}
impl Store {
    pub fn task_query_markers_unsupported(&self, project: &str) -> Result<bool, StoreError> {
        ReadTransaction::new(&self.connection()).task_query_markers_unsupported(project)
    }

    pub fn task_query_unsupported_domain(
        &self,
        project: &str,
        order: TaskQueryOrder,
        done_since: bool,
    ) -> Result<Option<String>, StoreError> {
        ReadTransaction::new(&self.connection())
            .task_query_unsupported_domain(project, order, done_since)
    }

    pub fn task_summary_by_number(
        &self,
        project: &str,
        number: u64,
    ) -> Result<Option<TaskSummaryCandidate>, StoreError> {
        ReadTransaction::new(&self.connection()).task_summary_by_number(project, number)
    }

    pub fn list_task_query_candidates(
        &self,
        project: &str,
        order: TaskQueryOrder,
        after: Option<TaskQueryPosition>,
        limit: usize,
    ) -> Result<Vec<(TaskSummaryCandidate, TaskQueryPosition)>, StoreError> {
        ReadTransaction::new(&self.connection())
            .list_task_query_candidates(project, order, after, limit)
    }
    pub fn task_query_matches(
        &self,
        id: &str,
        filter: &TaskQueryFilter,
    ) -> Result<bool, StoreError> {
        ReadTransaction::new(&self.connection()).task_query_matches(id, filter)
    }
}
impl ReadTransaction<'_> {
    pub fn task_query_markers_unsupported(&self, project: &str) -> Result<bool, StoreError> {
        let sql="SELECT EXISTS(SELECT 1 FROM tracker_task_query_facts INDEXED BY tracker_task_query_invalid_markers WHERE project_key=?1 AND invalid_markers=1)";
        Ok(self
            .connection()
            .query_row(sql, [project], |row| row.get(0))?)
    }

    /// Conservative project-wide validity is a pair of partial-index point
    /// seeks, independent of row filters and of the candidate scan window.
    pub fn task_query_unsupported_domain(
        &self,
        project: &str,
        order: TaskQueryOrder,
        done_since: bool,
    ) -> Result<Option<String>, StoreError> {
        let required = match order {
            TaskQueryOrder::Updated => Some(("updated", "updated_at")),
            TaskQueryOrder::Anchor => Some(("anchor", "anchor_fallback")),
            _ => None,
        };
        if let Some((flag, field)) = required {
            if self.task_query_has_invalid(project, flag)? {
                return Ok(Some(field.into()));
            }
        }
        if done_since && self.task_query_has_invalid(project, "done")? {
            return Ok(Some("done_at".into()));
        }
        Ok(None)
    }
    fn task_query_has_invalid(&self, project: &str, flag: &str) -> Result<bool, StoreError> {
        // flag is one of the three fixed internal literals above, never input.
        let sql=format!("SELECT EXISTS(SELECT 1 FROM tracker_task_query_facts INDEXED BY tracker_task_query_invalid_{flag} WHERE project_key=?1 AND invalid_{flag}=1)");
        Ok(self
            .connection()
            .query_row(&sql, [project], |row| row.get(0))?)
    }

    pub fn task_summary_by_number(
        &self,
        project: &str,
        number: u64,
    ) -> Result<Option<TaskSummaryCandidate>, StoreError> {
        let Some(number_sql) = i64::try_from(number).ok() else {
            return Ok(None);
        };
        let raw = self.connection().query_row("SELECT s.record,s.assignment_target,s.identities,q.assignee_kind,CASE WHEN length(CAST(q.assignee_agent_id AS BLOB))<=128 THEN q.assignee_agent_id END,q.status,q.labels_json,q.status_truncated,q.creator_kind,CASE WHEN length(CAST(q.creator_agent_id AS BLOB))<=128 THEN q.creator_agent_id END,q.read_through,q.watch_started_after,q.dismissed_through FROM tracker_task_summaries s LEFT JOIN tracker_task_query_sources q ON q.task_id=s.task_id WHERE s.project_key=?1 AND s.number=?2",params![project,number_sql],|row|Ok((row.get::<_,String>(0)?,row.get::<_,Option<String>>(1)?,row.get::<_,String>(2)?,CanonicalSelection {kind:row.get(3)?,agent:row.get(4)?,status:row.get(5)?,labels:row.get(6)?,status_truncated:row.get(7)?,creator_kind:row.get(8)?,creator_agent:row.get(9)?,read_through:row.get(10)?,watch_started_after:row.get(11)?,dismissed_through:row.get(12)?}))).optional()?;
        raw.map(|(raw, target, identities, selection)| {
            selection.apply(decode_summary_candidate(raw, target, identities, project)?)
        })
        .transpose()
    }

    pub fn list_task_query_candidates(
        &self,
        project: &str,
        order: TaskQueryOrder,
        after: Option<TaskQueryPosition>,
        limit: usize,
    ) -> Result<Vec<(TaskSummaryCandidate, TaskQueryPosition)>, StoreError> {
        let limit = limit.min(MAX_SUMMARY_CANDIDATES);
        let positions = self.query_positions(project, order, after, limit)?;
        positions
            .into_iter()
            .map(|position| {
                let candidate = self
                    .task_summary_by_number(project, position.number)?
                    .ok_or_else(|| {
                        StoreError::TaskSummaryInvalid("query projection has no summary".into())
                    })?;
                if candidate.row["number"].as_u64() != Some(position.number) {
                    return Err(StoreError::TaskSummaryInvalid(
                        "query projection number mismatch".into(),
                    ));
                }
                Ok((candidate, position))
            })
            .collect()
    }
    fn query_positions(
        &self,
        project: &str,
        order: TaskQueryOrder,
        after: Option<TaskQueryPosition>,
        limit: usize,
    ) -> Result<Vec<TaskQueryPosition>, StoreError> {
        let column = order.column();
        let direction = if order == TaskQueryOrder::Anchor {
            "ASC"
        } else {
            "DESC"
        };
        let compare = if order == TaskQueryOrder::Anchor {
            ">"
        } else {
            "<"
        };
        // Split the mixed-direction seek into two explicit index ranges. Each
        // range reads at most the requested number; tied groups never rescan.
        let mut positions = Vec::new();
        if let Some(after) = after.filter(|_| order != TaskQueryOrder::Number) {
            let sql = format!("SELECT {column},number FROM tracker_task_query_facts INDEXED BY {} WHERE project_key=?1 AND {column}=?2 AND number<?3 ORDER BY number DESC LIMIT ?4", order.index());
            let mut statement = self.connection().prepare(&sql)?;
            let rows = statement.query_map(
                params![project, after.sort, after.number as i64, limit as i64],
                position_row,
            )?;
            positions = rows.collect::<Result<Vec<_>, _>>()?;
        }
        if positions.len() < limit {
            let predicate = after
                .map(|_| format!(" AND {column}{compare}?2"))
                .unwrap_or_default();
            let tie = if order == TaskQueryOrder::Number {
                ""
            } else {
                ",number DESC"
            };
            let sql = format!("SELECT {column},number FROM tracker_task_query_facts INDEXED BY {} WHERE project_key=?1{predicate} ORDER BY {column} {direction}{tie} LIMIT ?3", order.index());
            let mut statement = self.connection().prepare(&sql)?;
            let rows = statement.query_map(
                params![
                    project,
                    after.map(|p| p.sort),
                    (limit - positions.len()) as i64
                ],
                position_row,
            )?;
            positions.extend(rows.collect::<Result<Vec<_>, _>>()?);
        }
        Ok(positions)
    }
    pub fn task_query_matches(
        &self,
        id: &str,
        filter: &TaskQueryFilter,
    ) -> Result<bool, StoreError> {
        let attention = filter.attention_rule.as_deref();
        let sql = "SELECT (?2 IS NULL OR watched=?2) AND (?3 IS NULL OR unfinished=?3) AND (?4 IS NULL OR blocked=?4) AND (?5 IS NULL OR held=?5) AND (?6 IS NULL OR undelivered=?6) AND (?7 IS NULL OR CASE ?7 WHEN 'asked' THEN attention_asked ELSE attention_legacy END) AND (?8 IS NULL OR EXISTS(SELECT 1 FROM tracker_task_query_memberships m WHERE m.task_id=f.task_id AND m.kind='workspace' AND m.value=?8)) AND (?9 IS NULL OR EXISTS(SELECT 1 FROM tracker_task_query_memberships m WHERE m.task_id=f.task_id AND m.kind='tracker' AND m.value=?9)) AND (?10 IS NULL OR review_mode IS NULL OR review_mode<>?10) AND (?11 IS NULL OR done_ms>=?11) AND (?12 IS NULL OR NOT(assignee_kind IS ?12 AND (?12<>'agent' OR assignee_agent_id IS ?13))) FROM tracker_task_query_facts f WHERE task_id=?1";
        Ok(self
            .connection()
            .query_row(
                sql,
                params![
                    id,
                    filter.watched,
                    filter.unfinished,
                    filter.blocked,
                    filter.held,
                    filter.undelivered,
                    attention,
                    filter.linked_workspace_id,
                    filter.tracker_agent_id,
                    filter.exclude_review_mode,
                    filter.done_since,
                    filter
                        .exclude_assignee
                        .as_ref()
                        .and_then(|value| value["kind"].as_str()),
                    filter
                        .exclude_assignee
                        .as_ref()
                        .and_then(|value| value["agent_id"].as_str())
                ],
                |row| row.get::<_, Option<bool>>(0),
            )?
            .unwrap_or(false))
    }
}
struct CanonicalSelection {
    kind: Option<String>,
    agent: Option<String>,
    status: Option<String>,
    labels: Option<String>,
    status_truncated: Option<bool>,
    creator_kind: Option<String>,
    creator_agent: Option<String>,
    read_through: Option<String>,
    watch_started_after: Option<String>,
    dismissed_through: Option<String>,
}
impl CanonicalSelection {
    fn apply(
        self,
        mut candidate: TaskSummaryCandidate,
    ) -> Result<TaskSummaryCandidate, StoreError> {
        let missing = || {
            StoreError::TaskSummaryInvalid(
                "query projection is missing canonical selection inputs".into(),
            )
        };
        candidate.row["assignee"] = match self.kind.as_deref() {
            None => Value::Null,
            Some("agent") => {
                serde_json::json!({"kind":"agent","agent_id":self.agent.ok_or_else(missing)?})
            }
            Some("user" | "project_agent") => serde_json::json!({"kind":self.kind}),
            _ => return Err(missing()),
        };
        candidate.row["created_by"] = match self.creator_kind.as_deref() {
            Some("agent") => {
                serde_json::json!({"kind":"agent","agent_id":self.creator_agent.ok_or_else(missing)?})
            }
            Some("user" | "build") => serde_json::json!({"kind":self.creator_kind}),
            _ => return Err(missing()),
        };
        for (field, value) in [
            ("read_through", self.read_through),
            ("watch_started_after", self.watch_started_after),
            ("dismissed_through", self.dismissed_through),
        ] {
            candidate.row[field] = value.map(Value::String).unwrap_or(Value::Null);
        }
        candidate.row["status"] = Value::String(self.status.ok_or_else(missing)?);
        let labels: Vec<String> = super::super::tracker::decode(
            &self.labels.ok_or_else(missing)?,
            "tracker_task_query_sources/labels",
            "canonical",
        )?;
        candidate.row["labels"] = serde_json::json!(labels);
        if self.status_truncated.ok_or_else(missing)? {
            candidate.row["summary_truncated"] = Value::Bool(true);
        }
        Ok(candidate)
    }
}
fn position_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TaskQueryPosition> {
    Ok(TaskQueryPosition {
        sort: row.get(0)?,
        number: row.get::<_, i64>(1)? as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch("CREATE TABLE meta(key TEXT PRIMARY KEY,value TEXT); CREATE TABLE tracker_task_summary_facts(task_id TEXT,ulid TEXT,meaningful INTEGER);").unwrap();
        connection
            .execute_batch(super::super::query_projection::TABLES)
            .unwrap();
        for (number, sort) in [(1, 10), (2, 10), (3, 20), (4, i64::MAX)] {
            connection.execute("INSERT INTO tracker_task_query_facts(task_id,project_key,number,updated_ms,anchor_ms,priority_rank,done_ms,unfinished,watched,attention_asked,attention_legacy,blocked,held,undelivered,review_mode,assignee_agent_id,unread_total,contribution_agent) VALUES(?1,'p',?2,?3,?3,?2,NULL,1,1,1,0,0,0,0,NULL,NULL,0,NULL)",params![format!("t{number}"),number,sort]).unwrap();
        }
        connection
    }
    #[test]
    fn project_timestamp_gate_uses_partial_indexes_and_ignores_unrelated_domains() {
        let connection = fixture();
        connection.execute("UPDATE tracker_task_query_facts SET invalid_updated=1,invalid_anchor=1,invalid_done=1 WHERE task_id='t1'",[]).unwrap();
        let source = ReadTransaction::new(&connection);
        for (order, cutoff, want) in [
            (TaskQueryOrder::Updated, false, Some("updated_at")),
            (TaskQueryOrder::Anchor, false, Some("anchor_fallback")),
            (TaskQueryOrder::Number, true, Some("done_at")),
            (TaskQueryOrder::Priority, true, Some("done_at")),
            (TaskQueryOrder::Number, false, None),
            (TaskQueryOrder::Priority, false, None),
        ] {
            assert_eq!(
                source
                    .task_query_unsupported_domain("p", order, cutoff)
                    .unwrap()
                    .as_deref(),
                want
            );
            assert_eq!(
                source
                    .task_query_unsupported_domain("other", order, cutoff)
                    .unwrap(),
                None
            );
        }
        for flag in ["updated", "anchor", "done"] {
            let sql=format!("EXPLAIN QUERY PLAN SELECT EXISTS(SELECT 1 FROM tracker_task_query_facts INDEXED BY tracker_task_query_invalid_{flag} WHERE project_key=?1 AND invalid_{flag}=1)");
            let mut statement = connection.prepare(&sql).unwrap();
            let detail = statement
                .query_map(["p"], |row| row.get::<_, String>(3))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
                .join(";");
            assert!(
                detail.contains("SEARCH tracker_task_query_facts USING")
                    && detail.contains(&format!(
                        "INDEX tracker_task_query_invalid_{flag} (project_key=?)"
                    )),
                "{detail}"
            );
            assert!(
                !detail.contains("SCAN tracker_task_query_facts"),
                "{detail}"
            );
        }
        connection.execute_batch("BEGIN; UPDATE tracker_task_query_facts SET invalid_updated=0,invalid_anchor=0,invalid_done=0 WHERE task_id='t1';").unwrap();
        assert_eq!(
            source
                .task_query_unsupported_domain("p", TaskQueryOrder::Updated, true)
                .unwrap(),
            None
        );
        connection.execute_batch("ROLLBACK;").unwrap();
        assert_eq!(
            source
                .task_query_unsupported_domain("p", TaskQueryOrder::Updated, true)
                .unwrap()
                .as_deref(),
            Some("updated_at")
        );
        connection
            .execute(
                "DELETE FROM tracker_task_query_facts WHERE task_id='t1'",
                [],
            )
            .unwrap();
        assert_eq!(
            source
                .task_query_unsupported_domain("p", TaskQueryOrder::Anchor, true)
                .unwrap(),
            None
        );
    }
    #[test]
    fn point_lookup_never_decodes_a_neighbor_and_supports_max_number() {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch("CREATE TABLE tracker_task_summaries(task_id TEXT,project_key TEXT,number INTEGER,record TEXT,assignment_target TEXT,identities TEXT,PRIMARY KEY(project_key,number)); CREATE TABLE tracker_task_query_sources(task_id TEXT PRIMARY KEY,assignee_kind TEXT,assignee_agent_id TEXT,status TEXT,labels_json TEXT,status_truncated INTEGER,creator_kind TEXT,creator_agent_id TEXT,read_through TEXT,watch_started_after TEXT,dismissed_through TEXT); INSERT INTO tracker_task_summaries VALUES('bad','p',1,'malformed',NULL,'{}'); INSERT INTO tracker_task_query_sources VALUES('max',NULL,NULL,'backlog','[]',0,'user',NULL,NULL,NULL,NULL);").unwrap();
        connection
            .execute(
                "INSERT INTO tracker_task_summaries VALUES('max','p',?1,?2,NULL,'{}')",
                params![
                    i64::MAX,
                    serde_json::json!({"number":i64::MAX,"id":"max"}).to_string()
                ],
            )
            .unwrap();
        let source = ReadTransaction::new(&connection);
        assert!(source.task_summary_by_number("p", 2).unwrap().is_none());
        assert_eq!(
            source
                .task_summary_by_number("p", i64::MAX as u64)
                .unwrap()
                .unwrap()
                .row["id"],
            "max"
        );
        assert!(source
            .task_summary_by_number("p", u64::MAX)
            .unwrap()
            .is_none());
    }
    #[test]
    fn seeks_each_direction_and_tied_group_without_duplicates() {
        let connection = fixture();
        let source = ReadTransaction::new(&connection);
        for (order, want) in [
            (TaskQueryOrder::Number, vec![4, 3, 2, 1]),
            (TaskQueryOrder::Updated, vec![4, 3, 2, 1]),
            (TaskQueryOrder::Priority, vec![4, 3, 2, 1]),
            (TaskQueryOrder::Anchor, vec![2, 1, 3, 4]),
        ] {
            let mut after = None;
            let mut seen = Vec::new();
            loop {
                let page = source.query_positions("p", order, after, 1).unwrap();
                if page.is_empty() {
                    break;
                }
                seen.push(page[0].number);
                after = Some(page[0]);
            }
            assert_eq!(seen, want);
        }
    }
    #[test]
    fn exact_membership_and_attention_are_independent_of_display() {
        let connection = fixture();
        connection.execute("INSERT INTO tracker_task_query_memberships VALUES('t1','workspace','WorkspaceA','p',1)",[]).unwrap();
        connection
            .execute(
                "INSERT INTO tracker_task_query_memberships VALUES('t1','tracker','AgentB','p',1)",
                [],
            )
            .unwrap();
        let source = ReadTransaction::new(&connection);
        let mut filter = TaskQueryFilter {
            linked_workspace_id: Some("WorkspaceA".into()),
            tracker_agent_id: Some("AgentB".into()),
            attention_rule: Some("asked".into()),
            ..Default::default()
        };
        assert!(source.task_query_matches("t1", &filter).unwrap());
        assert!(!source.task_query_matches("t2", &filter).unwrap());
        filter.attention_rule = Some("legacy".into());
        assert!(!source.task_query_matches("t1", &filter).unwrap());
        filter.attention_rule = None;
        filter.linked_workspace_id = Some("workspacea".into());
        assert!(!source.task_query_matches("t1", &filter).unwrap());
    }
}

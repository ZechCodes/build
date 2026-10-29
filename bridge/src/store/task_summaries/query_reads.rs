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
    pub fn task_summary_by_number(
        &self,
        project: &str,
        number: u64,
    ) -> Result<Option<TaskSummaryCandidate>, StoreError> {
        let Some(number_sql) = i64::try_from(number).ok() else {
            return Ok(None);
        };
        let raw = self.connection().query_row("SELECT record,assignment_target,identities FROM tracker_task_summaries WHERE project_key=?1 AND number=?2",params![project,number_sql],|row|Ok((row.get::<_,String>(0)?,row.get::<_,Option<String>>(1)?,row.get::<_,String>(2)?))).optional()?;
        raw.map(|(raw, target, identities)| {
            decode_summary_candidate(raw, target, identities, project)
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
        let sql = "SELECT (?2 IS NULL OR watched=?2) AND (?3 IS NULL OR unfinished=?3) AND (?4 IS NULL OR blocked=?4) AND (?5 IS NULL OR held=?5) AND (?6 IS NULL OR undelivered=?6) AND (?7 IS NULL OR CASE ?7 WHEN 'asked' THEN attention_asked ELSE attention_legacy END) AND (?8 IS NULL OR EXISTS(SELECT 1 FROM tracker_task_query_memberships m WHERE m.task_id=f.task_id AND m.kind='workspace' AND m.value=?8)) AND (?9 IS NULL OR EXISTS(SELECT 1 FROM tracker_task_query_memberships m WHERE m.task_id=f.task_id AND m.kind='tracker' AND m.value=?9)) AND (?10 IS NULL OR review_mode IS NULL OR review_mode<>?10) AND (?11 IS NULL OR done_ms>=?11) FROM tracker_task_query_facts f WHERE task_id=?1";
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
                    filter.done_since
                ],
                |row| row.get::<_, Option<bool>>(0),
            )?
            .unwrap_or(false))
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
            connection.execute("INSERT INTO tracker_task_query_facts VALUES(?1,'p',?2,?3,?3,?2,NULL,1,1,1,0,0,0,0,NULL,NULL,0,NULL)",params![format!("t{number}"),number,sort]).unwrap();
        }
        connection
    }
    #[test]
    fn point_lookup_never_decodes_a_neighbor_and_supports_max_number() {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch("CREATE TABLE tracker_task_summaries(project_key TEXT,number INTEGER,record TEXT,assignment_target TEXT,identities TEXT,PRIMARY KEY(project_key,number)); INSERT INTO tracker_task_summaries VALUES('p',1,'malformed',NULL,'{}');").unwrap();
        connection
            .execute(
                "INSERT INTO tracker_task_summaries VALUES('p',?1,?2,NULL,'{}')",
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

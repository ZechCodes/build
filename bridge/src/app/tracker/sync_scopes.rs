//! Pure bounded task recovery. The execution adapter captures authority first,
//! then runs these methods on ReadPool SyncRecovery without the application.
use super::{query_selections::QuerySelections, selections::TaskSelections};
use crate::store::{
    sync::{
        Entity, Head, HeadResult, ResyncReason, Scope, Stream, SyncError, MAX_RECOVERY_BYTES,
        MAX_RECOVERY_EVENTS,
    },
    ReadTransaction, TaskSummaryCandidate,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;

mod changes;
mod snapshot;

const MAX_TASK_ROW_BYTES: usize = 65536;

/// Project keys and project-id mappings come from the server's published
/// authorization context, never from client filter fields or a live app lock.
pub struct TaskSyncScope {
    scope: Scope,
    definition: Value,
    project: String,
    project_ids: Arc<BTreeMap<String, String>>,
    task: Option<String>,
    selected: TaskSelections,
    query: QuerySelections,
}

impl TaskSyncScope {
    pub fn project(project_id: &str, project_key: &str, filters: &Value) -> Result<Self, String> {
        validate_fields(filters)?;
        let selected = TaskSelections::parse(filters)?;
        let query = QuerySelections::parse(filters)?;
        let normalized = json!([project_id, selected.canonical(), query.canonical()]);
        let scope = Scope::wire_bound(
            Stream::project(project_key),
            normalized,
            (!project_key.is_empty()).then_some(project_key),
        );
        let mut canonical = serde_json::to_value(&query.filter).map_err(|e| e.to_string())?;
        canonical
            .as_object_mut()
            .expect("filter object")
            .retain(|_, v| !v.is_null());
        for (field, values) in ["states", "statuses", "labels", "assignees"]
            .into_iter()
            .zip(selected.canonical().as_array().expect("selections array"))
        {
            if values.as_array().is_some_and(|v| !v.is_empty()) {
                canonical[field] = values.clone();
            }
        }
        Ok(Self {
            scope,
            definition: json!({"kind":"project_tasks","project_id":project_id,"filters":canonical,"order":query.order}),
            project: project_key.into(),
            project_ids: Arc::new(BTreeMap::from([(project_key.into(), project_id.into())])),
            task: None,
            selected,
            query,
        })
    }
    pub fn task(task_id: &str, project_id: &str, project_key: &str) -> Self {
        let definition = json!({"kind":"task","task_id":task_id});
        Self {
            scope: Scope::wire_bound(
                Stream::task(task_id),
                json!([task_id]),
                (!project_key.is_empty()).then_some(project_key),
            ),
            definition,
            project: project_key.into(),
            project_ids: Arc::new(BTreeMap::from([(project_key.into(), project_id.into())])),
            task: Some(task_id.into()),
            selected: TaskSelections::parse(&json!({})).expect("empty selection"),
            query: QuerySelections::parse(&json!({})).expect("empty query"),
        }
    }
    /// Resolve a current row that moved between projects with the same captured
    /// authorization directory. Missing context refuses; it never exposes paths.
    pub fn with_project_ids(mut self, ids: Arc<BTreeMap<String, String>>) -> Self {
        self.project_ids = ids;
        self
    }
    pub fn scope(&self) -> &Scope {
        &self.scope
    }

    pub fn head(
        &self,
        read: &ReadTransaction<'_>,
        cursor: Option<&str>,
        epoch: &str,
        now: i64,
    ) -> Result<Value, SyncError> {
        let head = match self.checked_head(read, cursor, epoch, now)? {
            Ok(head) => head,
            Err(reset) => return Ok(reset),
        };
        self.ensure_supported(read)?;
        Ok(json!({
            "status":"ok",
            "scope":self.definition,
            "incarnation":head.incarnation,
            "head":head.head,
            "retention_floor":head.retention_floor,
            "bridge_epoch":head.bridge_epoch,
        }))
    }

    pub fn removed_head(
        &self,
        read: &ReadTransaction<'_>,
        cursor: &str,
        epoch: &str,
        now: i64,
    ) -> Result<Value, SyncError> {
        let position = read.sync_cursor_sequence(&self.scope, cursor)?;
        let head = match self.checked_head(read, None, epoch, now)? {
            Ok(head) => head,
            Err(reset) => return Ok(reset),
        };
        let reason = match position {
            crate::store::sync::CursorSequence::Foreign => ResyncReason::IncarnationChanged,
            crate::store::sync::CursorSequence::Rebound(_) => ResyncReason::ScopeRemoved,
            crate::store::sync::CursorSequence::Current(sequence) => {
                if sequence > head.head {
                    return Err(SyncError::FutureCursor);
                }
                ResyncReason::ScopeRemoved
            }
        };
        Ok(self.resync(&head, reason))
    }

    fn ensure_supported(&self, read: &ReadTransaction<'_>) -> Result<(), SyncError> {
        if self.task.is_some() {
            return Ok(());
        }
        if let Some(domain) = read.task_query_unsupported_domain(
            &self.project,
            self.query.order,
            self.query.filter.done_since.is_some(),
        )? {
            return Err(SyncError::Unavailable(format!(
                "unsupported task query timestamp: {domain}"
            )));
        }
        if read.task_query_markers_unsupported(&self.project)? {
            return Err(SyncError::Unavailable(
                "unsupported opaque task marker".into(),
            ));
        }
        Ok(())
    }

    fn member(
        &self,
        read: &ReadTransaction<'_>,
        project: &str,
        row: &Value,
        pending: bool,
    ) -> Result<bool, SyncError> {
        let canonical_pending = pending.then_some(&Value::Bool(true));
        Ok(project == self.project
            && self.selected.matches_row(row, canonical_pending)
            && self.query.matches_row(row)
            && (!self.query.needs_index_match()
                || read.task_query_matches(
                    row["id"]
                        .as_str()
                        .ok_or_else(|| SyncError::Unavailable("missing task id".into()))?,
                    &self.query.filter,
                )?))
    }
    fn record(&self, read: &ReadTransaction<'_>, id: &str) -> Result<Value, SyncError> {
        let kind = if self.task.is_some() {
            "task_head"
        } else {
            "task"
        };
        let Some((project, candidate, pending, revision)) = read.sync_task_summary(id)? else {
            let revision = read.sync_revision(&Entity::new("task", id))?;
            return Ok(json!({"kind":kind,"key":id,"revision":revision,"row":null,"member":false}));
        };
        self.record_candidate(read, id, &project, candidate, pending, revision)
    }
    fn record_candidate(
        &self,
        read: &ReadTransaction<'_>,
        id: &str,
        project: &str,
        mut candidate: TaskSummaryCandidate,
        pending: bool,
        revision: crate::store::sync::Sequence,
    ) -> Result<Value, SyncError> {
        let kind = if self.task.is_some() {
            "task_head"
        } else {
            "task"
        };
        let project_id = self.project_ids.get(project).ok_or_else(|| {
            SyncError::Unavailable("project authority context changed; retry sync".into())
        })?;
        candidate.row["project_id"] = json!(project_id);
        if self.query.order == crate::store::TaskQueryOrder::Anchor {
            candidate.row["order_anchor_ms"] = json!(read.sync_task_anchor_ms(id)?);
        }
        let member = self.task.is_some() || self.member(read, project, &candidate.row, pending)?;
        let dto: crate::api::v1::tasks::TaskSummaryView = serde_json::from_value(candidate.row)
            .map_err(|e| SyncError::Unavailable(format!("invalid task summary: {e}")))?;
        let summary =
            serde_json::to_value(dto).map_err(|e| SyncError::Unavailable(e.to_string()))?;
        let row = if self.task.is_some() {
            let (timeline, more) = read.sync_task_timeline_head(id)?;
            json!({"task":summary,"timeline":timeline,"has_more":more})
        } else {
            summary
        };
        let record = json!({"kind":kind,"key":id,"revision":revision,"row":row,"member":member,"selection":{"pending_present":pending}});
        let max = if self.task.is_some() {
            MAX_RECOVERY_BYTES - 8192
        } else {
            MAX_TASK_ROW_BYTES
        };
        if encoded_size(&record)? > max {
            return Err(SyncError::Unavailable(
                "task recovery record exceeds its byte bound".into(),
            ));
        }
        Ok(record)
    }
    fn resync(&self, head: &Head, reason: ResyncReason) -> Value {
        json!({"status":"resync_required","scope":self.definition,"incarnation":head.incarnation,"head":head.head,"bridge_epoch":head.bridge_epoch,"reason":reason})
    }
    fn checked_head(
        &self,
        read: &ReadTransaction<'_>,
        cursor: Option<&str>,
        epoch: &str,
        now: i64,
    ) -> Result<Result<Head, Value>, SyncError> {
        match read.sync_head(&self.scope, cursor, epoch, now)? {
            HeadResult::Ready(head) => Ok(Ok(head)),
            HeadResult::ResyncRequired {
                reason,
                incarnation,
                head,
                bridge_epoch,
            } => Ok(Err(
                json!({"status":"resync_required","scope":self.definition,"reason":reason,"incarnation":incarnation,"head":head,"bridge_epoch":bridge_epoch}),
            )),
        }
    }
}
fn encoded_size(value: &Value) -> Result<usize, SyncError> {
    serde_json::to_vec(value)
        .map(|v| v.len())
        .map_err(|e| SyncError::Unavailable(e.to_string()))
}
fn validate_fields(filters: &Value) -> Result<(), String> {
    let fields = [
        "state",
        "states",
        "status",
        "statuses",
        "label",
        "labels",
        "assignee",
        "assignees",
        "order",
        "watched",
        "unfinished",
        "blocked",
        "held",
        "undelivered",
        "attention_rule",
        "linked_workspace_id",
        "tracker_agent_id",
        "exclude_review_mode",
        "done_since",
        "exclude_assignee",
    ];
    let object = filters
        .as_object()
        .ok_or("task filters must be an object")?;
    if object.keys().any(|key| !fields.contains(&key.as_str())) {
        return Err("unknown task sync filter".into());
    }
    Ok(())
}

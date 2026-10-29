//! Point reads over transactional projections; no catalogue or history scan.
use super::read::TaskReadContext;
use super::read_source::TaskReadSource;
use super::StoredAnswer;
use crate::app::{require_str, AppState};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

impl AppState {
    pub(crate) fn tasks_counts(&self, params: &Value) -> Result<Value, String> {
        self.tracker_store()?
            .read_task_projection(|read| self.task_read_context().tasks_counts(read, params))
    }

    pub(crate) fn tasks_lookup(&self, params: &Value) -> Result<Value, String> {
        self.tracker_store()?
            .read_task_projection(|read| self.task_read_context().tasks_lookup(read, params))
    }
}

impl TaskReadContext {
    pub(in crate::app) fn tasks_counts(
        &self,
        source: &impl TaskReadSource,
        params: &Value,
    ) -> Result<Value, String> {
        let project_id = require_str(params, "project_id")?;
        let project = self
            .project_paths
            .get(&project_id)
            .ok_or_else(|| format!("unknown project_id: {project_id}"))?;
        let agents = agent_buckets(params)?;
        let basis = source.task_query_basis().stored()?;
        super::query_selections::ensure_supported_markers(
            source,
            project,
            &project_id,
            "tasks.counts",
        )?;
        let counts = source.task_counts(project, &agents).stored()?;
        let project_count = counts.first().ok_or("missing project aggregate")?;
        let buckets: Vec<_> = agents
            .iter()
            .zip(counts.iter().skip(1))
            .map(|(agent, count)| {
                json!({"agent_id":agent,"unfinished_open_count":count.unfinished_open_count,
                "watched_unread_total":count.watched_unread_total})
            })
            .collect();
        bounded_answer(json!({"project_id":project_id,"basis":basis,
            "unfinished_open_count":project_count.unfinished_open_count,
            "watched_unread_total":project_count.watched_unread_total,"agents":buckets}))
    }

    pub(in crate::app) fn tasks_lookup(
        &self,
        source: &impl TaskReadSource,
        params: &Value,
    ) -> Result<Value, String> {
        let project_id = require_str(params, "project_id")?;
        let project = self
            .project_paths
            .get(&project_id)
            .ok_or_else(|| format!("unknown project_id: {project_id}"))?;
        let number = params["number"]
            .as_u64()
            .filter(|n| *n > 0 && *n <= i64::MAX as u64)
            .ok_or("number must be a positive task number")?;
        let basis = source.task_query_basis().stored()?;
        let mut identities = BTreeMap::new();
        let task = match source.task_summary_by_number(project, number).stored()? {
            Some(mut candidate) => {
                self.prepare_summary(source, &project_id, &mut candidate, &identities)?;
                identities = candidate.identities;
                candidate.row
            }
            None => Value::Null,
        };
        bounded_answer(json!({"project_id":project_id,"number":number,"task":task,
            "identities":identities,"basis":basis}))
    }
}

fn agent_buckets(params: &Value) -> Result<Vec<String>, String> {
    let Some(value) = params.get("agent_ids") else {
        return Ok(Vec::new());
    };
    let values = value
        .as_array()
        .filter(|values| values.len() <= 100)
        .ok_or("agent_ids must contain at most 100 IDs")?;
    let agents: Result<BTreeSet<_>, _> = values
        .iter()
        .map(|value| {
            value
                .as_str()
                .filter(|id| !id.is_empty() && id.len() <= 128)
                .map(str::to_string)
                .ok_or("agent IDs must contain 1 to 128 bytes")
        })
        .collect();
    Ok(agents?.into_iter().collect())
}

fn bounded_answer(answer: Value) -> Result<Value, String> {
    if serde_json::to_vec(&answer)
        .map_err(|error| error.to_string())?
        .len()
        > 65_536
    {
        return Err("task point response exceeds 65536 bytes; repair the stored projection".into());
    }
    Ok(answer)
}

//! Lightweight reads touch only bounded materialized rows and shared identities.
use super::bounded_pages::{BoundedPage, MAX_RESPONSE_BYTES, MAX_ROW_BYTES, RESPONSE_OVERHEAD};
use super::read::TaskReadContext;
use super::read_source::TaskReadSource;
use super::selections::TaskSelections;
use super::StoredAnswer;
use crate::app::require_str;
use crate::store::TaskSummaryCandidate;
use serde_json::{json, Value};
use std::collections::BTreeMap;

impl TaskReadContext {
    pub(in crate::app) fn tasks_list_summary(
        &self,
        source: &impl TaskReadSource,
        params: &Value,
    ) -> Result<Value, String> {
        let project_id = require_str(params, "project_id")?;
        let project = self
            .project_paths
            .get(&project_id)
            .ok_or_else(|| format!("unknown project_id: {project_id}"))?;
        let filters = TaskSelections::parse(params)?;
        let key = source.tracker_list_key().stored()?;
        let (page, position) = BoundedPage::parse(
            params,
            &key,
            project,
            json!(["summary", filters.canonical()]),
        )?;
        let below = position
            .map(|position| {
                position
                    .parse::<u64>()
                    .map_err(|_| "invalid task cursor".to_string())
            })
            .transpose()?;
        let mut scan = SummaryScan::new(page.limit, below);
        self.scan_summaries(source, project, &project_id, &filters, &mut scan)?;
        let next = scan
            .more
            .then(|| scan.below.map(|number| page.cursor(&number.to_string())))
            .flatten();
        let rows = scan.rows;
        let mut answer =
            json!({"summary":true,"project_id":project_id,"user_session":self.user_session_json()});
        // Move the already-decoded page instead of serializing every row into
        // a second Value tree while constructing the response envelope.
        answer["tasks"] = Value::Array(rows.tasks);
        answer["identities"] = Value::Object(rows.identities.into_iter().collect());
        if let Some(cursor) = next {
            answer["next_cursor"] = json!(cursor);
        }
        ensure_response_bound(&answer)?;
        Ok(answer)
    }

    fn scan_summaries(
        &self,
        source: &impl TaskReadSource,
        project: &str,
        project_id: &str,
        filters: &TaskSelections,
        scan: &mut SummaryScan,
    ) -> Result<(), String> {
        'pages: loop {
            let take = (scan.budget + 1 - scan.scanned)
                .min(32)
                .min(scan.limit + 1 - scan.rows.tasks.len());
            let candidates = source
                .list_task_summary_candidates(project, scan.below, take)
                .stored()?;
            let exhausted = candidates.len() < take;
            for mut candidate in candidates {
                if !scan.admits() {
                    scan.more = true;
                    break 'pages;
                }
                let number = candidate.row["number"]
                    .as_u64()
                    .ok_or("invalid task summary number")?;
                if filters.matches_row(&candidate.row, candidate.assignment_target.as_ref()) {
                    self.prepare_summary(
                        source,
                        project_id,
                        &mut candidate,
                        &scan.rows.identities,
                    )?;
                    if !scan.rows.push(candidate)? {
                        scan.more = true;
                        break 'pages;
                    }
                }
                scan.scanned += 1;
                scan.below = Some(number);
            }
            if exhausted {
                break;
            }
        }
        Ok(())
    }

    fn prepare_summary(
        &self,
        source: &impl TaskReadSource,
        project: &str,
        candidate: &mut TaskSummaryCandidate,
        known: &BTreeMap<String, Value>,
    ) -> Result<(), String> {
        candidate.row["project_id"] = json!(project);
        let mut identities = BTreeMap::new();
        let references: std::collections::BTreeSet<&str> = ["created_by", "assignee"]
            .into_iter()
            .filter_map(|field| candidate.row[field]["agent_id"].as_str())
            .collect();
        for id in references {
            if let Some(value) = known.get(id) {
                identities.insert(id.to_string(), value.clone());
                continue;
            }
            let retained = candidate
                .identities
                .get(id)
                .map(|value| serde_json::from_value(value.clone()))
                .transpose()
                .map_err(|error| format!("invalid summary identity: {error}"))?
                .unwrap_or_else(|| super::read_identities::unknown_identity(id));
            let durable = source
                .task_summary_identity_with_owner(id)
                .stored()?
                .map(|(current, owner_path)| {
                    let current = self
                        .identities
                        .committed_identity(current, owner_path.as_deref());
                    super::identities::retain_known_fields(&retained, current)
                })
                .unwrap_or(retained);
            let mut identity = self.identities.identity(id, &durable);
            // Display names can change in the live overlay. IDs stay exact.
            identity.workspace_name = identity
                .workspace_name
                .map(|name| name.chars().take(128).collect());
            identities.insert(
                id.to_string(),
                serde_json::to_value(identity).map_err(|error| error.to_string())?,
            );
        }
        candidate.identities = identities;
        Ok(())
    }

    pub(in crate::app) fn tasks_filter_choices(
        &self,
        source: &impl TaskReadSource,
        params: &Value,
    ) -> Result<Value, String> {
        let project_id = require_str(params, "project_id")?;
        let project = self
            .project_paths
            .get(&project_id)
            .ok_or_else(|| format!("unknown project_id: {project_id}"))?;
        let dimension = require_str(params, "dimension")?;
        if !matches!(
            dimension.as_str(),
            "label" | "assignee" | "state" | "status"
        ) {
            return Err("dimension must be label, assignee, state or status".into());
        }
        let key = source.tracker_list_key().stored()?;
        let (page, position) =
            BoundedPage::parse(params, &key, project, json!(["choices", dimension]))?;
        let choices = if matches!(dimension.as_str(), "state" | "status") {
            vocabulary(&dimension, position.as_deref())
        } else {
            source
                .list_task_filter_choices(project, &dimension, position.as_deref(), page.limit + 1)
                .stored()?
        };
        let mut values = Vec::new();
        let mut bytes = 0;
        let mut last = None;
        let mut next = None;
        for choice in choices {
            let size = serde_json::to_vec(&choice.value)
                .map_err(|error| error.to_string())?
                .len();
            if size > MAX_ROW_BYTES {
                return Err(
                    "filter choice exceeds 65536 bytes; repair the stored choice before continuing"
                        .into(),
                );
            }
            if values.len() == page.limit || bytes + size > MAX_RESPONSE_BYTES - RESPONSE_OVERHEAD {
                next = last.map(|position: String| page.cursor(&position));
                break;
            }
            bytes += size + 1;
            last = Some(choice.key);
            values.push(choice.value);
        }
        let mut answer = json!({"project_id":project_id,"dimension":dimension,"choices":values});
        if let Some(cursor) = next {
            answer["next_cursor"] = json!(cursor);
        }
        ensure_response_bound(&answer)?;
        Ok(answer)
    }
}

struct SummaryScan {
    rows: SummaryRows,
    below: Option<u64>,
    scanned: usize,
    budget: usize,
    limit: usize,
    more: bool,
}
impl SummaryScan {
    fn new(limit: usize, below: Option<u64>) -> Self {
        Self {
            rows: SummaryRows::default(),
            below,
            scanned: 0,
            budget: limit * 4,
            limit,
            more: false,
        }
    }
    fn admits(&self) -> bool {
        self.scanned < self.budget && self.rows.tasks.len() < self.limit
    }
}

#[derive(Default)]
struct SummaryRows {
    tasks: Vec<Value>,
    identities: BTreeMap<String, Value>,
    bytes: usize,
}

impl SummaryRows {
    /// A row is consumed only once it fits. A row exceeding the individual
    /// cap is an explicit failure, never omitted or returned with a looping cursor.
    fn push(&mut self, candidate: TaskSummaryCandidate) -> Result<bool, String> {
        let row_bytes = serde_json::to_vec(&candidate.row)
            .map_err(|error| error.to_string())?
            .len();
        let identity_bytes = serde_json::to_vec(&candidate.identities)
            .map_err(|error| error.to_string())?
            .len();
        let size = row_bytes + identity_bytes + 2;
        if size > MAX_ROW_BYTES {
            return Err(
                "task summary exceeds 65536 bytes; repair the stored row before continuing".into(),
            );
        }
        if self.bytes + size > MAX_RESPONSE_BYTES - RESPONSE_OVERHEAD {
            return Ok(false);
        }
        self.bytes += size;
        self.tasks.push(candidate.row);
        self.identities.extend(candidate.identities);
        Ok(true)
    }
}

fn ensure_response_bound(answer: &Value) -> Result<(), String> {
    if serde_json::to_vec(answer)
        .map_err(|error| error.to_string())?
        .len()
        > MAX_RESPONSE_BYTES
    {
        return Err("bounded task response exceeds 1048576 bytes".into());
    }
    Ok(())
}

fn vocabulary(dimension: &str, after: Option<&str>) -> Vec<crate::store::TaskFilterChoice> {
    let values: Vec<&str> = if dimension == "state" {
        vec!["closed", "open"]
    } else {
        crate::tracker::COLUMNS
            .iter()
            .map(|column| column.id)
            .collect()
    };
    let mut values: Vec<_> = values
        .into_iter()
        .filter(|value| after.is_none_or(|after| *value > after))
        .map(|value| crate::store::TaskFilterChoice {
            key: value.into(),
            value: json!(value),
        })
        .collect();
    values.sort_by(|a, b| a.key.cmp(&b.key));
    values
}

#[cfg(test)]
mod tests {
    use super::*;
    fn candidate(bytes: usize) -> TaskSummaryCandidate {
        TaskSummaryCandidate {
            row: json!({"title":"x".repeat(bytes)}),
            assignment_target: None,
            identities: BTreeMap::new(),
        }
    }
    #[test]
    fn byte_budget_stops_before_consuming_the_next_row_and_rejects_oversize() {
        let mut rows = SummaryRows::default();
        let mut accepted = 0;
        while rows.push(candidate(60_000)).unwrap() {
            accepted += 1;
        }
        assert!(accepted > 0 && accepted < 100);
        let mut next = SummaryRows::default();
        assert!(
            next.push(candidate(60_000)).unwrap(),
            "unconsumed row fits at the start of the next page"
        );
        assert!(
            next.push(candidate(MAX_ROW_BYTES)).is_err(),
            "oversize row must be explicit, never silently skipped"
        );
        assert!(
            serde_json::to_vec(&json!({"tasks":rows.tasks,"identities":rows.identities}))
                .unwrap()
                .len()
                < MAX_RESPONSE_BYTES
        );
    }
}

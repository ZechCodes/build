//! Membership deltas are bounded by one card; refcounts avoid duplicate scans.

use super::super::StoreError;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;
use std::collections::BTreeMap;

type Choices = BTreeMap<(String, String, String), String>;

pub(in crate::store::task_summaries) fn refresh(
    conn: &Connection,
    id: &str,
) -> Result<(), StoreError> {
    let desired = desired(conn, id)?;
    let existing = existing(conn, id)?;
    for (key, value) in &existing {
        if desired.get(key) != Some(value) {
            remove(conn, id, key)?;
        }
    }
    for (key, value) in desired {
        if existing.get(&key) != Some(&value) {
            insert(conn, id, &key, &value)?;
        }
    }
    Ok(())
}

fn existing(conn: &Connection, id: &str) -> Result<Choices, StoreError> {
    Ok(conn.prepare_cached("SELECT project_key,dimension,choice_key,value FROM tracker_task_filter_choices WHERE task_id=?1")?
        .query_map([id], |row| Ok(((row.get(0)?,row.get(1)?,row.get(2)?),row.get(3)?)))?
        .collect::<Result<_,_>>()?)
}

fn desired(conn: &Connection, id: &str) -> Result<Choices, StoreError> {
    let row: Option<(String, String, String)> = conn
        .prepare_cached(
            "SELECT project_key,record,context FROM tracker_task_summaries WHERE task_id=?1",
        )?
        .query_row([id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .optional()?;
    let Some((project, record, context)) = row else {
        return Ok(BTreeMap::new());
    };
    let record: Value =
        super::super::super::tracker::decode(&record, "tracker_task_summaries", id)?;
    let context: Value =
        super::super::super::tracker::decode(&context, "tracker_task_summaries/context", id)?;
    let mut result = BTreeMap::new();
    let mut add = |dimension: &str, key: String, value: Value| {
        let encoded = value.to_string();
        result
            .entry((project.clone(), dimension.to_string(), key))
            .and_modify(|old: &mut String| {
                if encoded < *old {
                    *old = encoded.clone();
                }
            })
            .or_insert(encoded);
    };
    if record["_summary_invalid_labels"].as_bool() == Some(true) {
        add("label", format!("!invalid:{id}"), Value::Null);
    } else if let Some(labels) = record["labels"].as_array() {
        for label in labels.iter().filter_map(Value::as_str) {
            add(
                "label",
                label.to_ascii_lowercase(),
                Value::String(label.to_string()),
            );
        }
    }
    for dimension in ["status", "state"] {
        let value = record[dimension].clone();
        add(
            dimension,
            value.as_str().unwrap_or_default().to_string(),
            value,
        );
    }
    let assignee = assignee(&record, &context);
    let key = if assignee.is_null() {
        format!("!invalid:{id}")
    } else {
        assignee.to_string()
    };
    add("assignee", key, assignee);
    Ok(result)
}

fn assignee(record: &Value, context: &Value) -> Value {
    if context["invalid_assignee"].as_i64() == Some(1) {
        return Value::Null;
    }
    let assignee = &record["assignee"];
    if assignee.is_null() {
        Value::String("none".into())
    } else if assignee["kind"].as_str() == Some("agent") {
        serde_json::json!({"agent_id":assignee["agent_id"],"kind":"agent"})
    } else {
        assignee.clone()
    }
}

fn remove(conn: &Connection, id: &str, key: &(String, String, String)) -> Result<(), StoreError> {
    let (project, dimension, choice) = key;
    conn.prepare_cached("DELETE FROM tracker_task_filter_choices WHERE project_key=?1 AND dimension=?2 AND choice_key=?3 AND task_id=?4")?.execute(params![project,dimension,choice,id])?;
    conn.prepare_cached("UPDATE tracker_task_filter_values SET refs=refs-1,value=COALESCE((SELECT value FROM tracker_task_filter_choices WHERE project_key=?1 AND dimension=?2 AND choice_key=?3 ORDER BY value LIMIT 1),value) WHERE project_key=?1 AND dimension=?2 AND choice_key=?3")?.execute(params![project,dimension,choice])?;
    conn.prepare_cached("DELETE FROM tracker_task_filter_values WHERE project_key=?1 AND dimension=?2 AND choice_key=?3 AND refs<=0")?.execute(params![project,dimension,choice])?;
    Ok(())
}

fn insert(
    conn: &Connection,
    id: &str,
    key: &(String, String, String),
    value: &str,
) -> Result<(), StoreError> {
    let (project, dimension, choice) = key;
    conn.prepare_cached("INSERT INTO tracker_task_filter_choices(project_key,dimension,choice_key,task_id,value) VALUES(?1,?2,?3,?4,?5)")?.execute(params![project,dimension,choice,id,value])?;
    conn.prepare_cached("INSERT INTO tracker_task_filter_values(project_key,dimension,choice_key,value,refs) VALUES(?1,?2,?3,?4,1) ON CONFLICT(project_key,dimension,choice_key) DO UPDATE SET refs=refs+1,value=MIN(value,excluded.value)")?.execute(params![project,dimension,choice,value])?;
    Ok(())
}

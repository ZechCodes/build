use super::*;

#[derive(Clone)]
pub(super) struct Row(Arc<RowData>);

pub(super) struct RowData {
    pub id: String,
    pub project: String,
    pub owner: Option<String>,
    pub anchor: Option<String>,
    pub value: SharedJson,
}

impl std::ops::Deref for Row {
    type Target = RowData;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Row {
    pub fn new(id: &str, value: Value) -> Self {
        Self::build(id, value, false)
    }

    /// Metadata is borrowed from the already indexed named leaf. This does
    /// not materialize a composed value or index its payload a second time.
    pub fn from_shared(id: &str, value: SharedJson) -> Self {
        let fields = value.leaf().expect("named branch is a rendered leaf");
        Self(Arc::new(RowData {
            id: fields[id].as_str().unwrap_or_default().to_owned(),
            project: fields["project_id"].as_str().unwrap_or_default().to_owned(),
            owner: fields["entity_id"].as_str().map(str::to_owned),
            anchor: fields["anchor"].as_str().map(str::to_owned),
            value,
        }))
    }

    pub fn object(id: &str, value: Value) -> Self {
        Self::build(id, value, true)
    }

    fn build(id: &str, value: Value, structured: bool) -> Self {
        Self(Arc::new(RowData {
            id: value[id].as_str().unwrap_or_default().to_owned(),
            project: value["project_id"].as_str().unwrap_or_default().to_owned(),
            owner: value["entity_id"].as_str().map(str::to_owned),
            anchor: value["anchor"].as_str().map(str::to_owned),
            value: if structured {
                object(value)
            } else {
                SharedJson::value(value)
            },
        }))
    }

    pub fn patch(&self, patch: &mut impl FnMut(&SharedJson) -> SharedJson) -> Self {
        let value = patch(&self.value);
        if value.ptr_eq(&self.value) {
            return self.clone();
        }
        Self(Arc::new(RowData {
            value,
            id: self.id.clone(),
            project: self.project.clone(),
            owner: self.owner.clone(),
            anchor: self.anchor.clone(),
        }))
    }
}

#[derive(Clone)]
pub(super) struct TaskRow {
    pub source: Arc<Value>,
    pub value: SharedJson,
}

pub(super) struct TaskRows {
    pub rows: Vec<TaskRow>,
    #[cfg(test)]
    pub revision: u64,
}

pub(super) fn tasks(app: &AppState, previous: &[TaskRow]) -> TaskRows {
    #[cfg(test)]
    let _phase = crate::timing::phase_samples::phase("publication.incremental.task_reuse");
    // SQL replaces one immutable task payload at a time. Keep both the digest
    // index and any accepted receipt patches when its source is unchanged.
    #[cfg(test)]
    let _task_index_phase =
        crate::timing::phase_samples::phase("publication.incremental.task_index");
    let indexed = previous
        .iter()
        .map(|row| (Arc::as_ptr(&row.source), row))
        .collect::<HashMap<_, _>>();
    #[cfg(test)]
    drop(_task_index_phase);
    let captured = app.capture_snapshot_watched_tasks();
    let rows = captured
        .rows
        .into_iter()
        .map(|source| {
            if let Some(old) = indexed.get(&Arc::as_ptr(&source)) {
                return (*old).clone();
            }
            TaskRow {
                value: SharedJson::from_shared(Arc::clone(&source)),
                source,
            }
        })
        .collect();
    TaskRows {
        rows,
        #[cfg(test)]
        revision: captured.revision,
    }
}

pub(super) fn object(value: Value) -> SharedJson {
    match value {
        Value::Object(fields) => SharedJson::object(
            fields
                .into_iter()
                .map(|(key, value)| (key, SharedJson::value(value))),
        ),
        value => SharedJson::value(value),
    }
}

pub(super) fn take_rows(
    fields: &mut serde_json::Map<String, Value>,
    key: &str,
    id: &str,
    structured: bool,
) -> Vec<Row> {
    match fields.remove(key) {
        Some(Value::Array(rows)) => rows
            .into_iter()
            .map(|row| Row::build(id, row, structured))
            .collect(),
        _ => Vec::new(),
    }
}

pub(super) fn array(rows: &[Row]) -> SharedJson {
    SharedJson::array(rows.iter().map(|row| row.value.clone()))
}

pub(super) fn wrapped(key: &str, rows: &[Row]) -> SharedJson {
    SharedJson::object([(key.to_owned(), array(rows))])
}

pub(super) fn put(
    delta: SnapshotDelta,
    previous: &ReadView,
    method: &'static str,
    owner: &str,
    value: SharedJson,
) -> SnapshotDelta {
    #[cfg(test)]
    let _phase = crate::timing::phase_samples::phase("publication.incremental.put");
    let unchanged = {
        #[cfg(test)]
        let _phase = crate::timing::phase_samples::phase("publication.incremental.equality");
        previous.get_shared(method, owner) == Some(&value)
    };
    if unchanged {
        delta
    } else {
        delta.put_shared(method, owner, value)
    }
}

pub(super) fn push_workspaces(workspaces: &[Row], summaries: &[Row]) -> SharedJson {
    #[cfg(test)]
    let _phase = crate::timing::phase_samples::phase("publication.incremental.workspace_list_json");
    #[cfg(test)]
    let _summary_index_phase =
        crate::timing::phase_samples::phase("publication.incremental.summary_index");
    let summaries = summaries.iter().fold(HashMap::new(), |mut index, row| {
        #[cfg(test)]
        crate::app::snapshot_projection::summary_counts::join_visit();
        index.entry(row.id.as_str()).or_insert(row);
        index
    });
    #[cfg(test)]
    drop(_summary_index_phase);
    SharedJson::array(workspaces.iter().map(|row| {
        let Some(summary) = summaries.get(row.id.as_str()) else {
            return row.value.clone();
        };
        let mut fields = row
            .value
            .fields()
            .expect("workspace row is an object")
            .clone();
        for field in ["work_summary", "can_finish", "finish_blockers"] {
            fields.insert(
                field.to_owned(),
                summary.value.field(field).unwrap_or_default(),
            );
        }
        SharedJson::object(fields)
    }))
}

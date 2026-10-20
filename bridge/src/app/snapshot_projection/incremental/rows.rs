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

pub(super) fn tasks(app: &AppState, previous: &[TaskRow]) -> Vec<TaskRow> {
    // SQL replaces one immutable task payload at a time. Keep both the digest
    // index and any accepted receipt patches when its source is unchanged.
    let indexed = previous
        .iter()
        .map(|row| (Arc::as_ptr(&row.source), row))
        .collect::<HashMap<_, _>>();
    app.snapshot_watched_task_shared_rows()
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
        .collect()
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
    if previous.get_shared(method, owner) == Some(&value) {
        delta
    } else {
        delta.put_shared(method, owner, value)
    }
}

pub(super) fn push_workspaces(workspaces: &[Row], summaries: &[Row]) -> SharedJson {
    let summaries = summaries.iter().fold(HashMap::new(), |mut index, row| {
        index.entry(row.id.as_str()).or_insert(row);
        index
    });
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

//! Canonical document inputs are frozen at committed snapshot publication.
use crate::app::{require_str, AppState};
use crate::plan::is_worktree_contained_path;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;

pub(super) struct Documents(HashMap<String, Document>);

struct Document {
    path: String,
    stages: Vec<(String, String)>,
    root: Option<PathBuf>,
}

impl Documents {
    pub(super) fn capture(app: &AppState) -> Self {
        Self(
            app.plans
                .iter()
                .map(|(id, active)| {
                    (
                        id.clone(),
                        Document {
                            path: active.plan_path.clone(),
                            stages: active
                                .stages
                                .iter()
                                .map(|stage| (stage.id.clone(), stage.path.clone()))
                                .collect(),
                            root: app.store.as_ref().map(|store| store.plan_docs_dir(id)),
                        },
                    )
                })
                .collect(),
        )
    }

    pub(super) fn contains(&self, id: &str) -> bool {
        self.0.contains_key(id)
    }

    pub(super) fn prepare(&self, method: &str, params: &Value) -> Result<DocumentRead, String> {
        let id = require_str(params, "plan_id")?;
        let stage = (method == "task.stage_doc")
            .then(|| require_str(params, "stage_id"))
            .transpose()?;
        let doc = self.0.get(&id).ok_or("unknown plan_id")?;
        let path = match &stage {
            Some(stage) => {
                if doc.stages.is_empty() {
                    return Err("not a multi-stage plan".into());
                }
                let entry = doc
                    .stages
                    .iter()
                    .find(|(id, _)| id == stage)
                    .ok_or_else(|| format!("unknown stage_id: {stage}"))?;
                if !entry.1.starts_with(".build/plan/") || !is_worktree_contained_path(&entry.1) {
                    return Err(format!(
                        "stage doc path escapes .build/plan/: {:?}",
                        entry.1
                    ));
                }
                entry.1.clone()
            }
            None => {
                if !doc.stages.is_empty() {
                    return Err("multi-stage plan: use plan.stages / plan.stage_doc".into());
                }
                doc.path.clone()
            }
        };
        Ok(DocumentRead {
            id,
            stage,
            path,
            root: doc.root.clone().ok_or("no task store configured")?,
        })
    }
}

pub(super) struct DocumentRead {
    id: String,
    stage: Option<String>,
    path: String,
    root: PathBuf,
}

impl DocumentRead {
    pub(super) fn run(self) -> Result<Value, String> {
        let contents = is_worktree_contained_path(&self.path)
            .then(|| std::fs::read_to_string(self.root.join(&self.path)).ok())
            .flatten()
            .ok_or_else(|| {
                format!(
                    "{} doc not available: {}",
                    if self.stage.is_some() {
                        "stage"
                    } else {
                        "plan"
                    },
                    self.path
                )
            })?;
        Ok(match self.stage {
            Some(stage) => {
                json!({"task_id": self.id,"plan_id":self.id,"stage_id":stage,"path":self.path,"contents":contents})
            }
            None => json!({"plan_path":self.path,"contents":contents}),
        })
    }
}

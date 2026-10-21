//! Restored startup inputs before the first accepted snapshot exists.
use super::AppState;
use crate::app::read_view::ReadView;
use crate::models::{AgentProvider, RoleModels};
use imbl::OrdMap;
use serde_json::Value;
use std::path::{Path, PathBuf};

#[derive(Clone)]
pub(in crate::app) struct HarnessSettings {
    pub default_harness: AgentProvider,
    pub role_models: RoleModels,
}

#[derive(Clone)]
pub(in crate::app) struct McpBootstrap {
    pub settings: HarnessSettings,
    projects: OrdMap<String, PathBuf>,
    bindings: OrdMap<String, String>,
    workspaces: OrdMap<String, String>,
}

impl McpBootstrap {
    pub fn capture(app: &AppState) -> Self {
        Self {
            settings: HarnessSettings {
                default_harness: app.default_harness,
                role_models: app.role_models.clone(),
            },
            projects: app
                .projects
                .iter()
                .map(|p| (p.id.clone(), p.repo_path.clone()))
                .collect(),
            workspaces: app
                .workspaces
                .list(None)
                .into_iter()
                .map(|w| (w.id.clone(), w.project_id.clone()))
                .collect(),
            bindings: app
                .plans
                .keys()
                .chain(app.runs.keys())
                .filter_map(|owner| {
                    app.projects
                        .project_id_of(owner)
                        .map(|p| (owner.clone(), p.to_owned()))
                })
                .collect(),
        }
    }

    pub fn update_owner(&mut self, app: &AppState, owner: &str) {
        match app.projects.project_id_of(owner) {
            Some(project) => {
                self.bindings.insert(owner.into(), project.into());
                if let Some(p) = app.projects.get(project) {
                    self.projects.insert(project.into(), p.repo_path.clone());
                }
            }
            None => {
                self.bindings.remove(owner);
            }
        }
    }

    pub fn project_binding(&self, owner: &str) -> Option<&str> {
        self.bindings.get(owner).map(String::as_str)
    }
    pub fn project_path(&self, id: &str) -> Option<&Path> {
        self.projects.get(id).map(PathBuf::as_path)
    }

    pub fn workspace_project(&self, id: &str) -> Option<&str> {
        self.workspaces.get(id).map(String::as_str)
    }
}

impl HarnessSettings {
    // Reader calls this only after capturing the root; serialization work is
    // offlock. settings.get is the already-existing config commit projection.
    pub fn published(view: &ReadView) -> Result<Option<Self>, String> {
        let Some(row) = view.get_shared("settings.get", "") else {
            return Ok(None);
        };
        let field = |name| -> Option<Value> {
            row.leaf()
                .and_then(|v| v.get(name))
                .cloned()
                .or_else(|| row.field(name).map(|v| v.materialize()))
        };
        let parse = |name: &str| field(name).ok_or_else(|| format!("settings.get: missing {name}"));
        Ok(Some(Self {
            default_harness: serde_json::from_value(parse("default_harness")?)
                .map_err(|e| e.to_string())?,
            role_models: serde_json::from_value(parse("role_models")?)
                .map_err(|e| e.to_string())?,
        }))
    }
}

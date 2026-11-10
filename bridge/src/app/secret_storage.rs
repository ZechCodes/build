//! Startup composition only. No secret methods enter RPC dispatch.
use super::AppState;
use crate::secrets::{SecretError, SecretService, SweeperHandle};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

impl AppState {
    /// Called before sharing AppState. Disk work never runs under its mutex.
    pub fn with_secret_vault(
        mut self,
        state_root: &Path,
        device_id: &str,
    ) -> Result<Self, SecretError> {
        self.secret_state_root = Some(state_root.to_path_buf());
        self.session_registry.set_secret_device(device_id);
        self.secret_service = None;
        let config = self.config_value(&self.projects_dir, self.default_harness, self.isolation);
        if self.reconcile_secret_membership(&config).is_err() {
            eprintln!("secret vault unavailable");
            return Ok(self);
        }
        self.publish_secret_project_routes(&config);
        if let Some(store) = &self.store {
            let changes = Arc::clone(&self.changes);
            store
                .writer()
                .set_secret_change_observer(Arc::new(move |record| {
                    changes.note_secret_records(std::slice::from_ref(record))
                }));
        }
        if self.validate_secret_storage_layout(state_root).is_err() {
            eprintln!("secret vault unavailable");
            return Ok(self);
        }
        let opened = self
            .store
            .as_ref()
            .ok_or(SecretError::Unavailable)
            .and_then(|store| SecretService::open(state_root, device_id, store.writer()));
        match opened {
            Ok(service) => {
                let changes = Arc::clone(&self.changes);
                service.set_change_observer(Arc::new(move |record| {
                    changes.note_secret_records(std::slice::from_ref(record))
                }));
                self.secret_service = Some(service);
            }
            Err(_) => eprintln!("secret vault unavailable"),
        }
        Ok(self)
    }

    pub(in crate::app) fn reconcile_secret_membership(
        &self,
        config: &serde_json::Value,
    ) -> Result<(), String> {
        if let Some(store) = &self.store {
            store
                .writer()
                .secrets_reconcile_config(config)
                .map_err(|error| error.to_string())?;
        }

        Ok(())
    }

    pub(in crate::app) fn publish_secret_project_routes(&self, config: &serde_json::Value) {
        self.changes.set_secret_project_environments(
            config["projects"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|project| {
                    Some((
                        project["id"].as_str()?.to_owned(),
                        project["environment_id"].as_str()?.to_owned(),
                    ))
                }),
        );
    }

    /// All project admission doors use the same resolved device-vault fence.
    pub(in crate::app) fn validate_secret_source(&self, path: &Path) -> Result<(), String> {
        if let Some(root) = &self.secret_state_root {
            crate::config::vault::validate_tree("project source", path, root)?;
        }
        Ok(())
    }

    pub(in crate::app) fn validate_secret_project(
        &self,
        project: &super::projects::Project,
    ) -> Result<(), String> {
        self.validate_secret_source(&project.repo_path)?;
        for source in &project.sources {
            self.validate_secret_source(&source.path)?;
        }
        Ok(())
    }

    fn validate_secret_storage_layout(&self, state_root: &Path) -> Result<(), String> {
        for path in [&self.projects_dir, &self.worktrees_root] {
            crate::config::vault::validate_tree("project storage root", path, state_root)?;
        }
        for project in self.projects.iter() {
            for path in std::iter::once(&project.repo_path)
                .chain(project.sources.iter().map(|source| &source.path))
            {
                crate::config::vault::validate_tree("project source", path, state_root)?;
            }
        }
        for entry in self.projects.parked() {
            validate_parked_sources(entry, state_root)?;
        }
        self.validate_secret_workspace_layout(state_root)
    }

    fn validate_secret_workspace_layout(&self, state_root: &Path) -> Result<(), String> {
        for workspace in self.workspaces.list(None) {
            crate::config::vault::validate_tree("workspace root", &workspace.root, state_root)?;
            for directory in &workspace.directories {
                crate::config::vault::validate_tree(
                    "workspace directory",
                    &directory.path,
                    state_root,
                )?;
            }
        }
        for run in self.runs.values() {
            crate::config::vault::validate_tree("run worktree", &run.worktree.path, state_root)?;
        }
        Ok(())
    }

    /// Internal composition handle for the subsequent authority/audit layer.
    pub fn secret_service(&self) -> Option<SecretService> {
        self.secret_service.clone()
    }

    pub fn spawn_secret_sweeper(&self) -> Option<SweeperHandle> {
        self.secret_service
            .as_ref()
            .map(|service| service.spawn_sweeper(Duration::from_secs(60)))
    }
}

#[cfg(test)]
mod tests;

fn validate_parked_sources(entry: &serde_json::Value, state_root: &Path) -> Result<(), String> {
    if let Some(path) = entry.get("path").and_then(serde_json::Value::as_str) {
        crate::config::vault::validate_tree(
            "parked project",
            &super::expand_tilde(path),
            state_root,
        )?;
    }
    let sources = entry
        .get("sources")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten();
    for source in sources {
        if let Some(path) = source.get("path").and_then(serde_json::Value::as_str) {
            crate::config::vault::validate_tree(
                "parked project source",
                &super::expand_tilde(path),
                state_root,
            )?;
        }
    }
    Ok(())
}

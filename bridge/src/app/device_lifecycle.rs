//! Non-SQL owners mark before effects and reconcile only authoritative fields.
use super::AppState;
use crate::store::sync::Entity;
use serde_json::{json, Value};
use std::collections::BTreeMap;

impl AppState {
    pub(in crate::app) fn mark_device_lifecycle_dirty(&self) -> Result<(), String> {
        if let Some(store) = &self.store {
            store
                .writer()
                .mark_sync_dirty("registry", "lifecycle", crate::store::sync::now_ms())
                .map_err(|error| format!("could not persist sync marker: {error}"))?;
        }
        self.device_lifecycle_dirty.set(true);
        Ok(())
    }
    pub(in crate::app) fn reconcile_device_lifecycle(&self) -> Result<(), String> {
        if !self.device_lifecycle_dirty.get()
            || self.device_lifecycle_holds > 0
            || self
                .deferred_work
                .as_ref()
                .is_some_and(super::DeferredWork::changes_device_registry)
            || self.active_device_registry_jobs > 0
            || self.project_deletion_in_progress
        {
            return Ok(());
        }
        let Some(store) = &self.store else {
            return Ok(());
        };
        let writer = store.writer();
        let Some(generation) = writer
            .device_registry_generation()
            .map_err(|error| error.to_string())?
        else {
            return Ok(());
        };
        let markers = writer
            .device_registry_markers()
            .map_err(|error| error.to_string())?;
        let (rows, projects) = self.device_registry_rows()?;
        writer
            .reconcile_device_registry(generation, &rows, &projects, &markers)
            .map_err(|error| error.to_string())?;
        self.device_lifecycle_dirty.set(false);
        Ok(())
    }
    pub(in crate::app) fn settle_device_lifecycle(&self) {
        if let Err(error) = self.reconcile_device_lifecycle() {
            eprintln!("device sync reconciliation: {error}");
        }
    }
    pub(in crate::app) fn initialize_device_lifecycle(&self) -> Result<(), String> {
        self.mark_device_lifecycle_dirty()?;
        self.settle_device_lifecycle();
        Ok(())
    }
    fn device_registry_rows(&self) -> Result<RegistryRows, String> {
        let config = self.config_value(&self.projects_dir, self.default_harness, self.isolation);
        let mut rows = BTreeMap::new();
        let mut projects = BTreeMap::new();
        for project in config["projects"].as_array().into_iter().flatten() {
            project_rows(project, &mut rows, &mut projects);
        }
        for workspace in self.workspaces.iter() {
            let mut row = serde_json::to_value(workspace).map_err(|error| error.to_string())?;
            row.as_object_mut()
                .expect("workspace object")
                .remove("directories");
            rows.insert(Entity::new("workspace", &workspace.id), row);
            for directory in &workspace.directories {
                let mut row = serde_json::to_value(directory).map_err(|error| error.to_string())?;
                row["workspace_id"] = json!(workspace.id);
                row["project_id"] = json!(workspace.project_id);
                rows.insert(
                    Entity::new(
                        "workspace_directory",
                        &format!("{}/{}", workspace.id, directory.id),
                    ),
                    row,
                );
            }
        }
        let badge = json!({"projects":projects.len(),"workspaces":self.workspaces.iter().len(),
            "locked_workspaces":self.workspaces.iter().filter(|workspace|workspace.locked).count()});
        rows.insert(Entity::new("badge", "device"), badge);
        Ok((rows, projects))
    }
}
type RegistryRows = (BTreeMap<Entity, Value>, BTreeMap<String, String>);

#[cfg(test)]
mod identity_tests {
    use super::*;

    #[test]
    fn device_project_ids_keep_sibling_keys_distinct() {
        let mut rows = BTreeMap::new();
        let mut projects = BTreeMap::new();
        for (id, key) in [("proj-1", "/checkout"), ("proj-2", "project:opaque")] {
            project_rows(
                &json!({"id":id,"path":"/checkout","project_key":key}),
                &mut rows,
                &mut projects,
            );
        }
        assert_eq!(projects.len(), 2);
        assert_eq!(projects["/checkout"], "proj-1");
        assert_eq!(projects["project:opaque"], "proj-2");
        assert_eq!(rows[&Entity::new("project", "proj-2")]["path"], "/checkout");
    }
}

fn project_rows(
    project: &Value,
    rows: &mut BTreeMap<Entity, Value>,
    projects: &mut BTreeMap<String, String>,
) {
    let (Some(id), Some(path)) = (project["id"].as_str(), project["path"].as_str()) else {
        return;
    };
    let Some(key) = crate::app::projects::configured_project_key(project) else {
        return;
    };
    projects.insert(key.to_owned(), id.to_owned());
    let mut row = project.clone();
    row.as_object_mut()
        .expect("project object")
        .remove("sources");
    row["project_id"] = json!(id);
    row["name"] = json!(std::path::Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy()));
    rows.insert(Entity::new("project", id), row);
    for source in project["sources"].as_array().into_iter().flatten() {
        let Some(source_id) = source["id"].as_str() else {
            continue;
        };
        let mut row = source.clone();
        row["project_id"] = json!(id);
        rows.insert(
            Entity::new("project_source", &format!("{id}/{source_id}")),
            row,
        );
    }
}

use super::*;

impl AppState {
    /// Never delete files on startup. Changed rollback state invalidates the
    /// plan; preserve it and make a new explicit delete capture fresh authority.
    pub(in crate::app) fn recover_project_deletions(&mut self) -> Result<(), String> {
        let store = self.store.as_ref().ok_or("missing deletion store")?.clone();
        for deletion in store
            .writer()
            .project_deletions()
            .map_err(|e| e.to_string())?
        {
            self.recover_project_deletion(deletion)?;
        }
        store
            .writer()
            .collect_project_deletion_tombstones()
            .map_err(|e| e.to_string())?;
        self.refresh_deletion_fences()?;
        for plan in &self.project_deletion_fences {
            if let Some(number) = plan
                .project_id
                .strip_prefix("proj-")
                .and_then(|n| n.parse::<u64>().ok())
            {
                self.projects.reserve_ids_through(number.saturating_add(1));
            }
        }
        Ok(())
    }

    fn recover_project_deletion(&mut self, mut deletion: ProjectDeletion) -> Result<(), String> {
        let store = self.store.as_ref().ok_or("missing deletion store")?.clone();
        if deletion.tombstone.is_some() {
            return Ok(());
        }
        let plan = &deletion.plan;
        let configured = self
            .projects
            .iter()
            .any(|p| p.id == plan.project_id || p.project_key == plan.project_key)
            || self
                .projects
                .parked()
                .iter()
                .any(|p| p["id"] == plan.project_id || p["project_key"] == plan.project_key);
        let current = self.config_value(&self.projects_dir, self.default_harness, self.isolation);
        let matching = current["projects"]
            .as_array()
            .expect("project array")
            .iter()
            .find(|p| p["id"] == plan.project_id || p["project_key"] == plan.project_key);
        let readded =
            configured && (deletion.phase != "prepared" || matching != Some(&plan.project_config));
        if readded || self.validate_deletion_plan(&deletion).is_err() {
            if !configured {
                let mut config =
                    self.config_value(&self.projects_dir, self.default_harness, self.isolation);
                config["projects"]
                    .as_array_mut()
                    .expect("project array")
                    .push(plan.project_config.clone());
                if self
                    .projects
                    .environment(
                        plan.environment_config["id"]
                            .as_str()
                            .ok_or("saved environment ID missing")?,
                    )
                    .is_none()
                {
                    config["environments"]
                        .as_array_mut()
                        .expect("environment array")
                        .push(plan.environment_config.clone());
                }
                self.persist_config(&config)?;
                self.projects.restore_environments(&config);
                self.restore_configured_project(&plan.project_config);
            }
            deletion.phase = "abandoned".into();
            deletion.tombstone = Some("abandoned".into());
            store
                .writer()
                .checkpoint_project_deletion(&deletion)
                .map_err(|e| e.to_string())?;
            eprintln!("Project {} deletion abandoned: state changed under a previous bridge; preserved state, delete again for a fresh plan",deletion.plan.project_id);
        } else {
            eprintln!(
                "Project {} deletion-pending; retry project.delete with confirm:true{}",
                plan.project_id,
                if plan.delete_tasks {
                    ", tasks:delete"
                } else {
                    ""
                }
            );
        }
        Ok(())
    }

    pub(super) fn refresh_deletion_fences(&mut self) -> Result<(), String> {
        self.project_deletion_fences = self
            .project_deletion_records()?
            .into_iter()
            .filter(|r| r.tombstone.is_none())
            .map(|r| r.plan)
            .collect();
        Ok(())
    }
}

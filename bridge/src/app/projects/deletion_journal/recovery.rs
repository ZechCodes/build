use super::*;

impl AppState {
    /// Only committed pin receipts are drained at startup. Changed state invalidates the
    /// plan; preserve it and make a new explicit delete capture fresh authority.
    pub(in crate::app) fn recover_project_deletions(&mut self) -> Result<(), String> {
        let store = self.store.as_ref().ok_or("missing deletion store")?.clone();
        self.project_deletion_cache = store
            .writer()
            .project_deletions()
            .map_err(|e| e.to_string())?;
        for deletion in self.project_deletion_cache.clone() {
            self.recover_project_deletion(deletion)?;
        }
        store
            .writer()
            .collect_project_deletion_tombstones()
            .map_err(|e| e.to_string())?;
        self.project_deletion_cache = store
            .writer()
            .project_deletions()
            .map_err(|e| e.to_string())?;
        self.refresh_deletion_fences();
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
        // A committed receipt is owed independently of stale cleanup authority.
        // Failed releases remain durable, including on abandoned tombstones.
        if let Err(error) = drain_recovered_pins(&store, &mut deletion) {
            eprintln!(
                "Project {} pin release pending: {error}; retained receipt for startup retry",
                deletion.plan.project_id
            );
        }
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
        if readded || validate_plan(&store, &deletion, &self.deletion_sources()).is_err() {
            if !configured {
                self.restore_abandoned_registration(plan)?;
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

    fn restore_abandoned_registration(&mut self, plan: &ProjectDeletionPlan) -> Result<(), String> {
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
        Ok(())
    }

    pub(super) fn refresh_deletion_fences(&mut self) {
        let provisional: Vec<_> = self
            .project_deletion_fences
            .iter()
            .filter(|plan| {
                self.project_deletion_operations
                    .contains_key(&plan.project_id)
                    && !self
                        .project_deletion_cache
                        .iter()
                        .any(|record| record.plan.project_id == plan.project_id)
            })
            .cloned()
            .collect();
        self.project_deletion_fences = self
            .project_deletion_cache
            .iter()
            .filter(|r| r.tombstone.is_none())
            .map(|r| r.plan.clone())
            .chain(provisional)
            .collect();
    }
}

/// Safe on pending, completed and abandoned records. Clear authority only
/// after every recorded ref/marker is absent; a partial release is repeatable.
fn drain_recovered_pins(store: &Store, deletion: &mut ProjectDeletion) -> Result<(), String> {
    if deletion.pending_pins.is_empty() {
        return Ok(());
    }
    crate::reviews::history_pins::release(&deletion.pending_pins)?;
    let mut drained = deletion.clone();
    drained.pending_pins.clear();
    store
        .writer()
        .checkpoint_project_deletion(&drained)
        .map_err(|e| e.to_string())?;
    *deletion = drained;
    Ok(())
}

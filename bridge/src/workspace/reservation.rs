use super::*;

impl WorkspaceRegistry {
    /// A pure plan: callers save this identity before creating its container.
    pub(crate) fn reserve_delivery_workspace(
        &self,
        project_id: &str,
        name: &str,
        sources: &[WorkspaceSource],
        isolation: Isolation,
    ) -> Result<Workspace, String> {
        validate_segment("project id", project_id)?;
        let mut mounts = std::collections::HashSet::new();
        for source in sources {
            validate_segment("source mount", &source.mount)?;
            if !mounts.insert(&source.mount) {
                return Err(format!("duplicate source mount: {:?}", source.mount));
            }
        }
        let id = uuid::Uuid::new_v4().to_string();
        let slug = crate::worktree::slugify(name);
        let root = self
            .root
            .join(project_id)
            .join(format!("{}-{id}", &slug[..slug.len().min(12)]));
        let directories = sources
            .iter()
            .map(|source| WorkspaceDirectory {
                id: format!("{id}:{}", source.id),
                source_id: source.id.clone(),
                name: source.name.clone(),
                path: root.join(&source.mount),
                is_git: source.is_git,
                branch: None,
                effective_isolation: None,
                finished_head: None,
                status: DirectoryStatus::Pending,
                source_path: source.path.clone(),
                base_branch: source.base_branch.clone(),
                error: None,
            })
            .collect();
        Ok(Workspace {
            id,
            project_id: project_id.into(),
            name: name.into(),
            root,
            status: WorkspaceStatus::Provisioning,
            archived_at: None,
            directories,
            isolation,
            managed: true,
            created_by_agent: false,
            locked: false,
        })
    }

    /// Recover only a saved owned reservation. Never unwind partial files.
    pub(crate) fn claim_delivery_workspace(
        &mut self,
        reserved: &Workspace,
    ) -> Result<Workspace, String> {
        self.validate_delivery_reservation(reserved)?;
        let journal = self.root.join(".delivery-reservations").join(&reserved.id);
        remember_reservation(&journal, reserved)?;
        create_owned_container(reserved)?;
        let manifest = reserved.root.join(MANIFEST_FILE);
        let mut workspace = if manifest.exists() {
            let bytes = fs::read(&manifest).map_err(|error| error.to_string())?;
            let existing: Workspace =
                serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
            verify_reserved_identity(reserved, &existing)?;
            existing
        } else {
            reserved.clone()
        };
        validate_loaded_workspace(
            &workspace,
            &workspace.root,
            &self.root.join(&workspace.project_id),
            &fs::canonicalize(&self.root).map_err(|error| error.to_string())?,
        )?;
        if workspace.status == WorkspaceStatus::Finished {
            return Err("reserved workspace has been finished".into());
        }
        if workspace.status != WorkspaceStatus::Ready {
            workspace.status = WorkspaceStatus::Provisioning;
        }
        self.storage_anchor.capture();
        self.storage_anchor
            .capture_workspace(&workspace.root)
            .map_err(|error| error.to_string())?;
        persist(&workspace)?;
        self.workspaces
            .insert(workspace.id.clone(), workspace.clone());
        Ok(workspace)
    }

    fn validate_delivery_reservation(&self, workspace: &Workspace) -> Result<(), String> {
        validate_segment("project id", &workspace.project_id)?;
        validate_segment("workspace id", &workspace.id)?;
        let project_root = self.root.join(&workspace.project_id);
        if !workspace.managed || workspace.root.parent() != Some(project_root.as_path()) {
            return Err("reserved workspace is outside its managed project root".into());
        }
        let name = workspace
            .root
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or("reserved workspace has no directory name")?;
        validate_segment("workspace directory", name)?;
        if !name.ends_with(&format!("-{}", workspace.id)) {
            return Err("reserved workspace directory does not match its identity".into());
        }
        for directory in &workspace.directories {
            if directory.path.parent() != Some(workspace.root.as_path()) {
                return Err("reserved source directory escapes its workspace".into());
            }
            let mount = directory
                .path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or("reserved source has no mount")?;
            validate_segment("source mount", mount)?;
        }
        refuse_symlink(&self.root)?;
        refuse_symlink(&project_root)?;
        refuse_symlink(&self.root.join(".delivery-reservations"))?;
        refuse_symlink(&workspace.root)
    }
}

fn refuse_symlink(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(format!(
            "reserved workspace path is a symlink: {}",
            path.display()
        )),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

fn remember_reservation(journal: &Path, reserved: &Workspace) -> Result<(), String> {
    refuse_symlink(journal)?;
    if journal.exists() {
        let saved: Workspace =
            serde_json::from_slice(&fs::read(journal).map_err(|error| error.to_string())?)
                .map_err(|error| error.to_string())?;
        if saved != *reserved {
            return Err("workspace reservation journal has another owner".into());
        }
        return Ok(());
    }
    // A journal is only minted for an absent root; its UUID proves subsequent
    // container creation belongs to this saved assignment, including a crash
    // between mkdir and manifest publication.
    if fs::symlink_metadata(&reserved.root).is_ok() {
        return Err("reserved workspace root already exists without ownership proof".into());
    }
    fs::create_dir_all(
        journal
            .parent()
            .ok_or("reservation journal has no parent")?,
    )
    .map_err(|error| error.to_string())?;
    let bytes = serde_json::to_vec(reserved).map_err(|error| error.to_string())?;
    atomic_write(journal, &bytes).map_err(|error| error.to_string())
}

fn create_owned_container(reserved: &Workspace) -> Result<(), String> {
    fs::create_dir_all(reserved.root.parent().ok_or("workspace has no parent")?)
        .map_err(|error| error.to_string())?;
    match fs::create_dir(&reserved.root) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

fn verify_reserved_identity(reserved: &Workspace, existing: &Workspace) -> Result<(), String> {
    let same = reserved.id == existing.id
        && reserved.project_id == existing.project_id
        && reserved.root == existing.root
        && reserved.isolation == existing.isolation
        && existing.managed
        && reserved.directories.len() == existing.directories.len()
        && reserved
            .directories
            .iter()
            .zip(&existing.directories)
            .all(|(planned, actual)| {
                planned.id == actual.id
                    && planned.source_id == actual.source_id
                    && planned.path == actual.path
                    && planned.source_path == actual.source_path
                    && planned.base_branch == actual.base_branch
                    && planned.is_git == actual.is_git
            });
    if same {
        Ok(())
    } else {
        Err("reserved workspace manifest has another owner or source snapshot".into())
    }
}

#[cfg(test)]
mod tests {
    use super::super::*;

    fn plan(registry: &WorkspaceRegistry, source_root: &Path) -> Workspace {
        registry
            .reserve_delivery_workspace(
                "project",
                "reserved work",
                &[WorkspaceSource {
                    id: "source".into(),
                    name: "source".into(),
                    mount: "source".into(),
                    path: source_root.to_path_buf(),
                    is_git: false,
                    base_branch: String::new(),
                }],
                Isolation::Worktree,
            )
            .unwrap()
    }

    #[test]
    fn delivery_reservation_has_no_filesystem_side_effects() {
        let temp = tempfile::tempdir().unwrap();
        let registry = WorkspaceRegistry::empty(temp.path().join("workspaces"));
        let reserved = plan(&registry, temp.path());
        assert!(!registry.root().exists());
        assert!(!reserved.root.exists());
        let saved = serde_json::to_vec(&reserved).unwrap();
        let restored: Workspace = serde_json::from_slice(&saved).unwrap();
        assert_eq!(reserved, restored);
    }

    #[test]
    fn delivery_reservation_recovers_same_manifest_and_retains_work() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("workspaces");
        let mut registry = WorkspaceRegistry::empty(&root);
        let reserved = plan(&registry, temp.path());
        let claimed = registry.claim_delivery_workspace(&reserved).unwrap();
        let mount = &claimed.directories[0].path;
        fs::create_dir(mount).unwrap();
        fs::write(mount.join("user-work"), b"retained").unwrap();
        registry
            .provision(
                &claimed.id,
                &WorkspaceRegistry::retry_sources(&claimed),
                claimed.isolation,
                |_, _, _| Ok((None, Isolation::Worktree)),
            )
            .unwrap();
        let mut restored = WorkspaceRegistry::recover(&root).unwrap();
        let recovered = restored.claim_delivery_workspace(&reserved).unwrap();
        assert_eq!(recovered.id, reserved.id);
        assert_eq!(recovered.status, WorkspaceStatus::Ready);
        assert_eq!(fs::read(mount.join("user-work")).unwrap(), b"retained");
        assert_eq!(restored.list(Some("project")).len(), 1);
    }

    #[test]
    fn delivery_reservation_refuses_foreign_root_without_removing_it() {
        let temp = tempfile::tempdir().unwrap();
        let mut registry = WorkspaceRegistry::empty(temp.path().join("workspaces"));
        let reserved = plan(&registry, temp.path());
        fs::create_dir_all(&reserved.root).unwrap();
        fs::write(reserved.root.join("user-work"), b"untouched").unwrap();
        assert!(registry.claim_delivery_workspace(&reserved).is_err());
        assert_eq!(
            fs::read(reserved.root.join("user-work")).unwrap(),
            b"untouched"
        );
    }

    #[test]
    fn delivery_reservation_recovers_container_before_manifest_publication() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("workspaces");
        let mut registry = WorkspaceRegistry::empty(&root);
        let reserved = plan(&registry, temp.path());
        registry.claim_delivery_workspace(&reserved).unwrap();
        fs::remove_file(reserved.root.join(MANIFEST_FILE)).unwrap();
        let mut restarted = WorkspaceRegistry::recover(&root).unwrap();
        let recovered = restarted.claim_delivery_workspace(&reserved).unwrap();
        assert_eq!(recovered.id, reserved.id);
        assert_eq!(recovered.root, reserved.root);
        assert_eq!(restarted.list(Some("project")).len(), 1);
    }

    #[test]
    fn delivery_reservation_refuses_redirected_project_directory() {
        let temp = tempfile::tempdir().unwrap();
        let mut registry = WorkspaceRegistry::empty(temp.path().join("workspaces"));
        let reserved = plan(&registry, temp.path());
        let foreign = temp.path().join("foreign");
        fs::create_dir_all(&foreign).unwrap();
        fs::create_dir_all(registry.root()).unwrap();
        std::os::unix::fs::symlink(&foreign, registry.root().join("project")).unwrap();
        assert!(registry.claim_delivery_workspace(&reserved).is_err());
        assert_eq!(fs::read_dir(foreign).unwrap().count(), 0);
    }
}

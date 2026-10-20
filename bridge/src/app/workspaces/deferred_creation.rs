//! Resume a saved task workspace through ordinary checkpointed provisioning.

use super::*;

impl AppState {
    pub(in crate::app) fn prepare_deferred_workspace_snapshot(
        &mut self,
        reserved: &Workspace,
    ) -> Result<DeferredGit, String> {
        self.project_for(&reserved.project_id)?;
        if self.workspace_reserved(&reserved.id) {
            return Err(crate::reclaim::BUSY.to_string());
        }
        let workspace = self.workspaces.claim_delivery_workspace(reserved)?;
        let sources = WorkspaceRegistry::retry_sources(&workspace);
        // Delivery reservations retain their saved source inputs across settings
        // edits and restart, just like explicit manifest retries. Prepare only
        // unfinished inputs, always pinning Git bases under their shared lock.
        let base_syncs = self.retry_sync_subjects(&workspace.project_id, &sources);
        self.refresh_workspace_snapshot_observation(&workspace.id);
        self.note_board_lists_changed(crate::changes::BoardLists::WORKSPACES);
        Ok(DeferredGit {
            call: Box::new(WorkspaceCreateWork {
                registry_root: self.workspaces.root().to_path_buf(),
                rift_root: self.project_worktrees_root(&workspace.project_id),
                workspace_id: workspace.id.clone(),
                workspace_name: workspace
                    .root
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                workspace: reserved.clone(),
                unpublished: false,
                recover_existing: true,
                sources,
                isolation: workspace.isolation,
                base_syncs,
                source_state: self.source_validation_handle(),
                #[cfg(test)]
                prepared_gate: self.off_lock_workspace_prepared_gate.take(),
                synced: Default::default(),
            }),
            params: Value::Null,
            invalidates: true,
            #[cfg(test)]
            gate: None,
        })
    }
}

pub(super) fn recover_checkout(
    source: &WorkspaceSource,
    destination: &Path,
    workspace_name: &str,
) -> Result<(Option<String>, Isolation), String> {
    let refusal = || {
        format!(
            "retained partial workspace directory {} needs explicit recovery",
            destination.display()
        )
    };
    let metadata = std::fs::symlink_metadata(destination).map_err(|error| error.to_string())?;
    if metadata.file_type().is_symlink() || !source.is_git {
        return Err(refusal());
    }
    let repository = git2::Repository::open(destination).map_err(|_| refusal())?;
    let actual = std::fs::canonicalize(destination).map_err(|error| error.to_string())?;
    let workdir = repository.workdir().ok_or_else(refusal)?;
    if std::fs::canonicalize(workdir).map_err(|error| error.to_string())? != actual {
        return Err(refusal());
    }
    let head = repository.head().map_err(|_| refusal())?;
    let branch = head
        .shorthand()
        .filter(|_| head.is_branch())
        .ok_or_else(refusal)?;
    let expected = crate::worktree::branch_name_for(workspace_name);
    if branch != expected
        && !branch
            .strip_prefix(&format!("{expected}-"))
            .is_some_and(|suffix| suffix.parse::<u64>().is_ok())
    {
        return Err(refusal());
    }
    let isolation = Isolation::of(destination).ok_or_else(refusal)?;
    if isolation == Isolation::Worktree {
        let primary = git2::Repository::open(&source.path).map_err(|_| refusal())?;
        if primary.commondir() != repository.commondir() {
            return Err(refusal());
        }
    }
    Ok((Some(branch.to_string()), isolation))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repository(path: &Path) {
        std::fs::create_dir(path).unwrap();
        let repo = git2::Repository::init(path).unwrap();
        std::fs::write(path.join("tracked"), b"initial").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("tracked")).unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let signature = git2::Signature::now("Build tests", "test@example.com").unwrap();
        repo.commit(
            Some("refs/heads/main"),
            &signature,
            &signature,
            "initial",
            &tree,
            &[],
        )
        .unwrap();
        repo.set_head("refs/heads/main").unwrap();
    }

    #[test]
    fn deferred_checkout_recovery_keeps_changes_before_source_checkpoint() {
        let temp = tempfile::tempdir().unwrap();
        let source_path = temp.path().join("source");
        repository(&source_path);
        let source = WorkspaceSource {
            id: "one".into(),
            name: "one".into(),
            mount: "one".into(),
            path: source_path,
            is_git: true,
            base_branch: "main".into(),
        };
        let mut registry = WorkspaceRegistry::empty(temp.path().join("workspaces"));
        let reserved = registry
            .reserve_delivery_workspace(
                "project",
                "checkpoint gap",
                std::slice::from_ref(&source),
                Isolation::Worktree,
            )
            .unwrap();
        registry.claim_delivery_workspace(&reserved).unwrap();
        let destination = &reserved.directories[0].path;
        let workspace_name = reserved.root.file_name().unwrap().to_str().unwrap();
        let materialized = materialize_source(
            &source,
            destination,
            workspace_name,
            Isolation::Worktree,
            temp.path(),
            Some(
                &git2::Repository::open(&source.path)
                    .unwrap()
                    .head()
                    .unwrap()
                    .target()
                    .unwrap()
                    .to_string(),
            ),
        )
        .unwrap();
        std::fs::write(destination.join("tracked"), b"user edits").unwrap();
        std::fs::write(destination.join("untracked"), b"user file").unwrap();
        let mut restarted = WorkspaceRegistry::recover(registry.root()).unwrap();
        let claimed = restarted.claim_delivery_workspace(&reserved).unwrap();
        let ready = restarted
            .provision(
                &claimed.id,
                &[source],
                claimed.isolation,
                |source, destination, _| recover_checkout(source, destination, workspace_name),
            )
            .unwrap();
        assert_eq!(ready.status, crate::workspace::WorkspaceStatus::Ready);
        assert_eq!(ready.directories[0].branch, materialized.0);
        assert_eq!(
            std::fs::read(destination.join("tracked")).unwrap(),
            b"user edits"
        );
        assert_eq!(
            std::fs::read(destination.join("untracked")).unwrap(),
            b"user file"
        );
        assert_eq!(restarted.list(Some("project")).len(), 1);
    }

    #[test]
    fn deferred_checkout_recovery_retains_unverified_plain_directory() {
        let temp = tempfile::tempdir().unwrap();
        let source = WorkspaceSource {
            id: "one".into(),
            name: "one".into(),
            mount: "one".into(),
            path: temp.path().to_path_buf(),
            is_git: false,
            base_branch: String::new(),
        };
        let destination = temp.path().join("retained");
        std::fs::create_dir(&destination).unwrap();
        std::fs::write(destination.join("user-file"), b"retained").unwrap();
        let error = recover_checkout(&source, &destination, "reserved").unwrap_err();
        assert!(error.contains("explicit recovery"), "{error}");
        assert_eq!(
            std::fs::read(destination.join("user-file")).unwrap(),
            b"retained"
        );
    }
}

pub(in crate::app) mod lifecycle;
pub(in crate::app) mod reporting;
pub(in crate::app) mod review;
pub(in crate::app) mod views;

impl crate::app::AppState {
    /// Run teardown can remove a workspace's container or one of its adopted
    /// checkouts. Read the persisted user lock before retiring any process or
    /// changing a run. Ancestors are protected too: deleting a parent checkout
    /// must not take a locked workspace below it away.
    pub(in crate::app) fn refuse_removing_locked_workspace_at(
        &self,
        path: &std::path::Path,
    ) -> Result<(), String> {
        let path = Self::canonical_root(path);
        let locked = self.workspaces.list(None).into_iter().any(|workspace| {
            if !workspace.locked {
                return false;
            }
            let root = Self::canonical_root(&workspace.root);
            path.starts_with(&root) || root.starts_with(&path)
        });
        if locked {
            return Err(crate::workspace::LOCKED_REFUSAL.into());
        }
        Ok(())
    }
}

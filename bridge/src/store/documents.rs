use super::{
    copy_tree, dir_contains_a_file, remove_file_if_present, Store, StoreError, STAGE_PLAN_DIR,
};
use crate::plan::is_worktree_contained_path;
use std::path::Path;
use std::path::PathBuf;

impl Store {
    /// The name of the note left in the JSON tree once it has been imported.
    ///
    /// It is for a human reading the state dir, not for Build: an old bridge
    /// would ignore it. What protects against a rollback is
    /// [`refuse_a_rolled_back_store`](Self::refuse_a_rolled_back_store); this
    /// is what tells the person looking at the directory why there are two
    /// copies of their work in it.
    pub(super) const SUPERSEDED_NOTE: &'static str = "SUPERSEDED-BY-build.db.md";
    pub(super) fn write_superseded_note(&self, imported: usize) -> Result<(), StoreError> {
        let note = format!(
            "# These records were imported into `build.db`\n\n             {imported} records were read out of this tree and into the SQLite              database beside it. Build no longer reads them.\n\n             They are kept, not deleted, for two reasons:\n\n             - They are the backup of the migration. Deleting `build.db` makes              Build import them again from scratch, which is the whole recovery              if the database ever turns out to be wrong.\n             - They are yours to delete once you are satisfied. Build never will.\n\n             **Do not run an older build-bridge against this directory.** It              would read these files and serve state frozen at the moment of the              import, silently, and anything you did in the meantime would be              invisible. A build that understands the database refuses to start              if these files change after this point.\n"
        );
        std::fs::write(self.dir.join(Store::SUPERSEDED_NOTE), note)?;
        Ok(())
    }
    /// Where conversation attachments live when the entity that took them has
    /// no checkout to put them in. Its own directory, beside the records rather
    /// than among them: the store root is scanned for legacy task files.
    pub fn attachments_dir(&self) -> PathBuf {
        self.dir.join("attachments")
    }
    pub(super) fn issue_dir(&self, issue_id: &str) -> PathBuf {
        self.dir.join("issues").join(issue_id)
    }
    /// Where an Issue's canonical docs live (worktree-relative layout inside).
    pub(super) fn plan_docs_dir(&self, plan_id: &str) -> PathBuf {
        self.issue_dir(plan_id).join("docs")
    }
    /// Ingest a worktree's plan docs into the plan's canonical store docs —
    /// the single plan doc (`plan_path`, worktree-relative) and every file in
    /// the multi-stage plan dir (`.build/plan/`, a flat dir of docs plus the
    /// manifest). **Fail-fast**, unlike the legacy snapshot mirror: an
    /// escaping `plan_path` is rejected, IO errors propagate, and finding
    /// nothing at all to ingest is an error — the caller only ingests when a
    /// `done` report claimed docs exist, and the plan must not advance with
    /// unpersisted docs. A re-ingest overwrites with the latest contents.
    pub fn ingest_plan_docs(
        &self,
        plan_id: &str,
        worktree_path: &Path,
        plan_path: &str,
    ) -> Result<(), StoreError> {
        if !is_worktree_contained_path(plan_path) {
            return Err(StoreError::PathEscape {
                path: plan_path.to_string(),
            });
        }
        let docs_root = self.plan_docs_dir(plan_id);
        let mut ingested_files = 0usize;
        let plan_source = worktree_path.join(plan_path);
        if plan_source.is_file() {
            let dest = docs_root.join(plan_path);
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(&plan_source, &dest)?;
            ingested_files += 1;
        }
        let stage_dir = worktree_path.join(STAGE_PLAN_DIR);
        if stage_dir.is_dir() {
            let dest_dir = docs_root.join(STAGE_PLAN_DIR);
            std::fs::create_dir_all(&dest_dir)?;
            let mut worktree_names = std::collections::HashSet::new();
            for entry in std::fs::read_dir(&stage_dir)? {
                let source = entry?.path();
                if !source.is_file() {
                    continue; // stage docs are a flat dir of markdown files
                }
                let Some(name) = source.file_name() else {
                    continue;
                };
                std::fs::copy(&source, dest_dir.join(name))?;
                worktree_names.insert(name.to_os_string());
                ingested_files += 1;
            }
            // The worktree's stage dir is the truth, deletions included: a
            // revision that drops or renames a stage doc must not leave the
            // stale file in the store, where the next run's materialization
            // would commit it (invisibly — the materialization commit is
            // excluded from review diffs via base_sha).
            for entry in std::fs::read_dir(&dest_dir)? {
                let stored = entry?.path();
                if !stored.is_file() {
                    continue;
                }
                let Some(name) = stored.file_name() else {
                    continue;
                };
                if !worktree_names.contains(name) {
                    remove_file_if_present(&stored)?;
                }
            }
        }
        if ingested_files == 0 {
            return Err(StoreError::NothingToIngest {
                plan_id: plan_id.to_string(),
                plan_path: plan_path.to_string(),
                worktree_path: worktree_path.to_path_buf(),
            });
        }
        Ok(())
    }
    /// Materialize a plan's canonical docs into a worktree, preserving the
    /// worktree-relative (`.build/…`) layout — the reverse of
    /// `ingest_plan_docs`; run dispatch commits the result. Erroring when the
    /// store holds no docs is deliberate: dispatching a planned run without
    /// its plan would silently build from nothing.
    pub fn materialize_plan_docs(
        &self,
        plan_id: &str,
        worktree_path: &Path,
    ) -> Result<(), StoreError> {
        let docs_root = self.plan_docs_dir(plan_id);
        if !docs_root.is_dir() {
            return Err(StoreError::NoStoredDocs {
                plan_id: plan_id.to_string(),
            });
        }
        let copied = copy_tree(&docs_root, worktree_path, &[])?;
        if copied == 0 {
            return Err(StoreError::NoStoredDocs {
                plan_id: plan_id.to_string(),
            });
        }
        Ok(())
    }
    /// Whether the canonical store holds any doc at all for this plan. False
    /// for a migrated plan whose docs were unrecoverable (worktree and branch
    /// both gone) — the UI gates doc reads and Implement on this instead of
    /// spinning on reads that can never succeed.
    pub fn has_plan_docs(&self, plan_id: &str) -> bool {
        dir_contains_a_file(&self.plan_docs_dir(plan_id))
    }
}

use crate::app::{
    archived_worktree_json, merge_archived_worktree_facts, plan_state_str, require_str,
    run_state_str, DigestScope,
};
use crate::run::RunState;
use crate::store::WorktreeFinishStatus;
use crate::thread::ThreadDetail;

use serde_json::{json, Value};

use super::super::AppState;

impl AppState {
    /// Archived plans and external worktrees for one project, grouped by kind.
    /// Canonical project path is the durable join because project ids remint.
    pub(crate) fn archive_list(&self, params: &Value) -> Result<Value, String> {
        let project_id = require_str(params, "project_id")?;
        let project = self
            .projects
            .iter()
            .find(|project| project.id == project_id)
            .ok_or("unknown project_id")?;
        let project_path = project.repo_path.display().to_string();
        let plans = self
            .plans
            .iter()
            .filter(|(plan_id, active)| {
                active.plan.archived_at.is_some() && self.project_path_for(plan_id) == project_path
            })
            .map(|(plan_id, active)| {
                self.plan_view(plan_id, active, ThreadDetail::Digest, DigestScope::List)
            })
            .collect::<Vec<_>>();
        let worktrees = self
            .board
            .archived_values()
            .filter(|record| {
                record.project_path == project_path
                    && record.status == WorktreeFinishStatus::Archived
            })
            .map(archived_worktree_json)
            .collect::<Vec<_>>();
        Ok(json!({ "plans": plans, "worktrees": worktrees }))
    }

    /// Everything finished, across every project, newest first: archived
    /// issues, archived runs, and the archived worktrees no run stands behind.
    ///
    /// The archive is the user's, not a project's, which is why this cannot be
    /// `archive.list` with the project left off — and it speaks the feed's two
    /// work items (Decisions §Entity model), so a finished run and the worktree
    /// record it left behind are ONE branch row. `(project, branch)` is the
    /// join: a deleted checkout's path no longer canonicalizes, so the worktree
    /// id cannot be recomputed from a run whose files are gone.
    pub(crate) fn archived_list(&self) -> Value {
        let mut items: Vec<Value> = self
            .plans
            .iter()
            .filter(|(_, active)| active.plan.archived_at.is_some())
            .map(|(issue_id, active)| {
                let mut row = self.archived_row("issue", issue_id);
                let object = row.as_object_mut().expect("archived_row is an object");
                object.insert("title".into(), json!(active.plan.goal));
                object.insert("state".into(), json!(plan_state_str(&active.plan.state)));
                object.insert("finished_at".into(), json!(active.plan.archived_at));
                object.insert("issue_id".into(), json!(issue_id));
                object.insert("stages".into(), json!(active.stages.len()));
                row
            })
            .collect();

        let mut branches_taken: std::collections::HashSet<(String, String)> =
            std::collections::HashSet::new();
        for (run_id, active) in &self.runs {
            if active.run.state != RunState::Archived {
                continue;
            }
            let branch = active.worktree.branch();
            let project_path = self.project_path_for(run_id);
            let record = self.board.archived_values().find(|record| {
                record.status == WorktreeFinishStatus::Archived
                    && record.project_path == project_path
                    && record.branch.as_deref() == Some(branch.as_str())
            });
            branches_taken.insert((project_path, branch.clone()));
            let mut row = self.archived_row("branch", run_id);
            let object = row.as_object_mut().expect("archived_row is an object");
            let title = if active.run.goal.trim().is_empty() {
                branch.clone()
            } else {
                active.run.goal.clone()
            };
            object.insert("title".into(), json!(title));
            object.insert("branch".into(), json!(branch));
            object.insert("state".into(), json!(run_state_str(&active.run.state)));
            object.insert(
                "finished_at".into(),
                json!(self
                    .board
                    .attention()
                    .clock(run_id)
                    .state_changed_at
                    .as_ref()),
            );
            object.insert("run_id".into(), json!(run_id));
            object.insert(
                "issue_id".into(),
                json!(active.run.plan_id.as_ref().map(|id| id.0.clone())),
            );
            object.insert(
                "worktree_path".into(),
                json!(active.worktree.path.display().to_string()),
            );
            if let Some(record) = record {
                merge_archived_worktree_facts(object, record);
            }
            items.push(row);
        }

        for record in self.board.archived_values() {
            if record.status != WorktreeFinishStatus::Archived {
                continue;
            }
            let branch = record.branch.clone();
            if branch.as_ref().is_some_and(|branch| {
                branches_taken.contains(&(record.project_path.clone(), branch.clone()))
            }) {
                continue;
            }
            let project = self
                .projects
                .iter()
                .find(|project| project.repo_path.display().to_string() == record.project_path);
            let mut row = json!({
                "kind": "branch",
                "project_id": project.map(|project| project.id.clone()),
                "project": project.map(|project| project.name.clone()),
                "title": branch.clone().unwrap_or_else(|| record.worktree_name.clone()),
                "branch": branch,
                "state": "archived",
                "finished_at": record.archived_at,
                "run_id": Value::Null,
                "issue_id": Value::Null,
                "stages": Value::Null,
                "worktree_path": record.worktree_path,
            });
            merge_archived_worktree_facts(row.as_object_mut().expect("built as an object"), record);
            items.push(row);
        }

        // Done removes the workspace, so what the archive lists is the record
        // the registry kept of it — plus any live record still standing as
        // finished, which is what an adopted checkout finished before this
        // rule left behind.
        let mut finished = self.workspaces.finished_records();
        let recorded = finished
            .iter()
            .map(|record| record.id.clone())
            .collect::<std::collections::HashSet<_>>();
        finished.extend(
            self.workspaces
                .list(None)
                .into_iter()
                .filter(|workspace| {
                    workspace.status == crate::workspace::WorkspaceStatus::Finished
                        && !recorded.contains(&workspace.id)
                })
                .cloned(),
        );
        finished.sort_by(|left, right| left.name.cmp(&right.name).then(left.id.cmp(&right.id)));
        for workspace in finished {
            let project = self.projects.get(&workspace.project_id);
            items.push(json!({
                "kind": "workspace",
                "workspace_id": workspace.id,
                "project_id": workspace.project_id,
                "project": project.map(|project| project.name.clone()),
                "title": workspace.name,
                "branch": Value::Null,
                "state": "finished",
                "action": "archive",
                "finished_at": workspace.archived_at,
                "run_id": Value::Null,
                "issue_id": Value::Null,
                "stages": Value::Null,
                "worktree_id": Value::Null,
                "worktree_path": workspace.root.display().to_string(),
                "head_sha": Value::Null,
                "upstream": Value::Null,
                "unpushed": Value::Null,
                "dirty_files": Value::Null,
            }));
        }

        // Newest first; an item whose stamp was never written sorts last rather
        // than jumping the queue.
        items.sort_by(|left, right| {
            let stamp = |row: &Value| row["finished_at"].as_str().unwrap_or_default().to_string();
            stamp(right).cmp(&stamp(left))
        });
        json!({ "items": items })
    }

    /// The keys every archived row carries, with the entity's project already
    /// resolved. The caller fills in the rest for its kind.
    pub(in crate::app) fn archived_row(&self, kind: &str, entity_id: &str) -> Value {
        json!({
            "kind": kind,
            "project_id": self.projects.project_id_of(entity_id),
            "project": self.project_name_of(entity_id),
            "title": Value::Null,
            "branch": Value::Null,
            "state": Value::Null,
            "action": Value::Null,
            "finished_at": Value::Null,
            "run_id": Value::Null,
            "issue_id": Value::Null,
            "stages": Value::Null,
            "worktree_id": Value::Null,
            "workspace_id": Value::Null,
            "worktree_path": Value::Null,
            "head_sha": Value::Null,
            "upstream": Value::Null,
            "unpushed": Value::Null,
            "dirty_files": Value::Null,
        })
    }
}

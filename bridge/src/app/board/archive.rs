use super::super::AppState;
use crate::app::{merge_archived_worktree_facts, plan_state_str, run_state_str};
use crate::run::RunState;
use crate::store::{PersistedArchivedWorktree, WorktreeFinishStatus};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

impl AppState {
    /// Capture bounded metadata per retained archive row, without joining or rendering.
    pub(in crate::app) fn archive_read_input(&self) -> ArchiveReadInput {
        let plans = self
            .plans
            .iter()
            .filter(|(_, active)| active.plan.archived_at.is_some())
            .map(|(id, active)| ArchivedEntity {
                kind: "task",
                id: id.clone(),
                project_id: self.projects.project_id_of(id).map(str::to_string),
                project: self.project_name_of(id),
                project_path: self.project_path_for(id),
                title: active.plan.goal.clone(),
                state: plan_state_str(&active.plan.state).into(),
                finished_at: active.plan.archived_at.clone(),
                branch: None,
                plan_id: Some(id.clone()),
                stages: Some(active.stages.len()),
                worktree_path: None,
            });
        let runs = self
            .runs
            .iter()
            .filter(|(_, active)| active.run.state == RunState::Archived)
            .map(|(id, active)| {
                let branch = self.observed_run_branch(id, &active.worktree);
                ArchivedEntity {
                    kind: "branch",
                    id: id.clone(),
                    project_id: self.projects.project_id_of(id).map(str::to_string),
                    project: self.project_name_of(id),
                    project_path: self.project_path_for(id),
                    title: if active.run.goal.trim().is_empty() {
                        branch.clone()
                    } else {
                        active.run.goal.clone()
                    },
                    state: run_state_str(&active.run.state).into(),
                    finished_at: self.board.attention().clock(id).state_changed_at.clone(),
                    branch: Some(branch),
                    plan_id: active.run.plan_id.as_ref().map(|id| id.0.clone()),
                    stages: None,
                    worktree_path: Some(active.worktree.path.display().to_string()),
                }
            });
        ArchiveReadInput {
            entities: plans.chain(runs).collect(),
            records: self
                .board
                .archived_values()
                .filter(|record| record.status == WorktreeFinishStatus::Archived)
                .cloned()
                .collect(),
            registry_root: self.workspaces.root().to_path_buf(),
            current: self
                .workspaces
                .list(None)
                .into_iter()
                .filter(|workspace| workspace.status == crate::workspace::WorkspaceStatus::Finished)
                .cloned()
                .collect(),
            projects: self
                .projects
                .iter()
                .map(|project| {
                    (
                        project.id.clone(),
                        (
                            project.name.clone(),
                            project.repo_path.display().to_string(),
                        ),
                    )
                })
                .collect(),
        }
    }

    /// Finished tasks, runs and retained workspace records, newest first.
    pub(crate) fn archived_list(&self) -> Value {
        self.archive_read_input().read()
    }
}

struct ArchivedEntity {
    kind: &'static str,
    id: String,
    project_id: Option<String>,
    project: String,
    project_path: String,
    title: String,
    state: String,
    finished_at: Option<String>,
    branch: Option<String>,
    plan_id: Option<String>,
    stages: Option<usize>,
    worktree_path: Option<String>,
}

pub(in crate::app) struct ArchiveReadInput {
    entities: Vec<ArchivedEntity>,
    records: Vec<PersistedArchivedWorktree>,
    registry_root: PathBuf,
    current: Vec<crate::workspace::Workspace>,
    projects: HashMap<String, (String, String)>,
}

impl ArchiveReadInput {
    pub(in crate::app) fn read(&self) -> Value {
        let mut index = HashMap::new();
        for record in &self.records {
            if let Some(branch) = &record.branch {
                index
                    .entry((record.project_path.as_str(), branch.as_str()))
                    .or_insert(record);
            }
        }
        let mut taken = HashSet::new();
        let mut items = Vec::new();
        for entity in &self.entities {
            let key = entity
                .branch
                .as_ref()
                .map(|branch| (entity.project_path.as_str(), branch.as_str()));
            let record = key.and_then(|key| index.get(&key));
            if let Some(branch) = &entity.branch {
                taken.insert((entity.project_path.clone(), branch.clone()));
            }
            let mut row = entity.render();
            if let Some(record) = record {
                merge_archived_worktree_facts(row.as_object_mut().unwrap(), record);
            }
            items.push(row);
        }
        for record in &self.records {
            if record.branch.as_ref().is_some_and(|branch| {
                taken.contains(&(record.project_path.clone(), branch.clone()))
            }) {
                continue;
            }
            let project = self
                .projects
                .iter()
                .find(|(_, (_, path))| *path == record.project_path);
            let mut row = json!({
                "kind":"branch", "project_id":project.map(|(id,_)| id), "project":project.map(|(_, (name,_))| name),
                "title":record.branch.clone().unwrap_or_else(|| record.worktree_name.clone()), "branch":record.branch,
                "state":"archived", "finished_at":record.archived_at, "run_id":Value::Null, "task_id":Value::Null,
                "stages":Value::Null, "worktree_path":record.worktree_path,
            });
            merge_archived_worktree_facts(row.as_object_mut().unwrap(), record);
            items.push(row);
        }
        self.append_workspaces(&mut items);
        items.sort_by(|left, right| {
            right["finished_at"]
                .as_str()
                .unwrap_or_default()
                .cmp(left["finished_at"].as_str().unwrap_or_default())
        });
        json!({"items":items})
    }

    fn append_workspaces(&self, items: &mut Vec<Value>) {
        let mut finished =
            crate::workspace::WorkspaceRegistry::finished_records_at(&self.registry_root);
        let recorded: HashSet<_> = finished.iter().map(|record| record.id.clone()).collect();
        finished.extend(
            self.current
                .iter()
                .filter(|workspace| !recorded.contains(&workspace.id))
                .cloned(),
        );
        finished.sort_by(|left, right| left.name.cmp(&right.name).then(left.id.cmp(&right.id)));
        for workspace in finished {
            items.push(json!({
                "kind":"workspace", "workspace_id":workspace.id, "project_id":workspace.project_id,
                "project":self.projects.get(&workspace.project_id).map(|(name,_)| name), "title":workspace.name,
                "branch":Value::Null, "state":"finished", "action":"archive", "finished_at":workspace.archived_at,
                "run_id":Value::Null, "task_id":Value::Null, "stages":Value::Null, "worktree_id":Value::Null,
                "worktree_path":workspace.root.display().to_string(), "head_sha":Value::Null, "upstream":Value::Null,
                "unpushed":Value::Null, "dirty_files":Value::Null,
            }));
        }
    }
}

impl ArchivedEntity {
    fn render(&self) -> Value {
        json!({
            "kind":self.kind, "project_id":self.project_id, "project":self.project, "title":self.title,
            "branch":self.branch, "state":self.state, "action":Value::Null, "finished_at":self.finished_at,
            "run_id":(self.kind == "branch").then_some(&self.id), "task_id":self.plan_id, "stages":self.stages,
            "worktree_id":Value::Null, "workspace_id":Value::Null, "worktree_path":self.worktree_path,
            "head_sha":Value::Null, "upstream":Value::Null, "unpushed":Value::Null, "dirty_files":Value::Null,
        })
    }
}

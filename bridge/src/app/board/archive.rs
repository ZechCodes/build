use super::super::AppState;
use crate::app::{merge_archived_worktree_facts, plan_state_str, run_state_str};
use crate::run::RunState;
use crate::store::{PersistedArchivedWorktree, WorktreeFinishStatus};
use im::OrdMap;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

impl AppState {
    /// Bootstrap/legacy adapter; ordinary publication captures keyed entities.
    pub(in crate::app) fn archive_read_input(&self) -> ArchiveReadInput {
        #[cfg(test)]
        crate::app::observation_reads::capture_metrics::record("archive_scan", "");
        let order: Vec<_> = self.plans.keys().chain(self.runs.keys()).cloned().collect();
        ArchiveReadInput {
            entities: order
                .iter()
                .filter_map(|id| self.archive_entity(id).map(|row| (id.clone(), row)))
                .collect(),
            entity_order: Arc::new(order),
            records: Arc::new(
                self.board
                    .archived_values()
                    .filter(|record| record.status == WorktreeFinishStatus::Archived)
                    .cloned()
                    .collect(),
            ),
            registry_root: self.workspaces.root().to_path_buf(),
            current: self
                .workspaces
                .list(None)
                .into_iter()
                .filter(|workspace| workspace.status == crate::workspace::WorkspaceStatus::Finished)
                .map(|workspace| (workspace.id.clone(), Arc::new(workspace.clone())))
                .collect(),
            projects: self
                .projects
                .iter()
                .map(|project| {
                    (
                        project.id.clone(),
                        Arc::new((
                            project.name.clone(),
                            project.repo_path.display().to_string(),
                        )),
                    )
                })
                .collect(),
            project_order: Arc::new(
                self.projects
                    .iter()
                    .map(|project| project.id.clone())
                    .collect(),
            ),
        }
    }

    pub(in crate::app) fn archive_entity(&self, id: &str) -> Option<Arc<ArchivedEntity>> {
        if let Some(active) = self
            .plans
            .get(id)
            .filter(|active| active.plan.archived_at.is_some())
        {
            return Some(Arc::new(ArchivedEntity {
                kind: "task",
                id: id.into(),
                project_id: self.projects.project_id_of(id).map(str::to_string),
                project: self.project_name_of(id),
                project_path: self.project_path_for(id),
                title: active.plan.goal.clone(),
                state: plan_state_str(&active.plan.state),
                finished_at: active.plan.archived_at.clone(),
                branch: None,
                plan_id: Some(id.into()),
                stages: Some(active.stages.len()),
                worktree_path: None,
            }));
        }
        let active = self
            .runs
            .get(id)
            .filter(|active| active.run.state == RunState::Archived)?;
        let branch = self.observed_run_branch(id, &active.worktree);
        Some(Arc::new(ArchivedEntity {
            kind: "branch",
            id: id.into(),
            project_id: self.projects.project_id_of(id).map(str::to_string),
            project: self.project_name_of(id),
            project_path: self.project_path_for(id),
            title: if active.run.goal.trim().is_empty() {
                branch.clone()
            } else {
                active.run.goal.clone()
            },
            state: run_state_str(&active.run.state),
            finished_at: self.board.attention().clock(id).state_changed_at.clone(),
            branch: Some(branch),
            plan_id: active.run.plan_id.as_ref().map(|id| id.0.clone()),
            stages: None,
            worktree_path: Some(active.worktree.path.display().to_string()),
        }))
    }

    /// Finished tasks, runs and retained workspace records, newest first.
    pub(crate) fn archived_list(&self) -> Value {
        self.archive_read_input().read()
    }
}

pub(in crate::app) struct ArchivedEntity {
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

#[derive(Clone)]
pub(in crate::app) struct ArchiveReadInput {
    entities: OrdMap<String, Arc<ArchivedEntity>>,
    entity_order: Arc<Vec<String>>,
    records: Arc<Vec<PersistedArchivedWorktree>>,
    registry_root: PathBuf,
    current: OrdMap<String, Arc<crate::workspace::Workspace>>,
    projects: OrdMap<String, Arc<(String, String)>>,
    project_order: Arc<Vec<String>>,
}

impl ArchiveReadInput {
    pub(in crate::app) fn apply_owner(&mut self, id: &str, row: &Option<Arc<ArchivedEntity>>) {
        match row {
            Some(row) => {
                self.entities.insert(id.into(), row.clone());
            }
            None => {
                self.entities.remove(id);
            }
        }
    }
    pub(in crate::app) fn owner_order(&mut self, order: Arc<Vec<String>>) {
        self.entity_order = order;
    }
    pub(in crate::app) fn project_order(&mut self, order: Arc<Vec<String>>) {
        self.project_order = order;
    }
    pub(in crate::app) fn project(&mut self, id: &str, row: &Option<Arc<(String, String)>>) {
        match row {
            Some(row) => {
                self.projects.insert(id.into(), row.clone());
            }
            None => {
                self.projects.remove(id);
            }
        }
    }
    pub(in crate::app) fn workspace(
        &mut self,
        id: &str,
        row: &Option<Arc<crate::workspace::Workspace>>,
    ) {
        if let Some(row) = row
            .as_ref()
            .filter(|row| row.status == crate::workspace::WorkspaceStatus::Finished)
        {
            self.current.insert(id.into(), row.clone());
        } else {
            self.current.remove(id);
        }
    }
    pub(in crate::app) fn records(&mut self, records: Arc<Vec<PersistedArchivedWorktree>>) {
        self.records = records;
    }

    pub(in crate::app) fn read(&self) -> Value {
        let mut index = HashMap::new();
        for record in self.records.iter() {
            if let Some(branch) = &record.branch {
                index
                    .entry((record.project_path.as_str(), branch.as_str()))
                    .or_insert(record);
            }
        }
        let mut taken = HashSet::new();
        let mut items = Vec::new();
        for entity in self
            .entity_order
            .iter()
            .filter_map(|id| self.entities.get(id))
        {
            let project = entity
                .project_id
                .as_ref()
                .and_then(|id| self.projects.get(id));
            let project_path = project
                .map(|row| row.1.as_str())
                .unwrap_or(&entity.project_path);
            let key = entity
                .branch
                .as_ref()
                .map(|branch| (project_path, branch.as_str()));
            let record = key.and_then(|key| index.get(&key));
            if let Some(branch) = &entity.branch {
                taken.insert((project_path.to_string(), branch.clone()));
            }
            let mut row = entity.render();
            if let Some(project) = project {
                row["project"] = json!(project.0);
            }
            if let Some(record) = record {
                merge_archived_worktree_facts(row.as_object_mut().unwrap(), record);
            }
            items.push(row);
        }
        for record in self.records.iter() {
            if record.branch.as_ref().is_some_and(|branch| {
                taken.contains(&(record.project_path.clone(), branch.clone()))
            }) {
                continue;
            }
            let project = self
                .project_order
                .iter()
                .filter_map(|id| self.projects.get(id).map(|row| (id, row.as_ref())))
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
                .values()
                .map(Arc::as_ref)
                .filter(|workspace| !recorded.contains(&workspace.id))
                .cloned(),
        );
        finished.sort_by(|left, right| left.name.cmp(&right.name).then(left.id.cmp(&right.id)));
        for workspace in finished {
            items.push(json!({
                "kind":"workspace", "workspace_id":workspace.id, "project_id":workspace.project_id,
                "project":self.projects.get(&workspace.project_id).map(|row|&row.0), "title":workspace.name,
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

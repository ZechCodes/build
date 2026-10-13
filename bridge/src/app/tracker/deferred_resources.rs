//! Owned resource identities for deferred assignments, independent of RPC slots.

use super::dispatch::AssignTarget;
use crate::app::{AppState, DeferredGit};
use crate::tracker::Actor;
use crate::workspace::{Workspace, WorkspaceSource};
use serde_json::{json, Value};

impl AppState {
    pub(in crate::app) fn reserve_deferred_workspace(
        &self,
        project_id: &str,
        target: &AssignTarget,
        task_title: &str,
        actor: &Actor,
    ) -> Result<Value, String> {
        let AssignTarget::NewWorkspace {
            name, isolation, ..
        } = target
        else {
            return Err("assignment does not create a workspace".into());
        };
        let sources = self
            .sources_for(project_id)?
            .into_iter()
            .map(|source| WorkspaceSource {
                id: source.id,
                name: source.name,
                mount: source.mount,
                path: source.path,
                is_git: source.is_git,
                base_branch: source.base_branch,
            })
            .collect::<Vec<_>>();
        if sources.is_empty() {
            return Err("workspace.create: project has no sources".into());
        }
        let isolation = isolation
            .as_deref()
            .map(|value| {
                crate::isolation::Isolation::from_wire(value)
                    .ok_or_else(|| format!("unknown isolation: {value}"))
            })
            .transpose()?
            .unwrap_or_else(|| {
                self.project(project_id)
                    .and_then(|project| project.isolation)
                    .unwrap_or(self.isolation)
            });
        let mut workspace = self.workspaces.reserve_delivery_workspace(
            project_id,
            name.as_deref().unwrap_or(task_title),
            &sources,
            isolation,
        )?;
        workspace.created_by_agent = matches!(actor, Actor::Agent { .. });
        serde_json::to_value(workspace).map_err(|error| error.to_string())
    }

    pub(in crate::app) fn prepare_deferred_workspace(
        &mut self,
        reservation: &Value,
    ) -> Result<DeferredGit, String> {
        let workspace: Workspace = serde_json::from_value(reservation.clone())
            .map_err(|error| format!("invalid workspace reservation: {error}"))?;
        self.prepare_deferred_workspace_snapshot(&workspace)
    }

    /// The caller checks assignment authority before entering each resource
    /// step. This boundary additionally checks the target's project ownership.
    pub(in crate::app) fn deferred_assignment_agent(
        &mut self,
        project_id: &str,
        target: &AssignTarget,
        workspace_id: Option<&str>,
        actor: &Actor,
        creation_id: &str,
    ) -> Result<(String, String, Option<String>), String> {
        self.project_for(project_id)?;
        let recovered = self.saved_resource_owner(project_id, creation_id)?;
        if let Some(owner) = recovered {
            return self.recover_assignment_agent(
                project_id,
                target,
                workspace_id,
                actor,
                creation_id,
                &owner,
            );
        }
        let (entity_id, workspace_id) = match target {
            AssignTarget::Agent { agent_id } => {
                let entity_id = self.agent_of_this_project(project_id, agent_id)?;
                let workspace_id = self.workspace_of_conversation(&entity_id);
                return Ok((agent_id.clone(), entity_id, workspace_id));
            }
            AssignTarget::ProjectAgent => {
                let entity_id = self.deferred_project_owner(project_id)?;
                let agent_id = self.ensure_primary_agent(&entity_id)?;
                return Ok((agent_id, entity_id, None));
            }
            AssignTarget::NewProjectAgent { .. } => {
                (self.deferred_project_owner(project_id)?, None)
            }
            AssignTarget::NewAgent { workspace_id, .. } => (
                self.deferred_workspace_owner(project_id, workspace_id, target)?,
                Some(workspace_id.clone()),
            ),
            AssignTarget::NewWorkspace { .. } => {
                let workspace_id =
                    workspace_id.ok_or("assignment has no reserved workspace identity")?;
                (
                    self.deferred_workspace_owner(project_id, workspace_id, target)?,
                    Some(workspace_id.to_string()),
                )
            }
            _ => return Err("assignment does not dispatch to an agent".into()),
        };
        let mut params = creating_agent_params(target, actor, creation_id)?;
        params["entity_id"] = json!(entity_id);
        if matches!(target, AssignTarget::NewProjectAgent { .. }) {
            params["project_role"] = json!("worker");
        }
        let added = self.agent_add(&params)?;
        let agent_id = added["agent"]["id"]
            .as_str()
            .ok_or("the added agent has no id")?
            .to_string();
        Ok((agent_id, entity_id, workspace_id))
    }

    /// A creation identity is global to the saved delivery, including owners
    /// no longer attached in memory. Refuse retirement before minting a new
    /// conversation, which would otherwise lose roster-local deduplication.
    fn saved_resource_owner(
        &self,
        project_id: &str,
        creation_id: &str,
    ) -> Result<Option<String>, String> {
        let live = self.runs.iter().find(|(_, run)| {
            run.agents
                .iter()
                .any(|agent| agent.creation_id.as_deref() == Some(creation_id))
        });
        if let Some((owner, run)) = live {
            if run.run.state.is_terminal() || self.projects.project_id_of(owner) != Some(project_id)
            {
                return Err("saved task delivery owner was retired or changed project".into());
            }
        }
        let Some(store) = &self.store else {
            return Ok(live.map(|(owner, _)| owner.clone()));
        };
        let saved = store
            .load_all_run_rosters()
            .map_err(|error| error.to_string())?;
        for run in saved.iter().filter(|run| {
            run.agents
                .iter()
                .any(|agent| agent.creation_id.as_deref() == Some(creation_id))
        }) {
            if run.state.is_terminal()
                || !self.runs.contains_key(&run.id)
                || self.projects.project_id_of(&run.id) != Some(project_id)
            {
                return Err("saved task delivery owner was retired or changed project".into());
            }
        }
        Ok(live.map(|(owner, _)| owner.clone()))
    }

    fn recover_assignment_agent(
        &mut self,
        project_id: &str,
        target: &AssignTarget,
        workspace_id: Option<&str>,
        actor: &Actor,
        creation_id: &str,
        owner: &str,
    ) -> Result<(String, String, Option<String>), String> {
        let expected_workspace = match target {
            AssignTarget::NewWorkspace { .. } => workspace_id,
            AssignTarget::NewAgent { workspace_id, .. } => Some(workspace_id.as_str()),
            AssignTarget::NewProjectAgent { .. } => None,
            _ => return Err("saved creation does not match assignment target".into()),
        };
        let actual_workspace = self.workspace_of_conversation(owner);
        if actual_workspace.as_deref() != expected_workspace
            || self.projects.project_id_of(owner) != Some(project_id)
        {
            return Err("saved task delivery owner no longer matches its target".into());
        }
        let mut params = creating_agent_params(target, actor, creation_id)?;
        params["entity_id"] = json!(owner);
        if matches!(target, AssignTarget::NewProjectAgent { .. }) {
            params["project_role"] = json!("worker");
        }
        let added = self.agent_add(&params)?;
        let agent_id = added["agent"]["id"]
            .as_str()
            .ok_or("saved agent has no id")?
            .to_string();
        Ok((agent_id, owner.to_string(), actual_workspace))
    }

    fn deferred_project_owner(&mut self, project_id: &str) -> Result<String, String> {
        let conversation =
            self.project_ensure_conversation(&json!({ "project_id": project_id }))?;
        conversation["run_id"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| "the project conversation has no owner".into())
    }

    fn deferred_workspace_owner(
        &mut self,
        project_id: &str,
        workspace_id: &str,
        target: &AssignTarget,
    ) -> Result<String, String> {
        let workspace = self
            .workspaces
            .get(workspace_id)
            .ok_or_else(|| format!("unknown workspace_id: {workspace_id}"))?;
        if workspace.project_id != project_id {
            return Err(format!(
                "workspace {workspace_id} is not in project {project_id}"
            ));
        }
        let mut params = creating_agent_params(target, &Actor::User, "conversation")?;
        params["workspace_id"] = json!(workspace_id);
        let conversation = self.workspace_ensure_conversation(&params)?;
        conversation["run_id"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| "the workspace conversation has no owner".into())
    }
}

fn creating_agent_params(
    target: &AssignTarget,
    actor: &Actor,
    creation_id: &str,
) -> Result<Value, String> {
    let (choice, notify_user, agent_name) = match target {
        AssignTarget::NewWorkspace {
            choice,
            notify_user,
            agent_name,
            ..
        }
        | AssignTarget::NewAgent {
            choice,
            notify_user,
            agent_name,
            ..
        }
        | AssignTarget::NewProjectAgent {
            choice,
            notify_user,
            agent_name,
        } => (choice, notify_user, agent_name),
        _ => return Err("assignment does not create an agent".into()),
    };
    let mut params = choice.args().wire();
    params["creation_id"] = json!(creation_id);
    params["made_by_agent"] = json!(true);
    params["notify_user"] = json!(notify_user.unwrap_or(matches!(actor, Actor::User)));
    if let Some(name) = agent_name {
        params["name"] = json!(name);
    }
    if let Some(created_by) = actor.agent_id() {
        params["created_by"] = json!(created_by);
    }
    Ok(params)
}

#[cfg(test)]
mod tests {
    use crate::app::tracker::dispatch::AssignTarget;
    use crate::app::AppState;
    use crate::tracker::Actor;
    use crate::workspace::Workspace;
    use std::path::Path;

    fn state(root: &Path) -> AppState {
        let context =
            crate::harness::HarnessContext::resolved(root.join("mcp.sock"), root.to_path_buf())
                .unwrap();
        AppState::new_unrooted_configured(root.join("worktrees"), "main", true, context)
    }

    #[test]
    fn deferred_resource_snapshot_keeps_source_inputs_after_project_changes() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        std::fs::create_dir(&source).unwrap();
        std::fs::write(source.join("saved-file"), b"original").unwrap();
        let mut app = state(temp.path());
        let project_id = app.add_project(source.clone(), "saved-base".into());
        let target = AssignTarget::NewWorkspace {
            name: None,
            isolation: None,
            choice: Default::default(),
            notify_user: None,
            agent_name: None,
        };
        let saved = app
            .reserve_deferred_workspace(&project_id, &target, "saved title", &Actor::User)
            .unwrap();
        let workspace: Workspace = serde_json::from_value(saved.clone()).unwrap();
        assert!(!workspace.root.exists());
        assert_eq!(workspace.directories[0].source_path, source);
        assert_eq!(workspace.directories[0].base_branch, "saved-base");
        let replacement = temp.path().join("replacement");
        std::fs::create_dir(&replacement).unwrap();
        let live = app
            .projects
            .source_mut(&project_id, &workspace.directories[0].source_id)
            .unwrap();
        live.path = replacement;
        live.base_branch = "new-base".into();
        let job = app.prepare_deferred_workspace(&saved).unwrap();
        assert!(job.params.is_null());
        assert!(app.deferred_work.is_none());
        assert_eq!(
            app.workspaces.get(&workspace.id).unwrap().root,
            workspace.root
        );
        job.call.run(&job.params).unwrap();
        assert_eq!(
            std::fs::read(workspace.directories[0].path.join("saved-file")).unwrap(),
            b"original"
        );
    }

    #[test]
    fn deferred_project_worker_creation_recovers_the_saved_roster_on_restart() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        std::fs::create_dir(&source).unwrap();
        let config = temp.path().join("config.json");
        let store = temp.path().join("store");
        let target = AssignTarget::NewProjectAgent {
            choice: Default::default(),
            notify_user: None,
            agent_name: Some("Reserved worker".into()),
        };
        let (project_id, first) = {
            let mut app = state(temp.path())
                .with_config(&config)
                .unwrap()
                .with_task_store(&store)
                .unwrap();
            let project_id = app.add_project(source, "main".into());
            app.persist();
            let first = app
                .deferred_assignment_agent(&project_id, &target, None, &Actor::User, "delivery-one")
                .unwrap();
            let repeated = app
                .deferred_assignment_agent(&project_id, &target, None, &Actor::User, "delivery-one")
                .unwrap();
            assert_eq!(first, repeated);
            assert_eq!(
                app.entity_agents(&first.1).unwrap().len(),
                2,
                "one pin and one worker"
            );
            (project_id, first)
        };
        let mut restarted = state(temp.path())
            .with_config(&config)
            .unwrap()
            .with_task_store(&store)
            .unwrap();
        let recovered = restarted
            .deferred_assignment_agent(&project_id, &target, None, &Actor::User, "delivery-one")
            .unwrap();
        assert_eq!(recovered, first);
        assert_eq!(restarted.entity_agents(&recovered.1).unwrap().len(), 2);
    }

    #[test]
    fn deferred_workspace_creation_retry_refuses_retired_saved_owner() {
        for retained_live in [true, false] {
            let temp = tempfile::tempdir().unwrap();
            let source = temp.path().join("source");
            std::fs::create_dir(&source).unwrap();
            let mut app = state(temp.path())
                .with_task_store(temp.path().join("store"))
                .unwrap();
            let project_id = app.add_project(source, "main".into());
            let target = AssignTarget::NewWorkspace {
                name: None,
                isolation: None,
                choice: Default::default(),
                notify_user: None,
                agent_name: None,
            };
            let saved = app
                .reserve_deferred_workspace(&project_id, &target, "reserved", &Actor::User)
                .unwrap();
            let job = app.prepare_deferred_workspace(&saved).unwrap();
            job.call.run(&job.params).unwrap();
            job.call.invalidate(&mut app);
            let workspace_id = saved["id"].as_str().unwrap();
            let first = app
                .deferred_assignment_agent(
                    &project_id,
                    &target,
                    Some(workspace_id),
                    &Actor::User,
                    "retired-reservation",
                )
                .unwrap();
            let mut retired = app.runs.remove(&first.1).unwrap();
            retired.run.state = crate::run::RunState::Archived;
            app.persist_run_record(&first.1, &retired).unwrap();
            if retained_live {
                app.runs.insert(first.1.clone(), retired);
            }
            let before = app.runs.len();
            let result = app.deferred_assignment_agent(
                &project_id,
                &target,
                Some(workspace_id),
                &Actor::User,
                "retired-reservation",
            );
            assert!(
                result.is_err(),
                "retired reservation was recreated: {result:?}"
            );
            assert_eq!(app.runs.len(), before, "retry created a replacement owner");
        }
    }

    #[test]
    fn deferred_workspace_agent_refuses_a_workspace_from_another_project() {
        let temp = tempfile::tempdir().unwrap();
        let mut app = state(temp.path());
        let first_source = temp.path().join("one");
        let second_source = temp.path().join("two");
        std::fs::create_dir(&first_source).unwrap();
        std::fs::create_dir(&second_source).unwrap();
        let first = app.add_project(first_source, "main".into());
        let second = app.add_project(second_source, "main".into());
        let creating = AssignTarget::NewWorkspace {
            name: None,
            isolation: None,
            choice: Default::default(),
            notify_user: None,
            agent_name: None,
        };
        let saved = app
            .reserve_deferred_workspace(&first, &creating, "isolated", &Actor::User)
            .unwrap();
        let job = app.prepare_deferred_workspace(&saved).unwrap();
        job.call.run(&job.params).unwrap();
        job.call.invalidate(&mut app);
        let id = saved["id"].as_str().unwrap().to_string();
        let target = AssignTarget::NewAgent {
            workspace_id: id,
            choice: Default::default(),
            notify_user: None,
            agent_name: None,
        };
        let before = app.runs.len();
        let error = app
            .deferred_assignment_agent(&second, &target, None, &Actor::User, "foreign")
            .unwrap_err();
        assert!(error.contains("not in project"), "{error}");
        assert_eq!(app.runs.len(), before);
    }
}

use crate::app::AppState;
use crate::compaction::{
    CompactionCleanup, CompactionScope, CompactionSource, CompactionThresholds,
    ResolvedCompactionPolicy,
};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, PartialEq, Eq)]
pub(in crate::app) struct CompactionPolicies {
    pub(in crate::app) device: CompactionThresholds,
    projects: BTreeMap<String, CompactionThresholds>,
    workspaces: BTreeMap<String, CompactionThresholds>,
}

impl Default for CompactionPolicies {
    fn default() -> Self {
        Self {
            device: CompactionThresholds::broadcast(crate::agent::DEFAULT_COMPACT_ABOVE_TOKENS),
            projects: BTreeMap::new(),
            workspaces: BTreeMap::new(),
        }
    }
}

impl CompactionPolicies {
    pub(in crate::app) fn from_config(config: &Value) -> Result<Self, String> {
        let device = match config.get("compaction_thresholds") {
            Some(value) => CompactionThresholds::from_config(value)?,
            None => CompactionThresholds::broadcast(
                config
                    .get("compact_above_tokens")
                    .and_then(Value::as_u64)
                    .filter(|tokens| *tokens > 0)
                    .unwrap_or(crate::agent::DEFAULT_COMPACT_ABOVE_TOKENS),
            ),
        };
        let mut policies = Self {
            device,
            projects: BTreeMap::new(),
            workspaces: BTreeMap::new(),
        };
        for project in config
            .get("projects")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let (Some(id), Some(map)) =
                (project["id"].as_str(), project.get("compaction_thresholds"))
            {
                policies
                    .projects
                    .insert(id.into(), CompactionThresholds::from_config(map)?);
            }
        }
        if let Some(value) = config.get("workspace_compaction_thresholds") {
            let workspaces = value
                .as_object()
                .ok_or("workspace_compaction_thresholds must be an object")?;
            for (id, map) in workspaces {
                policies
                    .workspaces
                    .insert(id.clone(), CompactionThresholds::from_config(map)?);
            }
        }
        Ok(policies)
    }

    pub(in crate::app) fn write_config(&self, config: &mut Value) {
        config["compaction_thresholds"] = self.device.stored_value();
        config["workspace_compaction_thresholds"] = Value::Object(
            self.workspaces
                .iter()
                .map(|(id, map)| (id.clone(), map.stored_value()))
                .collect(),
        );
        for entry in config["projects"].as_array_mut().into_iter().flatten() {
            if let Some(map) = entry["id"].as_str().and_then(|id| self.projects.get(id)) {
                entry["compaction_thresholds"] = map.stored_value();
            }
        }
    }

    fn get(&self, scope: &CompactionScope) -> CompactionThresholds {
        match scope {
            CompactionScope::Device => self.device.clone(),
            CompactionScope::Project(id) => self.projects.get(id).cloned().unwrap_or_default(),
            CompactionScope::Workspace(id) => self.workspaces.get(id).cloned().unwrap_or_default(),
        }
    }

    fn set(&mut self, scope: CompactionScope, map: CompactionThresholds) {
        match scope {
            CompactionScope::Device => self.device = map,
            CompactionScope::Project(id) => {
                self.projects.insert(id, map);
            }
            CompactionScope::Workspace(id) => {
                self.workspaces.insert(id, map);
            }
        }
    }

    pub(in crate::app) fn forget_project(&mut self, project: &str) {
        self.projects.remove(project);
    }

    pub(in crate::app) fn forget_workspaces(&mut self, ids: &[String]) {
        for id in ids {
            self.workspaces.remove(id);
        }
    }
}

impl AppState {
    /// Internal storage API. No wire setter or harness side effect is implied.
    pub fn compaction_thresholds(
        &self,
        scope: &CompactionScope,
    ) -> Result<CompactionThresholds, String> {
        self.validate_compaction_scope(scope)?;
        Ok(self.compaction_policies.get(scope))
    }

    fn validate_compaction_scope(&self, scope: &CompactionScope) -> Result<(), String> {
        let known = match scope {
            CompactionScope::Device => true,
            CompactionScope::Project(id) => {
                self.projects.get(id).is_some()
                    || self
                        .projects
                        .parked()
                        .iter()
                        .any(|entry| entry["id"].as_str() == Some(id))
            }
            CompactionScope::Workspace(id) => self.workspaces.get(id).is_some(),
        };
        if known {
            Ok(())
        } else {
            Err("unknown compaction policy owner".into())
        }
    }

    /// Parse the whole provider patch, persist a prospective config, then
    /// publish memory. Empty patches are true no-ops, even on unwritable disk.
    pub fn patch_compaction_thresholds(
        &mut self,
        scope: CompactionScope,
        patch: &Value,
    ) -> Result<(), String> {
        let current = self.compaction_thresholds(&scope)?;
        let updated = current.patched(patch)?;
        if current == updated {
            return Ok(());
        }
        let mut prospective = self.compaction_policies.clone();
        prospective.set(scope.clone(), updated);
        self.persist_compaction_policies(prospective)?;
        let (kind, owner) = scope.event_address();
        self.note_compaction_changed(kind, owner);
        Ok(())
    }

    fn persist_compaction_policies(
        &mut self,
        prospective: CompactionPolicies,
    ) -> Result<(), String> {
        let mut config =
            self.config_value(&self.projects_dir, self.default_harness, self.isolation);
        prospective.write_config(&mut config);
        let committed = std::mem::replace(&mut self.compaction_policies, prospective);
        let projection = self.prepare_all_snapshot_projection();
        let prospective = std::mem::replace(&mut self.compaction_policies, committed);
        self.persist_config(&config)?;
        self.compaction_policies = prospective;
        if let Some(projection) = projection {
            self.publish_prepared_snapshot_projection(projection);
        }
        Ok(())
    }

    /// Called only after a deletion has succeeded, never merely because a
    /// workspace is absent from a boot-time listing or its project is parked.
    pub(in crate::app) fn cleanup_deleted_workspace_compaction(
        &mut self,
        ids: &[String],
    ) -> CompactionCleanup {
        if !ids
            .iter()
            .any(|id| self.compaction_policies.workspaces.contains_key(id))
        {
            return CompactionCleanup::Complete;
        }
        let mut prospective = self.compaction_policies.clone();
        prospective.forget_workspaces(ids);
        match self.persist_compaction_policies(prospective) {
            Ok(()) => CompactionCleanup::Complete,
            Err(error) => {
                eprintln!("retain compaction policies for deleted workspaces {ids:?}: {error}");
                CompactionCleanup::Retained { error }
            }
        }
    }

    pub(in crate::app) fn canonical_compaction_override(
        &self,
        owner: &str,
        agent_id: &str,
    ) -> Result<Option<u64>, String> {
        let address = self.resolve_conversation_address(owner, Some(agent_id))?;
        Ok(self
            .entity_agents(&address.conversation_entity_id)?
            .resolve(Some(&address.conversation_id))?
            .max_context_tokens
            .filter(|tokens| *tokens > 0))
    }

    /// Addressed launch provider, canonical history override and owner scopes.
    /// This resolves configured intent only; it does not claim active policy.
    pub fn resolve_compaction_policy(
        &self,
        owner: &str,
        agent_id: &str,
    ) -> Result<ResolvedCompactionPolicy, String> {
        let provider = self
            .entity_agents(owner)?
            .resolve(Some(agent_id))?
            .choice
            .provider;
        self.resolve_compaction_policy_for_provider(owner, agent_id, provider)
    }

    /// An admitted queued launch keeps its captured provider, even if the
    /// addressed agent's choice has since changed.
    pub(in crate::app) fn resolve_compaction_policy_for_provider(
        &self,
        owner: &str,
        agent_id: &str,
        provider: crate::models::AgentProvider,
    ) -> Result<ResolvedCompactionPolicy, String> {
        self.resolve_compaction_with_override(owner, agent_id, provider, true, false)
    }

    pub(in crate::app) fn inherited_compaction_policy(
        &self,
        owner: &str,
        agent_id: &str,
    ) -> Result<ResolvedCompactionPolicy, String> {
        let provider = self
            .entity_agents(owner)?
            .resolve(Some(agent_id))?
            .choice
            .provider;
        self.resolve_compaction_with_override(owner, agent_id, provider, false, false)
    }

    pub(in crate::app) fn resolve_snapshot_compaction_policy(
        &self,
        owner: &str,
        agent_id: &str,
        include_override: bool,
    ) -> Result<ResolvedCompactionPolicy, String> {
        let provider = self
            .entity_agents(owner)?
            .resolve(Some(agent_id))?
            .choice
            .provider;
        self.resolve_compaction_with_override(owner, agent_id, provider, include_override, true)
    }

    fn resolve_compaction_with_override(
        &self,
        owner: &str,
        agent_id: &str,
        provider: crate::models::AgentProvider,
        include_override: bool,
        observed: bool,
    ) -> Result<ResolvedCompactionPolicy, String> {
        let address = self.resolve_conversation_address(owner, Some(agent_id))?;
        let canonical_owner = &address.conversation_entity_id;
        let mut candidates = vec![(
            if include_override {
                self.canonical_compaction_override(owner, agent_id)?
            } else {
                None
            },
            CompactionSource::Conversation(address.conversation_id),
        )];
        let project_owner = if observed {
            self.is_project_snapshot_conversation_owner(canonical_owner)
        } else {
            self.is_project_conversation_owner(canonical_owner)
        };
        if !project_owner {
            if let Some(workspace) = self.compaction_workspace_of(canonical_owner, observed) {
                candidates.push((
                    self.compaction_policies
                        .get(&CompactionScope::Workspace(workspace.clone()))
                        .get(provider),
                    CompactionSource::Workspace(workspace),
                ));
            }
        }
        if let Some(project) = self.compaction_project_of(canonical_owner) {
            candidates.push((
                self.compaction_policies
                    .get(&CompactionScope::Project(project.clone()))
                    .get(provider),
                CompactionSource::Project(project),
            ));
        }
        candidates.push((
            self.compaction_policies.device.get(provider),
            CompactionSource::Device,
        ));
        candidates.push((
            Some(crate::agent::DEFAULT_COMPACT_ABOVE_TOKENS),
            CompactionSource::Default,
        ));
        let (tokens, source) = candidates
            .into_iter()
            .find_map(|(tokens, source)| tokens.map(|tokens| (tokens, source)))
            .expect("the built-in default is always present");
        Ok(ResolvedCompactionPolicy { tokens, source })
    }

    pub(in crate::app) fn compaction_project_of(&self, owner: &str) -> Option<String> {
        if let Some(id) = self.projects.project_id_of(owner) {
            return Some(id.to_string());
        }
        let retained_path = self.projects.project_path_for(owner);
        self.projects
            .parked()
            .iter()
            .find(|entry| {
                crate::app::projects::configured_primary_path(entry) == Some(retained_path.as_str())
            })
            .and_then(|entry| entry["id"].as_str())
            .map(str::to_owned)
    }

    fn compaction_workspace_of(&self, owner: &str, observed: bool) -> Option<String> {
        if observed {
            if let Some(context) = self.snapshot_ownership_context() {
                return context.compaction_workspace(owner).map(str::to_owned);
            }
        }
        let workspace = if observed {
            self.workspace_snapshot_for_conversation(owner, None)
        } else {
            self.workspace_for_conversation(owner, None)
        };
        if let Some(workspace) = workspace {
            return Some(workspace.id.clone());
        }
        if self.projects.project_id_of(owner).is_some() {
            return None;
        }
        // A parked project's durable owner keeps its recorded project path,
        // but has no live registry binding until the source returns.
        let project = self.compaction_project_of(owner)?;
        let run = self.runs.get(owner)?;
        if run.run.state.is_terminal() {
            return None;
        }
        self.workspaces
            .list(Some(&project))
            .into_iter()
            .find(|workspace| {
                if observed {
                    self.observed_same_path(&workspace.root, &run.worktree.path)
                } else {
                    crate::app::workspaces::same_path(&workspace.root, &run.worktree.path)
                }
            })
            .map(|workspace| workspace.id.clone())
    }

    pub(in crate::app) fn note_compaction_override_changed(&self, conversation: &str) {
        for owner in self.runs.keys().chain(self.plans.keys()) {
            if self.entity_agents(owner).is_ok_and(|agents| {
                agents
                    .iter()
                    .any(|agent| agent.conversation_id() == conversation)
            }) {
                self.note_entity_changed(owner);
                self.publish_live_roster_for(owner);
            }
        }
    }
}

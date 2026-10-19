//! Scoped wire observations and invalidations; no session mutation belongs here.
use crate::app::AppState;
use crate::compaction::{CompactionScope, CompactionSource, ResolvedCompactionPolicy};
use crate::harness::compaction::{AppliedCompactionPolicy, CompactionEnforcement};
use crate::models::AgentProvider;
use serde_json::{json, Value};

impl AppState {
    fn scope_parent(&self, scope: &CompactionScope) -> Option<CompactionScope> {
        match scope {
            CompactionScope::Device => None,
            CompactionScope::Project(_) => Some(CompactionScope::Device),
            CompactionScope::Workspace(id) => self
                .workspaces
                .get(id)
                .map(|workspace| CompactionScope::Project(workspace.project_id.clone())),
        }
    }

    fn scope_policy(
        &self,
        scope: Option<&CompactionScope>,
        provider: AgentProvider,
    ) -> ResolvedCompactionPolicy {
        if let Some(scope) = scope {
            if let Some(tokens) = self
                .compaction_thresholds(scope)
                .ok()
                .and_then(|map| map.get(provider))
            {
                return ResolvedCompactionPolicy {
                    tokens,
                    source: scope.source(),
                };
            }
            return self.scope_policy(self.scope_parent(scope).as_ref(), provider);
        }
        ResolvedCompactionPolicy {
            tokens: crate::agent::DEFAULT_COMPACT_ABOVE_TOKENS,
            source: CompactionSource::Default,
        }
    }

    pub(in crate::app) fn scope_compaction_observation(&self, scope: &CompactionScope) -> Value {
        let parent = self.scope_parent(scope);
        let mut inherited = serde_json::Map::new();
        let mut effective = serde_json::Map::new();
        for provider in AgentProvider::ALL {
            inherited.insert(
                provider.wire_id().into(),
                self.scope_policy(parent.as_ref(), provider).observation(),
            );
            effective.insert(
                provider.wire_id().into(),
                self.scope_policy(Some(scope), provider).observation(),
            );
        }
        json!({"inherited": inherited, "effective": effective})
    }

    pub(in crate::app) fn add_scope_compaction(&self, record: &mut Value, scope: &CompactionScope) {
        record["compaction_thresholds"] = self
            .compaction_thresholds(scope)
            .unwrap_or_default()
            .read_value();
        record["compaction"] = self.scope_compaction_observation(scope);
    }

    pub(crate) fn set_project_compaction(
        &mut self,
        project: &str,
        patch: &Value,
    ) -> Result<Value, String> {
        // Require a live project record before committing: the response must be authoritative.
        if self.projects.get(project).is_none() {
            return Err("unknown compaction policy owner".into());
        }
        self.patch_compaction_thresholds(CompactionScope::Project(project.into()), patch)?;
        Ok(self.project_json(self.projects.get(project).expect("validated project")))
    }

    pub(crate) fn set_workspace_compaction(
        &mut self,
        workspace: &str,
        patch: &Value,
    ) -> Result<Value, String> {
        self.patch_compaction_thresholds(CompactionScope::Workspace(workspace.into()), patch)?;
        self.workspace_get(&json!({"workspace_id": workspace}))
    }

    pub(in crate::app) fn agent_compaction_observation(
        &self,
        owner: &str,
        agent: &str,
        applied: Option<&AppliedCompactionPolicy>,
    ) -> Value {
        let configured = self.resolve_compaction_policy(owner, agent).ok();
        let inherited = self.inherited_compaction_policy(owner, agent).ok();
        let (enforcement, reason) = match applied {
            None => ("unsupported", Some("active_policy_unknown")),
            Some(policy) => (
                match policy.enforcement {
                    CompactionEnforcement::Native => "native",
                    CompactionEnforcement::BetweenTurn => "between_turn",
                    CompactionEnforcement::Unsupported => "unsupported",
                },
                policy.reason.as_deref(),
            ),
        };
        let active = applied.map(|policy| {
            let mut observation = policy.requested.observation();
            observation["evidence"] = json!("applied_at_start");
            observation["provider"] = json!(policy.choice.provider);
            observation["qualification"] = json!({"version": policy.capabilities.qualification.version, "evidence": policy.capabilities.qualification.evidence});
            observation["translated_trigger"] = json!(policy.translated_trigger);
            observation
        });
        let pending = applied
            .zip(configured.as_ref())
            .map(|(applied, configured)| {
                applied.requested != *configured
                    || self
                        .entity_agents(owner)
                        .ok()
                        .and_then(|agents| agents.resolve(Some(agent)).ok())
                        .is_some_and(|agent| agent.choice.provider != applied.choice.provider)
            });
        json!({"configured": configured.map(|policy| policy.observation()),
            "inherited": inherited.map(|policy| policy.observation()), "active": active,
            "enforcement": enforcement, "reason": reason, "pending_change": pending})
    }

    pub(in crate::app) fn publish_compaction_changed(&self, scope: &str, owner: Option<&str>) {
        self.changes().note_compaction(scope, owner);
    }
}

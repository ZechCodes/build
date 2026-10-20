//! Bounded workspace detail projections. History is attached by the immutable
//! read service after it selects the requested conversation and cursor.
use super::workspace_json;
use crate::app::read_view::{ReadView, SnapshotDelta};
use crate::app::{AppState, DigestScope};
use crate::thread::ThreadDetail;
use crate::workspace::Workspace;
use serde_json::{json, Value};
use std::collections::BTreeSet;

impl AppState {
    pub(in crate::app) fn prepare_workspace_snapshot_details(
        &self,
        mut delta: SnapshotDelta,
        previous: &ReadView,
        owner: Option<&str>,
    ) -> SnapshotDelta {
        let changed = self.snapshot_changed_agent_ids(previous, owner);
        let mut affected = BTreeSet::new();
        let mut retained = BTreeSet::new();
        for key in previous
            .keys()
            .filter(|key| key.method == "workspace.get" && !key.owner.contains('/'))
        {
            if previous
                .get(key.method, &key.owner)
                .is_some_and(|row| depends_on_owner(row, owner, &changed))
            {
                affected.insert(key.owner.clone());
            }
        }
        for workspace in self.workspaces.list(None) {
            if !affected.contains(&workspace.id)
                && !self.workspace_snapshot_depends(workspace, owner, &changed)
            {
                continue;
            }
            affected.insert(workspace.id.clone());
            let value = self.workspace_snapshot_base(workspace);
            retained.insert(workspace.id.clone());
            if previous.get("workspace.get", &workspace.id) != Some(&value) {
                delta = delta.put("workspace.get", &workspace.id, value.clone());
            }
            delta = self.prepare_workspace_agent_variants(
                delta,
                previous,
                workspace,
                &value,
                &mut retained,
            );
        }
        for key in previous.keys().filter(|key| key.method == "workspace.get") {
            let workspace = key.owner.split('/').next().unwrap_or_default();
            if affected.contains(workspace) && !retained.contains(&key.owner) {
                delta = delta.remove("workspace.get", key.owner.clone());
            }
        }
        delta
    }

    fn prepare_workspace_agent_variants(
        &self,
        mut delta: SnapshotDelta,
        previous: &ReadView,
        workspace: &Workspace,
        value: &Value,
        retained: &mut BTreeSet<String>,
    ) -> SnapshotDelta {
        let Some(owner) = value["entity_id"].as_str() else {
            return delta;
        };
        let Ok(roster) = self.entity_agents(owner) else {
            return delta;
        };
        for agent in roster.iter() {
            let Ok(thread) = self.agent_conversation(owner, Some(&agent.id)) else {
                continue;
            };
            let key = format!("{}/{}", workspace.id, agent.id);
            let mut variant = value.clone();
            variant["thread"] = self.snapshot_thread_digest(thread);
            variant["run"]["thread"] = variant["thread"].clone();
            retained.insert(key.clone());
            if previous.get("workspace.get", &key) != Some(&variant) {
                delta = delta.put("workspace.get", key, variant);
            }
        }
        delta
    }

    fn snapshot_changed_agent_ids(
        &self,
        previous: &ReadView,
        owner: Option<&str>,
    ) -> BTreeSet<String> {
        let Some(owner) = owner else {
            return BTreeSet::new();
        };
        let current = self
            .entity_agents(owner)
            .ok()
            .into_iter()
            .flat_map(|roster| roster.iter().map(|agent| agent.id.clone()));
        let previous = previous
            .get("agent.list", owner)
            .into_iter()
            .flat_map(|row| row["agents"].as_array().into_iter().flatten())
            .filter_map(|agent| agent["id"].as_str().map(str::to_owned));
        current.chain(previous).collect()
    }

    fn workspace_snapshot_depends(
        &self,
        workspace: &Workspace,
        owner: Option<&str>,
        changed: &BTreeSet<String>,
    ) -> bool {
        let Some(owner) = owner else {
            return true;
        };
        let Some(workspace_owner) = self.workspace_snapshot_conversation_owner(workspace) else {
            return false;
        };
        workspace_owner == owner
            || self
                .entity_agents(&workspace_owner)
                .ok()
                .is_some_and(|roster| {
                    roster
                        .iter()
                        .any(|agent| changed.contains(agent.conversation_id()))
                })
    }

    fn workspace_snapshot_base(&self, workspace: &Workspace) -> Value {
        let owner = self.workspace_snapshot_conversation_owner(workspace);
        let mut value = workspace_json(workspace);
        self.add_scope_compaction(
            &mut value,
            &crate::compaction::CompactionScope::Workspace(workspace.id.clone()),
        );
        self.snapshot_workspace_review(&workspace.id, &mut value);
        let summary = owner
            .as_deref()
            .map(|owner| self.session_summary(owner))
            .unwrap_or_default();
        value["session_started_ms"] = json!(summary.session_started_ms);
        value["last_activity_ms"] = json!(summary.last_activity_ms);
        value["conversations"] = json!(owner
            .as_deref()
            .map(|owner| self.conversation_activity_rows(owner))
            .unwrap_or_default());
        let run = owner
            .as_ref()
            .and_then(|owner| {
                self.runs.get(owner).map(|active| {
                    self.run_view(owner, active, ThreadDetail::Digest, DigestScope::Detail)
                })
            })
            .unwrap_or(Value::Null);
        value["entity_id"] = json!(owner);
        value["run_id"] = json!(owner);
        value["agents"] = run.get("agents").cloned().unwrap_or_else(|| json!([]));
        value["thread"] = run.get("thread").cloned().unwrap_or(Value::Null);
        value["run"] = run;
        value
    }
}

fn depends_on_owner(row: &Value, owner: Option<&str>, changed: &BTreeSet<String>) -> bool {
    let Some(owner) = owner else {
        return true;
    };
    row["entity_id"].as_str() == Some(owner)
        || row["thread"]["agent"]["id"]
            .as_str()
            .is_some_and(|id| changed.contains(id))
        || row["agents"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|agent| agent["conversation_id"].as_str())
            .any(|id| changed.contains(id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_detail_refresh_includes_canonical_aliases_and_keeps_unrelated_rows() {
        let changed = BTreeSet::from(["canonical".to_string()]);
        let direct = json!({"entity_id":"owner", "agents":[]});
        let alias =
            json!({"entity_id":"other", "thread":{"agent":{"id":"canonical"}}, "agents":[]});
        let secondary_alias =
            json!({"entity_id":"other", "agents":[{"conversation_id":"canonical"}]});
        let unrelated =
            json!({"entity_id":"unrelated", "agents":[{"conversation_id":"different"}]});
        assert!(depends_on_owner(&direct, Some("owner"), &changed));
        assert!(depends_on_owner(&alias, Some("owner"), &changed));
        assert!(depends_on_owner(&secondary_alias, Some("owner"), &changed));
        assert!(!depends_on_owner(&unrelated, Some("owner"), &changed));
        assert!(depends_on_owner(&unrelated, None, &changed));
    }
}

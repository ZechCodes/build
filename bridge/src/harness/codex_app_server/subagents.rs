use std::collections::VecDeque;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

use super::protocol::{
    ItemLifecycle, ServerNotification, ThreadMetadataNotification, ThreadReadResult,
};
use crate::harness::surfaces::{AgentSurfaces, SurfaceAgent};

const MAX_SUBAGENTS: usize = 128;
const LABEL_LIMIT: usize = 160;
const MESSAGE_LIMIT: usize = 512;
const ID_LIMIT: usize = 256;
const SEEN_EVENT_LIMIT: usize = 256;

/// Projects parent-thread collaboration items onto Build's shared agent surface.
/// The session also admits a narrow, ancestry-checked subset of child thread
/// metadata; child turn/item lifecycle traffic remains filtered out.
#[derive(Debug, Default)]
pub struct CodexSubagents {
    agents: Vec<TrackedAgent>,
    seen_events: VecDeque<(ItemLifecycle, String, String, &'static str)>,
    hydration_in_flight: Option<(String, u64, u64)>,
}

#[derive(Debug, Default)]
struct TrackedAgent {
    surface: SurfaceAgent,
    configured_model: bool,
    configured_effort: bool,
    model_revision: u64,
    effort_revision: u64,
    needs_hydration: bool,
}

impl CodexSubagents {
    /// Returns newly discovered child ids whose authoritative thread summary
    /// has not yet been requested from app-server.
    pub fn hydration_candidate(&mut self) -> Option<String> {
        if self.hydration_in_flight.is_some() {
            return None;
        }
        let tracked = self
            .agents
            .iter_mut()
            .find(|tracked| tracked.needs_hydration)?;
        tracked.needs_hydration = false;
        let id = tracked.surface.id.clone();
        self.hydration_in_flight =
            Some((id.clone(), tracked.model_revision, tracked.effort_revision));
        Some(id)
    }

    pub fn hydration_finished(&mut self, thread_id: &str, retry: bool) {
        if self
            .hydration_in_flight
            .as_ref()
            .map(|flight| flight.0.as_str())
            != Some(thread_id)
        {
            return;
        }
        self.hydration_in_flight = None;
        if let Some(tracked) = self
            .agents
            .iter_mut()
            .find(|tracked| tracked.surface.id == thread_id)
        {
            tracked.needs_hydration |= retry;
        }
    }

    pub fn apply_thread_read(&mut self, result: &ThreadReadResult, expected_parent: &str) -> bool {
        let thread = &result.thread;
        if thread.parent_thread_id.as_deref() != Some(expected_parent) {
            return false;
        }
        let preview = bounded_text(thread.preview.as_deref(), MESSAGE_LIMIT);
        let label = bounded_text(
            thread
                .agent_role
                .as_deref()
                .or(thread.agent_nickname.as_deref())
                .or(thread.name.as_deref()),
            LABEL_LIMIT,
        );
        let model = thread
            .model
            .as_ref()
            .and_then(|value| bounded_text(value.as_deref(), LABEL_LIMIT));
        let effort = thread
            .reasoning_effort
            .as_ref()
            .and_then(|value| bounded_text(value.as_deref(), LABEL_LIMIT));
        let Some((in_flight_id, model_revision, effort_revision)) =
            self.hydration_in_flight.as_ref()
        else {
            return false;
        };
        if in_flight_id != &thread.id {
            return false;
        }
        let (model_revision, effort_revision) = (*model_revision, *effort_revision);
        self.update_existing(&thread.id, |tracked| {
            let before = tracked.surface.clone();
            let model_fresh = tracked.model_revision == model_revision;
            let effort_fresh = tracked.effort_revision == effort_revision;
            let model = model_fresh.then_some(model.as_deref()).flatten();
            let effort = effort_fresh.then_some(effort.as_deref()).flatten();
            apply_configured_metadata(
                tracked,
                false,
                preview.as_deref(),
                label.as_deref(),
                model,
                effort,
            );
            if model_fresh && thread.model == Some(None) {
                tracked.surface.model = None;
                tracked.configured_model = true;
            }
            if effort_fresh && thread.reasoning_effort == Some(None) {
                tracked.surface.reasoning_effort = None;
                tracked.configured_effort = true;
            }
            tracked.surface != before
        })
    }

    pub fn apply(&mut self, notification: &ServerNotification) -> bool {
        let ServerNotification::Item(item) = notification else {
            return false;
        };
        let Some(event_key) = event_key(item) else {
            return false;
        };
        if self.seen_events.contains(&event_key) {
            return false;
        }
        if self.seen_events.len() == SEEN_EVENT_LIMIT {
            self.seen_events.pop_front();
        }
        self.seen_events.push_back(event_key);
        match item.item["type"].as_str() {
            Some("collabAgentToolCall") => {
                self.apply_collaboration(&item.item, item.lifecycle == ItemLifecycle::Started)
            }
            Some("subAgentActivity") => self.apply_activity(&item.item),
            _ => false,
        }
    }

    pub fn snapshot(&self) -> Option<AgentSurfaces> {
        (!self.agents.is_empty()).then(|| AgentSurfaces {
            subagents: self
                .agents
                .iter()
                .map(|agent| agent.surface.clone())
                .collect(),
            ..AgentSurfaces::default()
        })
    }

    /// Applies only descriptive/configuration metadata for a child thread.
    /// Lifecycle notifications stay isolated by the session.
    pub fn apply_thread_metadata(
        &mut self,
        notification: &ThreadMetadataNotification,
        expected_parent: &str,
    ) -> bool {
        match notification {
            ThreadMetadataNotification::Started {
                thread_id,
                parent_thread_id,
                preview,
                label,
                model,
                reasoning_effort,
            } if parent_thread_id.as_deref() == Some(expected_parent) => {
                let preview = bounded_text(preview.as_deref(), MESSAGE_LIMIT);
                let label = bounded_text(label.as_deref(), LABEL_LIMIT);
                let model = bounded_text(model.as_deref(), LABEL_LIMIT);
                let effort = bounded_text(reasoning_effort.as_deref(), LABEL_LIMIT);
                self.upsert(thread_id, |tracked, created| {
                    let changed = apply_configured_metadata(
                        tracked,
                        created,
                        preview.as_deref(),
                        label.as_deref(),
                        model.as_deref(),
                        effort.as_deref(),
                    );
                    tracked.model_revision += u64::from(model.is_some());
                    tracked.effort_revision += u64::from(effort.is_some());
                    changed
                })
            }
            ThreadMetadataNotification::SettingsUpdated {
                thread_id,
                model,
                reasoning_effort,
            } => {
                let model = bounded_text(model.as_deref(), LABEL_LIMIT);
                let effort = reasoning_effort
                    .as_ref()
                    .map(|effort| bounded_text(effort.as_deref(), LABEL_LIMIT));
                self.update_existing(thread_id, |tracked| {
                    let has_model = model.is_some();
                    let has_effort = effort.is_some();
                    let changed = apply_settings_metadata(tracked, model.as_deref(), effort);
                    tracked.model_revision += u64::from(has_model);
                    tracked.effort_revision += u64::from(has_effort);
                    changed
                })
            }
            ThreadMetadataNotification::Started { .. } => false,
        }
    }

    pub fn settle_running(&mut self) -> bool {
        let mut changed = false;
        for tracked in &mut self.agents {
            let agent = &mut tracked.surface;
            if matches!(agent.state.as_deref(), Some("queued" | "running")) {
                agent.state = Some("failed".to_string());
                agent.error = Some("Parent Codex session ended".to_string());
                retain_duration(agent);
                changed = true;
            }
        }
        changed
    }

    fn apply_collaboration(&mut self, item: &Value, lifecycle_started: bool) -> bool {
        let call_id = bounded_id(&item["id"]);
        let tool = item["tool"].as_str();
        let allows_restart =
            lifecycle_started && matches!(tool, Some("followupTask" | "resumeAgent" | "sendInput"));
        let prompt = optional_text(item, "prompt", LABEL_LIMIT);
        let model = requested_model(item);
        let effort = optional_text(item, "reasoningEffort", LABEL_LIMIT);
        // A completed collaboration *tool* does not imply that its target agent
        // completed. Terminal child state comes only from agentsStates/activity.
        let call_state = (item["status"].as_str() == Some("inProgress")).then_some("running");
        let states = item["agentsStates"].as_object();
        let Some(receivers) = item["receiverThreadIds"].as_array() else {
            return false;
        };

        let mut changed = false;
        for receiver in receivers.iter().filter_map(Value::as_str) {
            let observed = states.and_then(|states| states.get(receiver));
            let state = observed
                .and_then(|state| state["status"].as_str())
                .and_then(|status| normalized_state(Some(status)))
                .or(call_state);
            let message = observed.and_then(|state| optional_text(state, "message", MESSAGE_LIMIT));
            changed |= self.upsert(receiver, |tracked, created| {
                tracked.needs_hydration = true;
                let accepts_requested_model = !tracked.configured_model;
                let accepts_requested_effort = !tracked.configured_effort;
                let agent = &mut tracked.surface;
                let before = agent.clone();
                if created {
                    agent.label = prompt.clone().unwrap_or_else(|| "Sub-agent".to_string());
                    agent.started_at = Some(unix_millis());
                }
                if agent.label == "Sub-agent" {
                    if let Some(label) = &prompt {
                        agent.label = label.clone();
                    }
                }
                if model.is_some() && accepts_requested_model {
                    agent.model = model.clone();
                }
                if effort.is_some() && accepts_requested_effort {
                    agent.reasoning_effort = effort.clone();
                }
                if prompt.is_some()
                    && matches!(
                        tool,
                        Some("spawnAgent" | "followupTask" | "resumeAgent" | "sendInput")
                    )
                {
                    agent.description = prompt.clone();
                }
                if agent.spawning_call_id.is_none() && tool == Some("spawnAgent") {
                    agent.spawning_call_id = call_id.clone();
                }
                apply_state(agent, state, message.as_deref(), allows_restart);
                *agent != before
            });
        }
        changed
    }

    fn apply_activity(&mut self, item: &Value) -> bool {
        let Some(thread_id) = item["agentThreadId"].as_str() else {
            return false;
        };
        if thread_id.len() > ID_LIMIT {
            return false;
        }
        let label = optional_text(item, "agentPath", LABEL_LIMIT);
        let state = match item["kind"].as_str() {
            Some("started" | "interacted") => Some("running"),
            Some("interrupted") => Some("failed"),
            Some("completed") => Some("done"),
            _ => None,
        };
        self.upsert(thread_id, |tracked, created| {
            tracked.needs_hydration = true;
            let agent = &mut tracked.surface;
            let before = agent.clone();
            if created {
                agent.started_at = Some(unix_millis());
            }
            if let Some(label) = label {
                agent.label = label;
            } else if created {
                agent.label = "Sub-agent".to_string();
            }
            let preserves_failure = item["kind"].as_str() == Some("completed")
                && agent.state.as_deref() == Some("failed");
            if !preserves_failure {
                apply_state(agent, state, None, false);
            }
            *agent != before
        })
    }

    fn upsert(&mut self, id: &str, update: impl FnOnce(&mut TrackedAgent, bool) -> bool) -> bool {
        if id.is_empty() || id.len() > ID_LIMIT {
            return false;
        }
        if let Some(agent) = self.agents.iter_mut().find(|agent| agent.surface.id == id) {
            return update(agent, false);
        }
        if self.agents.len() == MAX_SUBAGENTS {
            let evicted = self
                .agents
                .iter()
                .position(|agent| matches!(agent.surface.state.as_deref(), Some("done" | "failed")))
                .unwrap_or(0);
            self.agents.remove(evicted);
        }
        let mut agent = TrackedAgent {
            surface: SurfaceAgent {
                id: id.to_string(),
                ..SurfaceAgent::default()
            },
            ..TrackedAgent::default()
        };
        update(&mut agent, true);
        self.agents.push(agent);
        true
    }

    fn update_existing(
        &mut self,
        id: &str,
        update: impl FnOnce(&mut TrackedAgent) -> bool,
    ) -> bool {
        if id.is_empty() || id.len() > ID_LIMIT {
            return false;
        }
        self.agents
            .iter_mut()
            .find(|agent| agent.surface.id == id)
            .is_some_and(update)
    }
}

fn apply_configured_metadata(
    tracked: &mut TrackedAgent,
    created: bool,
    description: Option<&str>,
    label: Option<&str>,
    model: Option<&str>,
    effort: Option<&str>,
) -> bool {
    let before = tracked.surface.clone();
    if created {
        tracked.surface.label = label.unwrap_or("Sub-agent").to_string();
        tracked.surface.started_at = Some(unix_millis());
    } else if tracked.surface.label == "Sub-agent" {
        if let Some(label) = label {
            tracked.surface.label = label.to_string();
        }
    }
    if let Some(description) = description {
        tracked.surface.description = Some(description.to_string());
    }
    if let Some(model) = model {
        tracked.surface.model = Some(model.to_string());
        tracked.configured_model = true;
    }
    if let Some(effort) = effort {
        tracked.surface.reasoning_effort = Some(effort.to_string());
        tracked.configured_effort = true;
    }
    tracked.surface != before
}

fn apply_settings_metadata(
    tracked: &mut TrackedAgent,
    model: Option<&str>,
    effort: Option<Option<String>>,
) -> bool {
    let before = tracked.surface.clone();
    if let Some(model) = model {
        tracked.surface.model = Some(model.to_string());
        tracked.configured_model = true;
    }
    if let Some(effort) = effort {
        tracked.surface.reasoning_effort = effort;
        tracked.configured_effort = true;
    }
    tracked.surface != before
}

fn event_key(
    item: &super::protocol::ItemNotification,
) -> Option<(ItemLifecycle, String, String, &'static str)> {
    let id = bounded_id(&item.item["id"])?;
    if item.turn_id.is_empty() || item.turn_id.len() > ID_LIMIT {
        return None;
    }
    let detail = match item.item["type"].as_str()? {
        "collabAgentToolCall" => match item.item["status"].as_str()? {
            "inProgress" => "inProgress",
            "completed" => "completed",
            "failed" => "failed",
            "interrupted" => "interrupted",
            _ => return None,
        },
        "subAgentActivity" => match item.item["kind"].as_str()? {
            "started" => "started",
            "interacted" => "interacted",
            "interrupted" => "interrupted",
            "completed" => "completed",
            _ => return None,
        },
        _ => return None,
    };
    Some((item.lifecycle, item.turn_id.clone(), id, detail))
}

fn bounded_id(value: &Value) -> Option<String> {
    let id = value.as_str()?;
    (!id.is_empty() && id.len() <= ID_LIMIT).then(|| id.to_string())
}

fn apply_state(
    agent: &mut SurfaceAgent,
    next: Option<&str>,
    message: Option<&str>,
    allows_restart: bool,
) {
    let terminal = matches!(agent.state.as_deref(), Some("done" | "failed"));
    if !(terminal && !allows_restart && matches!(next, Some("queued" | "running"))) {
        if let Some(next) = next {
            if terminal && allows_restart && matches!(next, "queued" | "running") {
                agent.started_at = Some(unix_millis());
                agent.duration_ms = None;
                agent.result = None;
                agent.error = None;
            }
            agent.state = Some(next.to_string());
            if matches!(next, "done" | "failed") {
                retain_duration(agent);
            }
        }
    }
    match agent.state.as_deref() {
        Some("done") => {
            agent.error = None;
            if let Some(message) = message {
                agent.result = Some(message.to_string());
            }
        }
        Some("failed") => {
            agent.result = None;
            if let Some(message) = message {
                agent.error = Some(message.to_string());
            }
        }
        _ => {}
    }
}

fn retain_duration(agent: &mut SurfaceAgent) {
    if agent.duration_ms.is_none() {
        agent.duration_ms = agent
            .started_at
            .map(|started_at| unix_millis().saturating_sub(started_at));
    }
}

fn normalized_state(status: Option<&str>) -> Option<&'static str> {
    match status? {
        "pendingInit" => Some("queued"),
        "running" | "inProgress" => Some("running"),
        "completed" | "shutdown" => Some("done"),
        "interrupted" | "errored" | "failed" | "notFound" => Some("failed"),
        _ => None,
    }
}

fn requested_model(item: &Value) -> Option<String> {
    optional_text(item, "model", LABEL_LIMIT)
}

fn optional_text(value: &Value, field: &str, limit: usize) -> Option<String> {
    bounded_text(value[field].as_str(), limit)
}

fn bounded_text(value: Option<&str>, limit: usize) -> Option<String> {
    let text = value?.trim();
    if text.is_empty() {
        return None;
    }
    let mut bounded: String = text.chars().take(limit).collect();
    if text.chars().count() > limit {
        bounded.push('…');
    }
    Some(bounded)
}

fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::harness::codex_app_server::protocol::{ItemLifecycle, ItemNotification};

    fn item(value: Value) -> ServerNotification {
        item_at(ItemLifecycle::Completed, value)
    }

    fn item_at(lifecycle: ItemLifecycle, value: Value) -> ServerNotification {
        ServerNotification::Item(ItemNotification {
            lifecycle,
            thread_id: "parent".to_string(),
            turn_id: "turn".to_string(),
            item: value,
        })
    }

    fn started_thread(
        parent: &str,
        model: Option<&str>,
        effort: Option<&str>,
    ) -> ThreadMetadataNotification {
        ThreadMetadataNotification::Started {
            thread_id: "child".to_string(),
            parent_thread_id: Some(parent.to_string()),
            preview: Some("Inspect the parser".to_string()),
            label: Some("reviewer".to_string()),
            model: model.map(str::to_string),
            reasoning_effort: effort.map(str::to_string),
        }
    }

    #[test]
    fn direct_child_thread_metadata_fills_spawn_gaps_and_outranks_requests() {
        let mut held = CodexSubagents::default();
        held.apply(&item(json!({
            "id":"spawn", "type":"collabAgentToolCall", "tool":"spawnAgent",
            "status":"inProgress", "model":"gpt-5.6-luna", "reasoningEffort":"low",
            "receiverThreadIds":["child"], "agentsStates":{}
        })));

        assert!(!held.apply_thread_metadata(
            &started_thread("unrelated-parent", Some("gpt-6-astra"), Some("high")),
            "parent"
        ));
        assert!(held.apply_thread_metadata(
            &started_thread("parent", Some("gpt-6-astra"), Some("high")),
            "parent"
        ));
        held.apply(&item(json!({
            "id":"repeat", "type":"collabAgentToolCall", "tool":"spawnAgent",
            "status":"completed", "model":"gpt-5.6-sol", "reasoningEffort":"medium",
            "receiverThreadIds":["child"], "agentsStates":{}
        })));

        let snapshot = held.snapshot().unwrap();
        let agent = &snapshot.subagents[0];
        assert_eq!(agent.description.as_deref(), Some("Inspect the parser"));
        assert_eq!(agent.model.as_deref(), Some("gpt-6-astra"));
        assert_eq!(agent.reasoning_effort.as_deref(), Some("high"));
    }

    #[test]
    fn activity_requests_one_authoritative_read_and_applies_only_direct_child() {
        let mut held = CodexSubagents::default();
        let activity = item(json!({
            "id":"activity", "type":"subAgentActivity", "agentPath":"/root/reviewer",
            "agentThreadId":"child", "kind":"started"
        }));
        held.apply(&activity);
        assert_eq!(held.hydration_candidate().as_deref(), Some("child"));
        assert!(held.hydration_candidate().is_none());

        let result = ThreadReadResult {
            thread: super::super::protocol::ThreadSummary {
                id: "child".to_string(),
                parent_thread_id: Some("other".to_string()),
                preview: Some("".to_string()),
                agent_role: None,
                agent_nickname: Some("Schrodinger".to_string()),
                name: None,
                model: Some(Some("gpt-5.6-sol".to_string())),
                reasoning_effort: Some(Some("low".to_string())),
            },
        };
        assert!(!held.apply_thread_read(&result, "parent"));
        let mut direct = result;
        direct.thread.parent_thread_id = Some("parent".to_string());
        assert!(held.apply_thread_read(&direct, "parent"));
        held.hydration_finished("child", false);

        let agent = &held.snapshot().unwrap().subagents[0];
        assert_eq!(agent.label, "/root/reviewer");
        assert_eq!(agent.model.as_deref(), Some("gpt-5.6-sol"));
        assert_eq!(agent.reasoning_effort.as_deref(), Some("low"));
    }

    #[test]
    fn newer_settings_outrank_an_old_read_and_a_fresh_read_can_clear_effort() {
        let mut held = CodexSubagents::default();
        held.apply(&item(json!({
            "id":"activity-1", "type":"subAgentActivity", "agentPath":"worker",
            "agentThreadId":"child", "kind":"started"
        })));
        assert_eq!(held.hydration_candidate().as_deref(), Some("child"));
        held.apply_thread_metadata(
            &ThreadMetadataNotification::SettingsUpdated {
                thread_id: "child".to_string(),
                model: Some("new-model".to_string()),
                reasoning_effort: Some(Some("high".to_string())),
            },
            "parent",
        );
        let mut read = ThreadReadResult {
            thread: super::super::protocol::ThreadSummary {
                id: "child".to_string(),
                parent_thread_id: Some("parent".to_string()),
                preview: None,
                agent_role: None,
                agent_nickname: None,
                name: None,
                model: Some(Some("old-model".to_string())),
                reasoning_effort: Some(Some("low".to_string())),
            },
        };
        assert!(!held.apply_thread_read(&read, "parent"));
        held.hydration_finished("child", false);

        held.apply(&item(json!({
            "id":"activity-2", "type":"subAgentActivity", "agentPath":"worker",
            "agentThreadId":"child", "kind":"interacted"
        })));
        assert_eq!(held.hydration_candidate().as_deref(), Some("child"));
        read.thread.model = Some(Some("newer-model".to_string()));
        read.thread.reasoning_effort = Some(None);
        assert!(held.apply_thread_read(&read, "parent"));
        let agent = &held.snapshot().unwrap().subagents[0];
        assert_eq!(agent.model.as_deref(), Some("newer-model"));
        assert!(agent.reasoning_effort.is_none());
    }

    #[test]
    fn settings_update_only_known_children_and_explicit_null_clears_effort() {
        let mut held = CodexSubagents::default();
        let settings = |thread_id: &str, effort| ThreadMetadataNotification::SettingsUpdated {
            thread_id: thread_id.to_string(),
            model: Some("gpt-5.6-sol".to_string()),
            reasoning_effort: effort,
        };
        assert!(!held.apply_thread_metadata(
            &settings("unknown", Some(Some("high".to_string()))),
            "parent"
        ));

        held.apply(&item(json!({
            "id":"activity", "type":"subAgentActivity", "agentPath":"/root/parser",
            "agentThreadId":"child", "kind":"started"
        })));
        held.apply(&item(json!({
            "id":"spawn", "type":"collabAgentToolCall", "tool":"spawnAgent",
            "status":"completed", "reasoningEffort":"high",
            "receiverThreadIds":["child"], "agentsStates":{}
        })));
        assert!(held.apply_thread_metadata(&settings("child", Some(None)), "parent"));
        held.apply(&item(json!({
            "id":"repeat", "type":"collabAgentToolCall", "tool":"spawnAgent",
            "status":"completed", "reasoningEffort":"ultra",
            "receiverThreadIds":["child"], "agentsStates":{}
        })));

        let agent = &held.snapshot().unwrap().subagents[0];
        assert_eq!(agent.model.as_deref(), Some("gpt-5.6-sol"));
        assert_eq!(agent.reasoning_effort, None);
    }

    #[test]
    fn child_thread_metadata_ids_are_rejected_and_text_is_bounded() {
        let mut held = CodexSubagents::default();
        let mut oversized_id = started_thread("parent", Some("model"), Some("high"));
        if let ThreadMetadataNotification::Started { thread_id, .. } = &mut oversized_id {
            *thread_id = "x".repeat(ID_LIMIT + 1);
        }
        assert!(!held.apply_thread_metadata(&oversized_id, "parent"));
        assert!(held.snapshot().is_none());

        let oversized_text = "x".repeat(MESSAGE_LIMIT + 1);
        let metadata = ThreadMetadataNotification::Started {
            thread_id: "child".to_string(),
            parent_thread_id: Some("parent".to_string()),
            preview: Some(oversized_text.clone()),
            label: Some(oversized_text.clone()),
            model: Some(oversized_text.clone()),
            reasoning_effort: Some(oversized_text),
        };
        assert!(held.apply_thread_metadata(&metadata, "parent"));
        let snapshot = held.snapshot().unwrap();
        let agent = &snapshot.subagents[0];
        assert_eq!(agent.label.chars().count(), LABEL_LIMIT + 1);
        assert_eq!(
            agent.description.as_ref().unwrap().chars().count(),
            MESSAGE_LIMIT + 1
        );
        assert_eq!(
            agent.model.as_ref().unwrap().chars().count(),
            LABEL_LIMIT + 1
        );
        assert_eq!(
            agent.reasoning_effort.as_ref().unwrap().chars().count(),
            LABEL_LIMIT + 1
        );
    }

    #[test]
    fn spawn_metadata_and_activity_form_one_agent() {
        let mut held = CodexSubagents::default();
        assert!(held.apply(&item(json!({
            "id":"call-7", "type":"collabAgentToolCall", "tool":"spawnAgent",
            "status":"inProgress", "prompt":"Inspect the parser", "model":"gpt-5.6-sol",
            "reasoningEffort":"high", "receiverThreadIds":["child-1"],
            "agentsStates":{"child-1":{"status":"pendingInit","message":null}}
        }))));
        assert!(held.apply(&item(json!({
            "id":"activity-1", "type":"subAgentActivity", "agentPath":"/root/parser",
            "agentThreadId":"child-1", "kind":"started"
        }))));

        let agent = &held.snapshot().unwrap().subagents[0];
        assert_eq!(agent.id, "child-1");
        assert_eq!(agent.label, "/root/parser");
        assert_eq!(agent.model.as_deref(), Some("gpt-5.6-sol"));
        assert_eq!(agent.reasoning_effort.as_deref(), Some("high"));
        assert_eq!(agent.description.as_deref(), Some("Inspect the parser"));
        assert_eq!(agent.state.as_deref(), Some("running"));
        assert_eq!(agent.spawning_call_id.as_deref(), Some("call-7"));
    }

    #[test]
    fn terminal_activity_cannot_be_regressed_or_duplicated() {
        let mut held = CodexSubagents::default();
        let completed = item(json!({
            "id":"activity", "type":"subAgentActivity", "agentPath":"worker",
            "agentThreadId":"child", "kind":"completed"
        }));
        assert!(held.apply(&completed));
        assert!(!held.apply(&completed));
        assert!(!held.apply(&item(json!({
            "id":"later", "type":"subAgentActivity", "agentPath":"worker",
            "agentThreadId":"child", "kind":"interacted"
        }))));
        assert_eq!(
            held.snapshot().unwrap().subagents[0].state.as_deref(),
            Some("done")
        );
    }

    #[test]
    fn generic_completion_preserves_an_authoritative_agent_error() {
        let mut held = CodexSubagents::default();
        held.apply(&item(json!({
            "id":"spawn", "type":"collabAgentToolCall", "tool":"spawnAgent",
            "status":"completed", "receiverThreadIds":["child"],
            "agentsStates":{"child":{"status":"errored","message":"review failed"}}
        })));
        held.apply(&item(json!({
            "id":"activity", "type":"subAgentActivity", "agentPath":"worker",
            "agentThreadId":"child", "kind":"completed"
        })));
        let snapshot = held.snapshot().unwrap();
        let agent = &snapshot.subagents[0];
        assert_eq!(agent.state.as_deref(), Some("failed"));
        assert_eq!(agent.error.as_deref(), Some("review failed"));
    }

    #[test]
    fn completion_state_carries_result_and_is_bounded() {
        let mut held = CodexSubagents::default();
        for index in 0..=MAX_SUBAGENTS {
            held.apply(&item(json!({
                "id":format!("call-{index}"), "type":"collabAgentToolCall", "tool":"spawnAgent",
                "status":"completed", "receiverThreadIds":[format!("child-{index}")],
                "agentsStates":{format!("child-{index}"):{"status":"completed","message":"finished"}}
            })));
        }
        let agents = held.snapshot().unwrap().subagents;
        assert_eq!(agents.len(), MAX_SUBAGENTS);
        assert_eq!(agents[0].id, "child-1");
        assert_eq!(agents.last().unwrap().result.as_deref(), Some("finished"));
    }

    #[test]
    fn followup_restarts_a_terminal_agent_and_session_end_settles_it() {
        let mut held = CodexSubagents::default();
        held.apply(&item(json!({
            "id":"done", "type":"subAgentActivity", "agentPath":"worker",
            "agentThreadId":"child", "kind":"completed"
        })));
        assert!(held.apply(&item_at(
            ItemLifecycle::Started,
            json!({
                "id":"followup", "type":"collabAgentToolCall", "tool":"followupTask",
                "status":"inProgress", "receiverThreadIds":["child"],
                "agentsStates":{"child":{"status":"running","message":null}}
            })
        )));
        assert_eq!(
            held.snapshot().unwrap().subagents[0].state.as_deref(),
            Some("running")
        );
        held.apply(&item(json!({
            "id":"finished", "type":"subAgentActivity", "agentPath":"worker",
            "agentThreadId":"child", "kind":"completed"
        })));
        assert!(!held.apply(&item(json!({
            "id":"followup", "type":"collabAgentToolCall", "tool":"followupTask",
            "status":"completed", "receiverThreadIds":["child"],
            "agentsStates":{"child":{"status":"running","message":null}}
        }))));
        assert_eq!(
            held.snapshot().unwrap().subagents[0].state.as_deref(),
            Some("done")
        );
        held.apply(&item_at(
            ItemLifecycle::Started,
            json!({
                "id":"second-followup", "type":"collabAgentToolCall", "tool":"followupTask",
                "status":"inProgress", "receiverThreadIds":["child"],
                "agentsStates":{"child":{"status":"running","message":null}}
            }),
        ));
        assert!(held.settle_running());
        assert_eq!(
            held.snapshot().unwrap().subagents[0].state.as_deref(),
            Some("failed")
        );
    }

    #[test]
    fn tool_completion_does_not_claim_child_completion_or_create_receiverless_ghosts() {
        let mut held = CodexSubagents::default();
        held.apply(&item(json!({
            "id":"spawn", "type":"collabAgentToolCall", "tool":"spawnAgent",
            "status":"completed", "receiverThreadIds":["child"],
            "agentsStates":{"child":{"status":"running","message":null}}
        })));
        assert_eq!(
            held.snapshot().unwrap().subagents[0].state.as_deref(),
            Some("running")
        );

        let mut failed = CodexSubagents::default();
        assert!(!failed.apply(&item(json!({
            "id":"failed", "type":"collabAgentToolCall", "tool":"spawnAgent",
            "status":"failed", "receiverThreadIds":[], "agentsStates":{}
        }))));
        assert!(failed.snapshot().is_none());
    }

    #[test]
    fn hostile_identifiers_and_statuses_create_no_surface() {
        let mut held = CodexSubagents::default();
        for value in [
            json!({"id":"x".repeat(ID_LIMIT + 1),"type":"subAgentActivity","agentPath":"worker","agentThreadId":"child","kind":"started"}),
            json!({"id":"ok","type":"subAgentActivity","agentPath":"worker","agentThreadId":"x".repeat(ID_LIMIT + 1),"kind":"started"}),
            json!({"id":"receiver","type":"collabAgentToolCall","tool":"spawnAgent","status":"inProgress","receiverThreadIds":["x".repeat(ID_LIMIT + 1)],"agentsStates":{}}),
            json!({"id":"ok","type":"collabAgentToolCall","tool":"spawnAgent","status":"future","receiverThreadIds":["child"],"agentsStates":{}}),
        ] {
            assert!(!held.apply(&item(value)));
        }
        assert!(held.snapshot().is_none());
        assert!(held.seen_events.len() <= SEEN_EVENT_LIMIT);
    }

    #[test]
    fn event_identity_includes_turn_and_resume_clears_terminal_metadata() {
        let mut held = CodexSubagents::default();
        let completion = |turn: &str| {
            ServerNotification::Item(ItemNotification {
                lifecycle: ItemLifecycle::Completed,
                thread_id: "parent".to_string(),
                turn_id: turn.to_string(),
                item: json!({"id":"same","type":"subAgentActivity","agentPath":"worker","agentThreadId":"child","kind":"completed"}),
            })
        };
        assert!(held.apply(&completion("turn-1")));
        assert!(!held.apply(&completion("turn-1")));
        held.apply(&item(json!({
            "id":"error", "type":"collabAgentToolCall", "tool":"spawnAgent",
            "status":"failed", "receiverThreadIds":["child"],
            "agentsStates":{"child":{"status":"errored","message":"old failure"}}
        })));

        assert!(held.apply(&item_at(
            ItemLifecycle::Started,
            json!({
                "id":"resume", "type":"collabAgentToolCall", "tool":"resumeAgent",
                "status":"inProgress", "receiverThreadIds":["child"],
                "agentsStates":{"child":{"status":"running","message":null}}
            })
        )));
        let snapshot = held.snapshot().unwrap();
        let agent = &snapshot.subagents[0];
        assert_eq!(agent.state.as_deref(), Some("running"));
        assert!(agent.result.is_none() && agent.error.is_none() && agent.duration_ms.is_none());
        drop(snapshot);
        assert!(held.apply(&completion("turn-2")));
        assert_eq!(
            held.snapshot().unwrap().subagents[0].state.as_deref(),
            Some("done")
        );

        let overlong_turn = ServerNotification::Item(ItemNotification {
            lifecycle: ItemLifecycle::Completed,
            thread_id: "parent".to_string(),
            turn_id: "x".repeat(ID_LIMIT + 1),
            item: json!({"id":"turn","type":"subAgentActivity","agentPath":"worker","agentThreadId":"other","kind":"completed"}),
        });
        assert!(!held.apply(&overlong_turn));
    }
}

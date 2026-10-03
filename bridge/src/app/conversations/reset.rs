use super::{reset_files::ResetFiles, AppState, ConversationAddress};
use crate::agent::{Agent, AgentLifecycle};
use crate::app::{model_choice_from, require_str, DigestScope, TabKey};
use crate::harness::{AgentSession, HarnessConversationCleanup};
use crate::lifecycle::{PendingRow, PendingState, Performed, WorktreeChange, WorktreeMutation};
use crate::models::ModelChoice;
use crate::thread::{SessionLineage, Thread};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

impl AppState {
    /// Replace history, keeping every durable identity and ownership link.
    pub(crate) fn conversation_reset(&mut self, params: &Value) -> Result<Value, String> {
        let entity_id = require_str(params, "entity_id")?;
        let project_id = require_str(params, "project_id")?;
        require_str(params, "agent_id")?;
        require_str(params, "conversation_id")?;
        let expected = require_str(params, "expected_thread_id")?;
        let address = self.resolve_conversation_params(&entity_id, params)?;
        self.validate_reset_project(&address, &project_id)?;
        self.guard_thread_id(&address, Some(&Value::String(expected.clone())))?;
        let addressed = self
            .entity_agents(&entity_id)?
            .by_id(&address.agent_id)
            .expect("resolved agent");
        let choice = reset_choice(params, &addressed.choice)?;
        if let Some(why) = crate::harness::installed::held_refusal(&self.cli_readings, &choice) {
            return Err(why);
        }
        let agents = self.reset_agents(&address, params, choice)?;
        let selected_limit = params.get("max_context_tokens").is_some();
        self.refuse_reset_in_flight(&address, &agents)?;
        let (homes, leaves) = self.reset_file_inputs(&address, &agents)?;
        let native = self.reset_native_inputs(&address, &agents)?;
        let row = PendingRow::on_directory(
            format!("reset:{}", address.conversation_id),
            "Clearing conversation".into(),
            PendingState::Updating,
        );
        let state_root = self.state_root.clone();
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or("conversation.reset: HOME is unavailable")?;
        self.defer_lifecycle_holding(row, move |state| {
            state
                .resetting_conversations
                .insert(address.conversation_id.clone());
            let held = state
                .delivery_queue
                .take_conversation(&address.conversation_id);
            let native = native
                .into_iter()
                .map(|mut input| {
                    let root = state
                        .entity_agent_root(&input.agent.owner_id)
                        .expect("validated agent root");
                    input.retirement = state
                        .retire_tab(&TabKey::agent(&root, &input.agent.id), "conversation_reset");
                    state.retire_agent(&root, &input.agent.id);
                    input
                })
                .collect();
            let work = ResetConversationWork {
                state_root,
                home,
                native,
                homes,
                leaves,
            };
            let settle = move |state: &mut AppState, outcome: Result<ResetArtifacts, String>| {
                state
                    .resetting_conversations
                    .remove(&address.conversation_id);
                let outcome = outcome.and_then(|artifacts| {
                    state.commit_conversation_reset(
                        address.clone(),
                        expected.clone(),
                        agents,
                        selected_limit,
                        artifacts,
                    )
                });
                if outcome.is_err()
                    && state
                        .conversation_at(&address)
                        .is_ok_and(|thread| thread.id == expected)
                {
                    state.delivery_queue.restore_conversation(held);
                }
                outcome
            };
            (work, settle)
        })
    }

    fn commit_conversation_reset(
        &mut self,
        address: ConversationAddress,
        expected: String,
        agents: Vec<Agent>,
        selected_limit: bool,
        mut artifacts: ResetArtifacts,
    ) -> Result<Value, String> {
        self.guard_thread_id(&address, Some(&Value::String(expected.clone())))?;
        for agent in &agents {
            let current = self.resolve_conversation_address(&agent.owner_id, Some(&agent.id))?;
            if current.conversation_id != address.conversation_id
                || self.projects.project_id_of(&current.entity_id)
                    != self.projects.project_id_of(&address.entity_id)
            {
                return Err("stale conversation binding; reset target changed".into());
            }
        }
        let agents = self.rebase_reset_agents(&address, agents, selected_limit)?;
        if artifacts
            .lineages
            .iter()
            .any(|lineage| self.native_history_is_shared(&address, lineage))
        {
            return Err(
                "conversation.reset: native history became shared; try again shortly".into(),
            );
        }
        let attention = self.reset_attention(&agents);
        let summary_rows = self.reset_summary_rows(&agents)?;
        if let Some(store) = self.store.as_ref() {
            let exclusive = store
                .reset_attachment_leaves(&address.conversation_id)
                .map_err(|error| error.to_string())?;
            artifacts.files.retain_only(&exclusive)?;
            store
                .reset_conversation(
                    &address.conversation_id,
                    &agents,
                    &attention,
                    &self.reset_summary_owners(&agents),
                )
                .map_err(|error| format!("conversation.reset store: {error}"))?;
        }
        // Once the database commits, no later failure may restore resumable
        // history. Finalize every guard, retaining an unlink error until the
        // replacement generation has been published.
        let cleanup_result = artifacts.commit();
        self.operation_ledger
            .forget_conversation(&address.conversation_id);
        self.board.attention_mut().replace_entries(attention);
        let mut owners = HashSet::new();
        for agent in agents {
            self.operation_ledger.forget_conversation(&agent.id);
            owners.insert(agent.owner_id.clone());
            self.install_reset_agent(agent)?;
        }
        self.replace_session_summaries(summary_rows);
        let thread_id = self.conversation_at(&address)?.id.clone();
        self.board
            .attention_mut()
            .advance_conversation(thread_id, 0);
        for owner in owners {
            self.note_entity_changed(&owner);
            self.publish_live_roster_for(&owner);
        }
        self.note_board_changed();
        cleanup_result?;
        let agent = self
            .entity_agents(&address.entity_id)?
            .by_id(&address.agent_id)
            .expect("installed agent");
        let thread = self.conversation_at(&address)?;
        let root = self.entity_agent_root(&address.entity_id).ok();
        Ok(json!({
            "entity_id": address.entity_id, "agent_id": agent.id,
            "conversation_id": address.conversation_id,
            "previous_thread_id": expected, "thread_id": thread.id,
            "thread_generation_revision": thread.generation_revision,
            "agent": self.agent_digest(&address.entity_id, agent, root.as_deref(), DigestScope::Detail),
            "thread": thread.wire_value_page(None, 100),
        }))
    }

    fn rebase_reset_agents(
        &self,
        address: &ConversationAddress,
        agents: Vec<Agent>,
        selected_limit: bool,
    ) -> Result<Vec<Agent>, String> {
        agents
            .into_iter()
            .map(|desired| {
                let mut current = self
                    .entity_agents(&desired.owner_id)?
                    .by_id(&desired.id)
                    .ok_or("reset agent disappeared")?
                    .clone();
                reset_process_fields(&mut current);
                current.thread = desired.thread;
                if current.id == address.agent_id {
                    current.choose(desired.choice);
                    if selected_limit {
                        current.max_context_tokens = desired.max_context_tokens;
                    }
                } else {
                    current.choose(current.choice.clone());
                }
                Ok(current)
            })
            .collect()
    }

    fn reset_summary_owners(&self, agents: &[Agent]) -> HashSet<String> {
        agents
            .iter()
            .filter(|agent| {
                self.entity_agents(&agent.owner_id)
                    .is_ok_and(|roster| roster.is_primary(&agent.id))
            })
            .map(|agent| agent.owner_id.clone())
            .collect()
    }

    fn reset_summary_rows(
        &self,
        agents: &[Agent],
    ) -> Result<Vec<crate::store::SessionMessageTime>, String> {
        let Some(store) = self.store.as_ref() else {
            return Ok(Vec::new());
        };
        let mut rows = store
            .session_message_times()
            .map_err(|error| error.to_string())?;
        rows.retain(|(_, _, id, _, _)| !agents.iter().any(|agent| agent.id == *id));
        Ok(rows)
    }

    fn validate_reset_project(
        &self,
        address: &ConversationAddress,
        project_id: &str,
    ) -> Result<(), String> {
        for owner in [&address.entity_id, &address.conversation_entity_id] {
            if self.projects.project_id_of(owner) != Some(project_id) {
                return Err("conversation.reset: conversation does not belong to project".into());
            }
        }
        Ok(())
    }

    fn reset_agents(
        &self,
        address: &ConversationAddress,
        params: &Value,
        choice: ModelChoice,
    ) -> Result<Vec<Agent>, String> {
        let mut agents = Vec::new();
        for owner in self.runs.keys().chain(self.plans.keys()) {
            for old in self
                .entity_agents(owner)?
                .iter()
                .filter(|agent| agent.conversation_id() == address.conversation_id)
            {
                let mut agent = old.clone();
                reset_process_fields(&mut agent);
                agent.thread = Thread::for_agent(&agent.id);
                if agent.id == address.conversation_id {
                    agent.thread.id = format!("thread:{}:{}", agent.id, uuid::Uuid::new_v4());
                    agent.thread.generation_revision =
                        old.thread.generation_revision.saturating_add(1);
                }
                if agent.id == address.agent_id {
                    agent.choose(choice.clone());
                    if let Some(limit) = params.get("max_context_tokens") {
                        agent.max_context_tokens = match limit { Value::Null => None, _ => Some(limit.as_u64().ok_or("invalid max_context_tokens: expected a nonnegative integer or null")?) };
                    }
                } else {
                    agent.choose(agent.choice.clone());
                }
                agents.push(agent);
            }
        }
        Ok(agents)
    }

    fn refuse_reset_in_flight(
        &self,
        address: &ConversationAddress,
        agents: &[Agent],
    ) -> Result<(), String> {
        for agent in agents {
            if self.projects.project_id_of(&agent.owner_id)
                != self.projects.project_id_of(&address.entity_id)
            {
                return Err("conversation.reset: conversation does not belong to project".into());
            }
            let root = self.entity_agent_root(&agent.owner_id)?;
            let key = TabKey::agent(&root, &agent.id);
            if self.session_registry.claim_is_held(&key)
                || self.delivery_queue.agent_in_flight(&key)
            {
                return Err(
                    "conversation.reset: session delivery is in progress; try again shortly".into(),
                );
            }
        }
        Ok(())
    }

    fn reset_attention(&self, agents: &[Agent]) -> HashMap<String, crate::attention::Attention> {
        let mut entries: HashMap<_, _> = self
            .board
            .attention()
            .persistence()
            .into_entries()
            .into_iter()
            .collect();
        for agent in agents {
            if let Some(attention) = entries.get_mut(&agent.owner_id) {
                attention.agent_read_sequences.remove(&agent.id);
                attention.agent_dismissed_through.remove(&agent.id);
                if self
                    .entity_agents(&agent.owner_id)
                    .is_ok_and(|roster| roster.is_primary(&agent.id))
                {
                    attention.last_read_sequence = 0;
                    attention.dismissed_through = 0;
                }
            }
        }
        entries
    }

    fn reset_file_inputs(
        &self,
        address: &ConversationAddress,
        agents: &[Agent],
    ) -> Result<(Vec<PathBuf>, HashSet<String>), String> {
        let leaves = match self.store.as_ref() {
            Some(store) => store
                .reset_attachment_leaves(&address.conversation_id)
                .map_err(|error| error.to_string())?,
            None => HashSet::new(),
        };
        let mut homes = HashSet::new();
        for agent in agents {
            homes.extend(self.attachment_homes(&agent.owner_id)?.in_read_order());
        }
        Ok((homes.into_iter().collect(), leaves))
    }

    fn reset_native_inputs(
        &self,
        address: &ConversationAddress,
        agents: &[Agent],
    ) -> Result<Vec<ResetNativeAgent>, String> {
        let thread = self.conversation_at(address)?;
        let mut native = Vec::new();
        for agent in agents {
            let old = self
                .entity_agents(&agent.owner_id)?
                .by_id(&agent.id)
                .expect("resolved alias")
                .clone();
            let root = self.entity_agent_root(&agent.owner_id)?;
            let snapshot = self
                .session_registry
                .agent_snapshot(&TabKey::agent(&root, &agent.id));
            let mut lineages: Vec<_> = thread
                .sessions
                .iter()
                .filter(|lineage| lineage.agent_id == agent.id)
                .cloned()
                .collect();
            let captured_native_id = snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.session.session_id());
            let session_lineage_id = snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.instance.as_ref())
                .map(|instance| instance.id.clone());
            if let Some(snapshot) = &snapshot {
                if let Some(named) = &captured_native_id {
                    let lineage = snapshot.instance.as_ref().and_then(|instance| {
                        lineages
                            .iter_mut()
                            .find(|lineage| lineage.id == instance.id)
                    });
                    let lineage = lineage.ok_or(
                        "conversation.reset: live native history has no exact session lineage",
                    )?;
                    lineage.resume_session_id = Some(named.clone());
                }
            }
            lineages.retain(|lineage| !self.native_history_is_shared(address, lineage));
            let session = snapshot.map(|snapshot| snapshot.session);
            native.push(ResetNativeAgent {
                agent: old,
                lineages,
                session,
                retirement: None,
                captured_native_id,
                session_lineage_id,
            });
        }
        Ok(native)
    }

    pub(in crate::app) fn native_history_is_shared(
        &self,
        address: &ConversationAddress,
        native: &SessionLineage,
    ) -> bool {
        let Some(named) = native.resume_session_id.as_deref() else {
            return false;
        };
        let Some(provider) = crate::models::AgentProvider::from_wire(&native.provider) else {
            return true;
        };
        let namespace = crate::harness::harness_for(provider).native_history_namespace();
        self.runs.keys().chain(self.plans.keys()).any(|owner| {
            self.entity_agents(owner).is_ok_and(|roster| {
                roster
                    .iter()
                    .filter(|agent| agent.conversation_id() != address.conversation_id)
                    .any(|agent| self.agent_shares_native(owner, agent, native, named, namespace))
            })
        })
    }

    fn agent_shares_native(
        &self,
        owner: &str,
        agent: &Agent,
        native: &SessionLineage,
        named: &str,
        namespace: &str,
    ) -> bool {
        let current = crate::harness::harness_for(agent.choice.provider);
        if agent.resume_session_id.as_deref() == Some(named)
            && current.native_history_namespace() == namespace
        {
            return true;
        }
        let live = self.entity_agent_root(owner).ok().and_then(|root| {
            self.session_registry
                .agent_snapshot(&TabKey::agent(&root, &agent.id))
        });
        // A live id without durable attribution is protected conservatively;
        // it cannot establish exclusive ownership of a provider artifact.
        if live.is_some_and(|live| live.session.session_id().as_deref() == Some(named)) {
            return true;
        }
        self.agent_conversation(owner, Some(&agent.id))
            .is_ok_and(|thread| {
                thread
                    .sessions
                    .iter()
                    .any(|session| native_lineages_share(native, session))
            })
    }

    fn install_reset_agent(&mut self, agent: Agent) -> Result<(), String> {
        let root = self.entity_agent_root(&agent.owner_id)?;
        self.retire_agent(&root, &agent.id);
        self.compactions.forget(&agent.owner_id, &agent.id);
        self.usage_limits.forget_agent(&agent.owner_id, &agent.id);
        self.dispatched_task.remove(&agent.id);
        self.reminded_holdings.remove(&agent.id);
        let owner = agent.owner_id.clone();
        let id = agent.id.clone();
        let roster = if let Some(run) = self.runs.get_mut(&owner) {
            if run.agents.is_primary(&id) {
                run.last_summary = None;
                run.last_error = None;
            }
            &mut run.agents
        } else {
            let plan = self
                .plans
                .get_mut(&owner)
                .expect("validated conversation owner");
            if plan.agents.is_primary(&id) {
                plan.last_summary = None;
                plan.last_error = None;
            }
            &mut plan.agents
        };
        *roster.by_id_mut(&id).expect("validated agent") = agent;
        Ok(())
    }
}

fn reset_process_fields(agent: &mut Agent) {
    agent.state = AgentLifecycle::Idle;
    agent.resume_session_id = None;
    agent.topic = None;
    agent.active_model = None;
    agent.active_effort = None;
    agent.working_since = None;
    agent.start_error = None;
    agent.last_context_tokens = None;
    agent.last_context_at = None;
    agent.session_cache_read_tokens = None;
}

fn reset_choice(params: &Value, old: &ModelChoice) -> Result<ModelChoice, String> {
    let parsed = model_choice_from(params, old.provider)?;
    let mut choice = old.clone();
    if parsed.provider != old.provider {
        choice = parsed.clone();
    }
    if params.get("model").is_some() {
        choice.model = parsed.model;
    }
    if params.get("effort").is_some() {
        choice.effort = parsed.effort;
    }
    choice.provider = parsed.provider;
    choice.validate()?;
    Ok(choice)
}

struct ResetNativeAgent {
    agent: Agent,
    lineages: Vec<SessionLineage>,
    session: Option<Arc<dyn AgentSession>>,
    retirement: Option<crate::reaper::Retirement>,
    captured_native_id: Option<String>,
    session_lineage_id: Option<String>,
}

impl ResetNativeAgent {
    fn refresh_native_id(&mut self) -> Result<(), String> {
        let named = self
            .session
            .as_ref()
            .and_then(|session| session.session_id());
        if named.is_none() || named == self.captured_native_id {
            return Ok(());
        }
        let lineage = self
            .lineages
            .iter_mut()
            .find(|lineage| Some(&lineage.id) == self.session_lineage_id.as_ref())
            .ok_or("conversation.reset: stopped native history has no exact session lineage")?;
        lineage.resume_session_id = named;
        Ok(())
    }
}

struct ResetConversationWork {
    home: PathBuf,
    state_root: PathBuf,
    native: Vec<ResetNativeAgent>,
    homes: Vec<PathBuf>,
    leaves: HashSet<String>,
}

struct ResetArtifacts {
    files: ResetFiles,
    native: Vec<HarnessConversationCleanup>,
    lineages: Vec<SessionLineage>,
}

impl ResetArtifacts {
    fn commit(self) -> Result<(), String> {
        let mut errors = Vec::new();
        if let Err(error) = self.files.commit() {
            errors.push(error);
        }
        for native in self.native {
            if let Err(error) = native.commit() {
                errors.push(error.to_string());
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }
}

impl WorktreeMutation for ResetConversationWork {
    type Output = ResetArtifacts;

    fn perform(self) -> Result<Performed<Self::Output>, String> {
        let mut cleanups = Vec::new();
        let mut lineages = Vec::new();
        for mut input in self.native {
            if input
                .retirement
                .take()
                .is_some_and(|retired| !retired.wait(Duration::from_secs(10)))
            {
                return Err(
                    "conversation.reset: process is still stopping; try again shortly".into(),
                );
            }
            input.refresh_native_id()?;
            let mut cleanup = HarnessConversationCleanup::prepare(
                &self.home,
                &self.state_root,
                &input.agent.id,
                input.agent.choice.provider,
                &input.lineages,
            )
            .map_err(|error| error.to_string())?;
            if let Some(session) = input
                .session
                .filter(|session| !session.conversation_artifacts().is_empty())
            {
                cleanup
                    .stage_session(&self.state_root, session.as_ref())
                    .map_err(|error| error.to_string())?;
            }
            lineages.extend(input.lineages);
            cleanups.push(cleanup);
        }
        let files = ResetFiles::prepare(&self.homes, &self.leaves)?;
        Ok(Performed {
            change: WorktreeChange::nothing(),
            output: ResetArtifacts {
                files,
                native: cleanups,
                lineages,
            },
        })
    }
}

fn native_lineages_share(one: &SessionLineage, other: &SessionLineage) -> bool {
    if one.resume_session_id != other.resume_session_id {
        return false;
    }
    let Some(one_provider) = crate::models::AgentProvider::from_wire(&one.provider) else {
        return true;
    };
    let Some(other_provider) = crate::models::AgentProvider::from_wire(&other.provider) else {
        return true;
    };
    let one_harness = crate::harness::harness_for(one_provider);
    let other_harness = crate::harness::harness_for(other_provider);
    if one_harness.native_history_namespace() != other_harness.native_history_namespace() {
        return false;
    }
    let (Some(one_cwd), Some(other_cwd)) = (one.stood_in(), other.stood_in()) else {
        return true;
    };
    one_harness.native_history_cwd(std::path::Path::new(one_cwd))
        == other_harness.native_history_cwd(std::path::Path::new(other_cwd))
}

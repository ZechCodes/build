//! Who an agent is, as a message names it.
//!
//! An agent id is enough to address an agent and not enough to show one: a
//! client drawing an inbound message wants the workspace or project it came
//! from and what that conversation is about, and neither is derivable from the
//! id. So the daemon stamps both onto the identity when it posts the message —
//! at post time, from what it knew then, because a workspace can be renamed and
//! a topic changes with the work.

use std::path::PathBuf;

use crate::app::projects::scratch_dir;
use crate::app::workspaces::same_path;
use crate::app::AppState;
use crate::thread::{AgentIdentity, AgentOwnerKind, AgentOwnerRef};

impl AppState {
    /// One agent as a message wears it: its id, the workspace or project its
    /// conversation owner is, and the topic that conversation carries now.
    ///
    /// Everything but the id is best effort. An owner Build cannot name — a
    /// run that is nobody's workspace, an entity that belongs to no project —
    /// leaves `owner` absent, and a client falls back to the id it always had.
    pub(in crate::app) fn agent_identity(&self, entity_id: &str, agent_id: &str) -> AgentIdentity {
        AgentIdentity {
            id: agent_id.to_string(),
            owner: self.conversation_owner_ref(entity_id),
            topic: Some(self.agent_topic(entity_id, agent_id)),
            name: self.agent_name(entity_id, agent_id),
            context: None,
        }
    }

    /// The agent that WROTE a message, as the message wears it: its identity,
    /// and how full its context was as it wrote — snapshotted now, because a
    /// reader wants the room the author had for these words.
    pub(in crate::app) fn sender_identity(&self, entity_id: &str, agent_id: &str) -> AgentIdentity {
        AgentIdentity {
            context: self.agent_context_reading(entity_id, agent_id),
            ..self.agent_identity(entity_id, agent_id)
        }
    }

    /// The agent's last context reading, or `None` without one.
    pub(in crate::app) fn agent_context_reading(
        &self,
        entity_id: &str,
        agent_id: &str,
    ) -> Option<crate::thread::ContextReading> {
        self.entity_agents(entity_id).ok().and_then(|agents| {
            agents
                .by_id(agent_id)
                .and_then(|agent| agent.context_reading(self.compact_above_tokens))
        })
    }

    /// What this agent is called, when it has been named. `None` rather than
    /// empty: a topic that has not been set was still read from a conversation
    /// that exists, but an unnamed agent has no name to report and the reader
    /// falls back to the ordinal.
    pub(in crate::app) fn agent_name(&self, entity_id: &str, agent_id: &str) -> Option<String> {
        self.entity_agents(entity_id)
            .ok()
            .and_then(|agents| agents.by_id(agent_id).and_then(|agent| agent.name.clone()))
    }

    /// What the agent last called its conversation, or the empty string for one
    /// that has not said. Empty rather than absent: the conversation WAS read,
    /// and it has no name yet.
    fn agent_topic(&self, entity_id: &str, agent_id: &str) -> String {
        self.entity_agents(entity_id)
            .ok()
            .and_then(|agents| agents.by_id(agent_id))
            .and_then(|agent| agent.topic.clone())
            .unwrap_or_default()
    }

    /// The workspace or project this conversation owner stands for.
    pub(in crate::app) fn conversation_owner_ref(&self, entity_id: &str) -> Option<AgentOwnerRef> {
        self.conversation_owner_candidates(entity_id)?.resolve()
    }

    /// What [`Self::conversation_owner_ref`] decides between, read out of the
    /// app state without touching the disk, so the paths can be resolved
    /// after the app lock is let go (#200's push content).
    pub(in crate::app) fn conversation_owner_candidates(
        &self,
        entity_id: &str,
    ) -> Option<OwnerCandidates> {
        let project_id = self.projects.project_id_of(entity_id)?;
        let root = self.runs.get(entity_id)?.worktree.path.clone();
        Some(OwnerCandidates {
            root,
            project: self.projects.get(project_id).map(|project| {
                (
                    scratch_dir(&self.state_root, &project.repo_path),
                    AgentOwnerRef {
                        kind: AgentOwnerKind::Project,
                        id: project.id.clone(),
                        name: project.name.clone(),
                    },
                )
            }),
            workspaces: self
                .workspaces
                .list(Some(project_id))
                .into_iter()
                .map(|workspace| {
                    (
                        workspace.root.clone(),
                        AgentOwnerRef {
                            kind: AgentOwnerKind::Workspace,
                            id: workspace.id.clone(),
                            name: workspace.name.clone(),
                        },
                    )
                })
                .collect(),
        })
    }
}

/// An entity's run root, and the project scratch root and workspace roots it
/// may be. Owned, so it can leave the app lock.
pub(in crate::app) struct OwnerCandidates {
    root: PathBuf,
    /// The project's scratch root and its owner, when the project is known.
    project: Option<(PathBuf, AgentOwnerRef)>,
    workspaces: Vec<(PathBuf, AgentOwnerRef)>,
}

impl OwnerCandidates {
    /// Which one the root is, through symlinks ([`same_path`]), so this reads
    /// the disk: once per workspace at most.
    ///
    /// A project's owner is asked about first: it is a run in the project's
    /// scratch directory, which is no workspace's root, so asking the other way
    /// round would answer nothing for it.
    pub(in crate::app) fn resolve(self) -> Option<AgentOwnerRef> {
        if let Some((scratch, project)) = self.project {
            if same_path(&self.root, &scratch) {
                return Some(project);
            }
        }
        self.workspaces
            .into_iter()
            .find(|(workspace_root, _)| same_path(workspace_root, &self.root))
            .map(|(_, owner)| owner)
    }
}

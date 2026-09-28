//! A project as a conversation owner.
//!
//! A workspace mints an owner for its own root (`workspace.ensure_conversation`);
//! a project mints one the same way, with one difference: a project's owner is
//! rooted in a bridge-owned scratch directory, never in the project's checkout.
//! That root is where Build keeps what is its own — the owner's identity, the
//! agents' `.build/` scaffold and the files attached to the conversation — so
//! none of it is ever written into the user's code.
//!
//! The project agent itself stands in the project's base
//! ([`AppState::project_base`]): it reads the code it is orchestrating, and
//! its standing instructions forbid it to change anything there — every change
//! goes through a workspace.

use super::safe_mount_name;
use crate::agent::AgentOwner;
use crate::app::{has_agent_choice, model_choice_from, require_str, sha256_hex, AppState};
use crate::models::{AgentProvider, ModelChoice};
use crate::orchestrator::ActiveRun;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// The directory project scratch space lives under, inside Build's own state
/// directory. Unlike the router's scratch next to it, this one outlives every
/// session that works in it: the conversation is durable, so its working root
/// has to be too.
pub(in crate::app) const PROJECT_SCRATCH_DIR_NAME: &str = "project-scratch";

/// Where the conversation owner of the project at `project_path` works.
///
/// Keyed by the project's canonical path, not by its `proj-N` id: an id is
/// minted per boot from the config that restored it and two boots can spell the
/// same repository differently, which would strand the directory the last boot
/// filled. The last segment is carried for legibility; the digest is what makes
/// it unique.
pub(in crate::app) fn scratch_dir(state_root: &Path, project_path: &Path) -> PathBuf {
    state_root
        .join(PROJECT_SCRATCH_DIR_NAME)
        .join(scratch_key(project_path))
}

fn scratch_key(project_path: &Path) -> String {
    let name = project_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("project");
    let digest = sha256_hex(project_path.as_os_str().as_encoded_bytes());
    format!("{}-{}", safe_mount_name(name), &digest[..12])
}

impl AppState {
    /// Ensure this project has an entity for conversations and agents. Like
    /// `workspace.ensure_conversation` this performs no Git adoption and
    /// chooses no source checkout: the owner is minted over the project's own
    /// scratch directory, which is cut here if this is the first call.
    ///
    /// Idempotent, across a restart as well as within one boot: the owner is
    /// found again by the root it works in, and that root is derived from the
    /// project's canonical path.
    pub(crate) fn project_ensure_conversation(&mut self, params: &Value) -> Result<Value, String> {
        let project_id = require_str(params, "project_id")?;
        let project = self
            .projects
            .get(&project_id)
            .ok_or_else(|| format!("unknown project_id: {project_id}"))?;
        let project_name = project.name.clone();
        let scratch = scratch_dir(&self.state_root, &project.repo_path);
        if let Some(run_id) = self.project_conversation_owner(&project_id, &scratch) {
            return Ok(conversation(&project_id, &run_id));
        }

        let model_choice = self.asked_project_agent_choice(params)?;
        std::fs::create_dir_all(&scratch).map_err(|error| {
            format!(
                "could not cut project scratch at {}: {error}",
                scratch.display()
            )
        })?;
        let run_id = format!("run-{}", uuid::Uuid::new_v4());
        let active = ActiveRun::workspace_conversation(
            crate::run::RunId::new(&run_id),
            project_name,
            scratch,
            model_choice,
        );
        self.projects
            .bind_entity(run_id.clone(), project_id.clone());
        if let Err(error) = self.finish_run_mutation(run_id.clone(), active) {
            self.runs.remove(&run_id);
            self.forget_run(&run_id);
            return Err(error);
        }
        Ok(conversation(&project_id, &run_id))
    }

    /// What this device says a project agent starts on, as a selection to
    /// spend: the setting, with the default harness standing where it names no
    /// harness of its own.
    pub(in crate::app) fn project_agent_choice(&self) -> ModelChoice {
        self.runnable_default(self.project_agent.resolved(self.default_harness))
    }

    /// The harness a project agent falls back to when the caller names none.
    fn project_agent_harness(&self) -> AgentProvider {
        self.project_agent.provider.unwrap_or(self.default_harness)
    }

    /// What `project.ensure_conversation` mints on: the device's answer, and
    /// the client's where it gives one.
    ///
    /// The flattened `provider`/`model`/`effort` are the API this verb has
    /// always had and they still win — a caller that names a harness gets it.
    /// Saying nothing is no longer "the default harness": it is this device's
    /// project-agent choice, which is where the question belongs now.
    fn asked_project_agent_choice(&self, params: &Value) -> Result<ModelChoice, String> {
        if has_agent_choice(params) {
            return model_choice_from(params, self.project_agent_harness());
        }
        Ok(self.project_agent_choice())
    }

    /// The choice a NEW agent on `entity_id` is minted on.
    ///
    /// A project's conversation is the device's to configure — its agent talks
    /// about the project rather than working in a checkout — so a mint there
    /// reads the device's setting, and moving the setting moves what the next
    /// project agent opens on. Every other entity keeps the choice persisted on
    /// it: that one was made for the work in it.
    pub(in crate::app) fn mint_model_choice(&self, entity_id: &str) -> Result<ModelChoice, String> {
        if self.is_project_conversation_owner(entity_id) {
            return Ok(self.project_agent_choice());
        }
        self.entity_model_choice(entity_id)
    }

    /// Which kind of agent an owner mints, read off the owner itself: a
    /// project's conversation owner mints project agents and everything else
    /// mints coding ones. Whoever asked for the agent never decides.
    pub(in crate::app) fn agent_owner<'a>(&self, owner_id: &'a str) -> AgentOwner<'a> {
        if self.is_project_conversation_owner(owner_id) {
            AgentOwner::project(owner_id)
        } else {
            AgentOwner::from(owner_id)
        }
    }

    /// Whether `run_id` is the conversation owner of a project: the run bound
    /// to a project that stands in that project's scratch directory. Derived
    /// from the record rather than stored, so it still answers for an owner a
    /// restart restored.
    pub(in crate::app) fn is_project_conversation_owner(&self, run_id: &str) -> bool {
        let Some(active) = self.runs.get(run_id) else {
            return false;
        };
        let Some(project) = self
            .projects
            .project_id_of(run_id)
            .and_then(|project_id| self.projects.get(project_id))
        else {
            return false;
        };
        crate::app::workspaces::same_path(
            &active.worktree.path,
            &scratch_dir(&self.state_root, &project.repo_path),
        )
    }

    /// The live run that owns this project's scratch root. Read by path rather
    /// than by a stored id, so a boot that re-minted `proj-N` still finds the
    /// owner the last boot left — and only a live run wins, so a terminal one
    /// never keeps a project from starting over.
    pub(in crate::app) fn project_conversation_owner(
        &self,
        project_id: &str,
        scratch: &Path,
    ) -> Option<String> {
        self.runs
            .iter()
            .find(|(run_id, active)| {
                !active.run.state.is_terminal()
                    && self.projects.project_id_of(run_id) == Some(project_id)
                    && crate::app::workspaces::same_path(&active.worktree.path, scratch)
            })
            .map(|(run_id, _)| run_id.clone())
    }

    /// The project's base: the directory a project agent stands in.
    ///
    /// The project's own path, `repo_path` — the primary source, the folder
    /// the project was added from and the one a primary terminal opens in.
    /// Not the folder it sits in: nothing says that folder holds only this
    /// project, and on a machine where it is itself an old checkout its
    /// instructions and stale copies of the code would be read as this
    /// project's. A project with several sources names the others in the
    /// agent's prompt. `None` while the folder is missing, and the agent then
    /// starts in its scratch root rather than failing to start at all.
    pub(in crate::app) fn project_base(&self, project_id: &str) -> Option<PathBuf> {
        let project = self.projects.get(project_id)?;
        project
            .repo_path
            .is_dir()
            .then(|| project.repo_path.clone())
    }

    /// Where an agent on `owner`'s conversation is started: the project's base
    /// for a project's own conversation, the owner's root for everything else.
    pub(in crate::app) fn agent_process_cwd(&self, owner: &str, root: &Path) -> PathBuf {
        if self.is_project_conversation_owner(owner) {
            let base = self
                .projects
                .project_id_of(owner)
                .and_then(|project_id| self.project_base(project_id));
            if let Some(base) = base {
                return base;
            }
        }
        root.to_path_buf()
    }

    /// The conversation owner this project has right now, or `None` for a
    /// project nobody has talked to yet — the lookup above, asked by project id
    /// alone. `project.list` carries the answer so a rail can show a project's
    /// agent without minting one to find out there is none.
    pub(in crate::app) fn project_conversation_run(&self, project_id: &str) -> Option<String> {
        let project = self.projects.get(project_id)?;
        let scratch = scratch_dir(&self.state_root, &project.repo_path);
        self.project_conversation_owner(project_id, &scratch)
    }
}

/// The shape `workspace.ensure_conversation` answers in, with the project named
/// where the workspace was.
fn conversation(project_id: &str, run_id: &str) -> Value {
    json!({
        "project_id": project_id,
        "entity_id": run_id,
        "run_id": run_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Scratch is Build's, per project, durable, and outside every checkout —
    /// the same path always lands in the same directory, whatever id the
    /// project happens to be wearing.
    #[test]
    fn scratch_is_one_directory_per_project_path_under_builds_own_state() {
        let root = Path::new("/home/dev/.build");
        let project = Path::new("/home/dev/code/build");
        let one = scratch_dir(root, project);
        let namesake = scratch_dir(root, Path::new("/home/dev/forks/build"));

        assert_eq!(one, scratch_dir(root, project), "keyed by the path alone");
        assert_ne!(
            one, namesake,
            "two projects named alike are two directories"
        );
        assert!(one.starts_with(root.join(PROJECT_SCRATCH_DIR_NAME)));
        assert!(!one.starts_with(project), "never inside the checkout");
        assert!(
            one.file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("build-"),
            "{}",
            one.display()
        );
    }
}

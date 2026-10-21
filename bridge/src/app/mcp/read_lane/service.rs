//! Handles captured once at listener setup; reads never retain AppState.
use crate::app::read_context::{HarnessSettings, McpAddresses, McpBootstrap, ReadService};
use crate::app::read_view::{ReadView, ReadViewSlot};
use crate::app::runtime::sessions::{McpAuthoritySnapshot, McpReadAuthority};
use crate::app::AppState;
use crate::harness::installed::Readings;
use crate::store::ReadPool;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub(in crate::app::mcp) struct McpReadService {
    pub(super) read: Arc<ReadService>,
    pub(super) view: ReadViewSlot,
    pub(super) pool: Option<ReadPool>,
    pub(super) admission: Option<Arc<crate::update::AdmissionGate>>,
    pub(super) readings: Arc<Readings>,
    authority: McpReadAuthority,
    #[cfg(test)]
    pub(super) gate: Option<crate::test_support::off_lock::OffLockGate>,
    #[cfg(test)]
    pub(in crate::app::mcp) sql_probe: Option<super::probe::SqlReadProbe>,
}

pub(super) struct Inputs {
    pub(super) addresses: Arc<McpAddresses>,
    pub(super) bootstrap: Arc<McpBootstrap>,
    pub(super) authority: McpAuthoritySnapshot,
    pub(super) view: Arc<ReadView>,
    deleting_project: bool,
}

impl McpReadService {
    /// This constructor is called only while building the listener/handler.
    pub(in crate::app::mcp) fn capture(app: &AppState) -> Arc<Self> {
        app.initialize_read_context();
        Arc::new(Self {
            read: Arc::new(app.read_service()),
            view: app.read_view.clone(),
            pool: app.store.as_ref().map(crate::store::Store::read_pool),
            admission: app.update_admission(),
            readings: app.cli_readings.clone(),
            authority: app.session_registry.mcp_read_authority(),
            #[cfg(test)]
            gate: app.off_lock_gate.clone(),
            #[cfg(test)]
            sql_probe: None,
        })
    }

    pub(super) fn inputs(&self) -> Inputs {
        let authority = self.authority.load();
        let (addresses, bootstrap, deleting_project) = self.read.mcp_input_snapshot();
        Inputs {
            authority,
            addresses,
            bootstrap,
            deleting_project,
            view: self.view.load(),
        }
    }
}

impl Inputs {
    pub(super) fn project_binding(&self, owner: &str) -> Result<&str, String> {
        let binding = match &self.view.observations {
            Some(context) => context.scopes.mcp_project_binding(owner),
            None => self.bootstrap.project_binding(owner),
        };
        binding.ok_or_else(|| format!("{owner} belongs to no project"))
    }

    pub(super) fn project_path(&self, id: &str) -> Result<&Path, String> {
        let path = match &self.view.observations {
            Some(context) => context.scopes.mcp_project_path(id),
            None => self.bootstrap.project_path(id),
        };
        path.ok_or_else(|| format!("unknown project_id: {id}"))
    }

    pub(super) fn project(&self, owner: &str) -> Result<(&str, &Path), String> {
        let id = self.project_binding(owner)?;
        Ok((id, self.project_path(id)?))
    }

    pub(super) fn deleting_project(&self) -> bool {
        self.deleting_project
    }

    pub(super) fn workspace(&self, id: &str) -> Result<&str, String> {
        let project = match &self.view.observations {
            Some(context) => context.scopes.mcp_workspace_project(id),
            None => self.bootstrap.workspace_project(id),
        };
        project.ok_or_else(|| format!("unknown workspace_id: {id}"))
    }

    pub(super) fn workspace_selection(&self, id: &str) -> Result<Option<String>, String> {
        self.require_rosters()?;
        self.view
            .global
            .as_ref()
            .and_then(|global| global.mcp_workspace_selection(id))
            .map(|(_, owner)| owner.map(str::to_owned))
            .ok_or_else(|| format!("unknown workspace_id: {id}"))
    }

    pub(super) fn project_selection(&self, id: &str) -> Result<Option<String>, String> {
        self.require_rosters()?;
        self.view
            .global
            .as_ref()
            .and_then(|global| global.mcp_project_selection(id))
            .map(|owner| owner.map(str::to_owned))
            .ok_or_else(|| format!("unknown project_id: {id}"))
    }

    pub(super) fn require_rosters(&self) -> Result<(), String> {
        if self.view.global.is_none() || self.view.observations.is_none() {
            Err("agent.list: snapshot was not initialized".into())
        } else {
            Ok(())
        }
    }

    pub(super) fn harness_settings(&self) -> Result<HarnessSettings, String> {
        match HarnessSettings::published(&self.view)? {
            Some(settings) => Ok(settings),
            None if self.view.observations.is_none() => Ok(self.bootstrap.settings.clone()),
            None => Err("settings.get: snapshot was not initialized".into()),
        }
    }

    pub(super) fn guard_reservation(&self, conversation: &str) -> Result<(), String> {
        if self.authority.is_resetting(conversation) {
            Err("conversation.reset: conversation is being cleared; try again shortly".into())
        } else {
            Ok(())
        }
    }
}

pub(super) struct ProjectScope {
    owner: String,
    pub(super) id: String,
    pub(super) path: PathBuf,
    on_rebind: String,
}

impl ProjectScope {
    pub(super) fn capture(inputs: &Inputs, owner: &str, on_rebind: String) -> Result<Self, String> {
        let (id, path) = inputs.project(owner)?;
        Ok(Self {
            owner: owner.into(),
            id: id.into(),
            path: path.into(),
            on_rebind,
        })
    }

    pub(super) fn validate(&self, inputs: &Inputs) -> Result<(), String> {
        let id = inputs.project_binding(&self.owner)?;
        if id != self.id {
            return Err(self.on_rebind.clone());
        }
        if inputs.project_path(id)? != self.path.as_path() {
            return Err(format!("unknown project_id: {}", self.id));
        }
        if inputs.deleting_project() {
            return Err(crate::app::rpc::PROJECT_DELETION_IN_PROGRESS.into());
        }
        Ok(())
    }
}

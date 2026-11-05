use super::SecretSessionAuthority;
use crate::app::read_context::ReadContextSlot;
use crate::app::AppState;
use crate::secrets::SecretCaller;

/// Resource-free authority handle for private release workers, outside AppState.
#[derive(Clone)]
pub(crate) struct SecretReleaseAuthority {
    sessions: SecretSessionAuthority,
    committed: ReadContextSlot,
    resets: super::McpReadAuthority,
}

impl SecretReleaseAuthority {
    pub(crate) fn authenticate(&self, agent: &str, token: &str) -> Option<SecretCaller> {
        let caller = self.sessions.authenticate(agent, token)?;
        self.revalidate(&caller).then_some(caller)
    }

    pub(crate) fn revalidate(&self, caller: &SecretCaller) -> bool {
        self.sessions.is_current(caller)
            && !self.resets.load().is_resetting(&caller.conversation_id)
            && self
                .committed
                .load()
                .as_ref()
                .is_some_and(|context| context.validates_secret_caller(caller))
    }
}

impl AppState {
    pub(crate) fn secret_release_authority(&self) -> super::SecretReleaseAuthority {
        self.initialize_read_context();
        SecretReleaseAuthority {
            sessions: self.session_registry.secret_authority(),
            committed: self.read_context.clone(),
            resets: self.session_registry.mcp_read_authority(),
        }
    }

    pub(in crate::app) fn mint_secret_launch(
        &mut self,
        issuer: &super::McpIssuer,
        project: &str,
    ) -> Option<crate::harness::SecretLaunchEnvironment> {
        let super::McpIssuer::Coding(address) = issuer else {
            return None;
        };
        let (device, channel, socket) = self.session_registry.secret_launch_context()?;
        // A canonical alias cannot turn another project's conversation into a grant.
        if self.projects.project_id_of(&address.canonical_owner) != Some(project) {
            return None;
        }
        let token = uuid::Uuid::new_v4().to_string();
        let caller = SecretCaller {
            device_id: device.into(),
            channel: channel.into(),
            project_id: project.into(),
            owner_id: address.owner.clone(),
            canonical_owner_id: address.canonical_owner.clone(),
            agent_id: address.agent.clone(),
            conversation_id: address.conversation.clone(),
            generation: address.thread_id.clone(),
            thread_id: address.thread_id.clone(),
            generation_revision: address.generation,
            session_incarnation: uuid::Uuid::new_v4().to_string(),
        };
        let launch = crate::harness::SecretLaunchEnvironment::new(
            socket.into(),
            channel.into(),
            address.agent.clone(),
            address.owner.clone(),
            token.clone(),
            std::env::current_exe()
                .ok()?
                .into_os_string()
                .into_string()
                .ok()?,
        );
        self.session_registry.install_secret_token(token, caller);
        Some(launch)
    }
}

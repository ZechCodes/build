//! Private immediate capability and runtime fences for independent MCP reads.
//! Producers remain serialized by AppState; immutable readers take no app guard.
use super::registry::constant_time_token_eq;
use crate::app::read_context::IssuedCodingAddress;
use crate::app::AppState;
use arc_swap::ArcSwap;
use imbl::{OrdMap, OrdSet};
use std::sync::Arc;

#[derive(Clone, PartialEq, Eq)]
pub(in crate::app) enum McpIssuer {
    Coding(IssuedCodingAddress),
    Router(RouterAuthority),
}

/// Resource-free runtime incarnation: retained roots cannot keep a session alive.
#[derive(Clone)]
pub(in crate::app) struct RouterAuthority {
    pub(in crate::app) capture: String,
    pub(in crate::app) agent: String,
    incarnation: Arc<()>,
}

impl PartialEq for RouterAuthority {
    fn eq(&self, other: &Self) -> bool {
        self.capture == other.capture
            && self.agent == other.agent
            && Arc::ptr_eq(&self.incarnation, &other.incarnation)
    }
}
impl Eq for RouterAuthority {}

// Deliberately no Debug/Serialize implementations on secret-bearing data.
#[derive(Clone)]
struct Capability {
    secret: Arc<str>,
    issuer: McpIssuer,
}

#[derive(Clone, Default)]
struct AuthorityRoot {
    capabilities: OrdMap<String, Capability>,
    routers: OrdMap<String, RouterAuthority>,
    resetting: OrdSet<String>,
}

#[derive(Clone)]
pub(in crate::app) struct McpReadAuthority {
    root: Arc<ArcSwap<AuthorityRoot>>,
}

pub(in crate::app) struct McpAuthoritySnapshot(Arc<AuthorityRoot>);

impl McpReadAuthority {
    pub(super) fn new() -> Self {
        Self {
            root: Arc::new(ArcSwap::new(Arc::new(AuthorityRoot::default()))),
        }
    }

    pub(in crate::app) fn load(&self) -> McpAuthoritySnapshot {
        McpAuthoritySnapshot(self.root.load_full())
    }

    // Only SessionRegistry's app-guarded producer methods call these writers.
    fn update(&self, change: impl FnOnce(&mut AuthorityRoot)) {
        let mut next = (*self.root.load_full()).clone();
        change(&mut next);
        self.root.store(Arc::new(next));
    }

    pub(super) fn install(&self, agent: String, token: &str, issuer: McpIssuer) {
        self.update(|root| {
            root.capabilities.insert(
                agent,
                Capability {
                    secret: Arc::from(token),
                    issuer,
                },
            );
        });
    }

    pub(super) fn revoke(&self, agent: &str) {
        self.update(|root| {
            root.capabilities.remove(agent);
        });
    }

    pub(super) fn register_router(&self, capture: String, agent: String) -> RouterAuthority {
        let router = RouterAuthority {
            capture,
            agent,
            incarnation: Arc::new(()),
        };
        self.update(|root| {
            root.routers.insert(router.agent.clone(), router.clone());
        });
        router
    }

    pub(super) fn remove_router(&self, capture: &str, agent: &str) {
        self.update(|root| {
            if root
                .routers
                .get(agent)
                .is_some_and(|router| router.capture == capture)
            {
                root.routers.remove(agent);
            }
        });
    }

    pub(super) fn set_resetting(&self, conversation: &str, resetting: bool) {
        self.update(|root| {
            if resetting {
                root.resetting.insert(conversation.into());
            } else {
                root.resetting.remove(conversation);
            }
        });
    }
}

impl McpAuthoritySnapshot {
    /// Authentication checks bytes before exposing non-secret issuer data.
    /// Coding issuers must additionally match saved committed addressing.
    pub(in crate::app) fn authenticate_token(
        &self,
        agent: &str,
        supplied: &str,
    ) -> Option<McpIssuer> {
        let held = self.0.capabilities.get(agent)?;
        if !constant_time_token_eq(supplied, &held.secret) {
            return None;
        }
        match &held.issuer {
            McpIssuer::Router(router) if !self.router_is_current(router) => None,
            issuer => Some(issuer.clone()),
        }
    }

    pub(in crate::app) fn is_resetting(&self, conversation: &str) -> bool {
        self.0.resetting.contains(conversation)
    }

    pub(in crate::app) fn router_is_current(&self, captured: &RouterAuthority) -> bool {
        self.0.routers.get(&captured.agent) == Some(captured)
    }

    pub(super) fn router_issuer(&self, capture: &str, agent: &str) -> Option<RouterAuthority> {
        self.0
            .routers
            .get(agent)
            .filter(|router| router.capture == capture)
            .cloned()
    }
}

impl AppState {
    /// Capture the reservation's validated live address before any child exists.
    /// Never infer its issuer from a possibly older committed publication.
    pub(in crate::app) fn live_mcp_issuer(
        &self,
        owner: &str,
        agent: &str,
    ) -> Result<McpIssuer, String> {
        if crate::router::is_router_agent(agent) {
            self.session_registry
                .mcp_read_authority()
                .load()
                .router_issuer(owner, agent)
                .map(McpIssuer::Router)
                .ok_or_else(|| format!("unknown router agent: {agent}"))
        } else {
            self.live_mcp_issued_address(owner, agent)
                .map(McpIssuer::Coding)
        }
    }

    #[cfg(test)]
    pub(in crate::app) fn test_install_mcp_token(&mut self, agent: String, token: String) {
        let endpoint = self
            .addressed_session(agent.clone())
            .expect("fixture has a live MCP identity");
        let owner = match endpoint {
            crate::app::mcp::AddressedSession::Coding(generation) => generation.entity_id,
            crate::app::mcp::AddressedSession::Router { capture_id, .. } => capture_id,
        };
        let issuer = self
            .live_mcp_issuer(&owner, &agent)
            .expect("fixture issuer is explicit and valid");
        self.session_registry
            .install_mcp_token(agent, token, issuer);
    }
}

#[cfg(test)]
mod tests;

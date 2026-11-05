//! Separate purpose-limited launch capabilities; never accepts MCP tokens.
use super::registry::constant_time_token_eq;
use crate::secrets::SecretCaller;
use arc_swap::ArcSwap;
use imbl::OrdMap;
use std::sync::Arc;

// Neither the capability nor its root implements Debug/Serialize.
#[derive(Clone)]
struct SecretCapability {
    token: Arc<str>,
    caller: SecretCaller,
}

#[derive(Clone)]
pub(in crate::app) struct SecretSessionAuthority {
    root: Arc<ArcSwap<OrdMap<String, SecretCapability>>>,
}

impl SecretSessionAuthority {
    pub(super) fn new() -> Self {
        Self {
            root: Arc::new(ArcSwap::from_pointee(OrdMap::new())),
        }
    }

    pub(super) fn clear(&self) {
        self.root.store(Arc::new(OrdMap::new()));
    }

    pub(super) fn install(&self, token: String, caller: SecretCaller) {
        let mut next = (*self.root.load_full()).clone();
        next.insert(
            caller.agent_id.clone(),
            SecretCapability {
                token: Arc::from(token),
                caller,
            },
        );
        self.root.store(Arc::new(next));
    }

    pub(super) fn revoke(&self, agent: &str) {
        let mut next = (*self.root.load_full()).clone();
        next.remove(agent);
        self.root.store(Arc::new(next));
    }

    pub(in crate::app) fn authenticate(&self, agent: &str, token: &str) -> Option<SecretCaller> {
        let current = self.root.load_full();
        let held = current.get(agent)?;
        constant_time_token_eq(token, &held.token).then(|| held.caller.clone())
    }

    pub(in crate::app) fn is_current(&self, caller: &SecretCaller) -> bool {
        self.root
            .load()
            .get(&caller.agent_id)
            .is_some_and(|held| held.caller == *caller)
    }
}

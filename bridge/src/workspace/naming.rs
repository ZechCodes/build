//! Naming requests are once per workspace in this bridge process.
use super::*;

impl WorkspaceRegistry {
    pub(crate) fn claim_name_request(&mut self, id: &str) -> bool {
        self.get(id)
            .is_some_and(|workspace| workspace.waiting_for_name)
            && self.name_requests.insert(id.to_string())
    }

    pub(crate) fn release_name_request(&mut self, id: &str) {
        self.name_requests.remove(id);
    }
}

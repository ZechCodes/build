//! Attachment identity and the current server-issued terminal ID on one socket.

use std::{collections::HashMap, sync::Arc};

use crate::execution::session::TerminalInterest;

use super::wire::Method;

const MAX_INTERESTS: usize = 64;

#[derive(Default)]
pub(super) struct Interests {
    entries: HashMap<String, Attachment>,
    next_order: u64,
}

struct Attachment {
    interest: Arc<TerminalInterest>,
    order: u64,
    method: Method,
    active: bool,
    term_id: Option<String>,
}

impl Interests {
    pub(super) fn contains(&self, id: &str) -> bool {
        self.entries.contains_key(id)
    }

    pub(super) fn registration(&self, id: &str) -> Option<Arc<TerminalInterest>> {
        self.entries.get(id).map(|entry| entry.interest.clone())
    }

    pub(super) fn begin(
        &mut self,
        id: String,
        method: Method,
        requested_term: Option<&str>,
    ) -> Option<Arc<TerminalInterest>> {
        if self.entries.len() >= MAX_INTERESTS {
            // Reuse an existing slot when an explicit shell ID is being
            // replaced. Agent targets may resolve or rekey inside the handler.
            let term = requested_term.filter(|_| method == Method::Attach)?;
            let old = self.entries.iter().find_map(|(id, entry)| {
                (entry.active && entry.term_id.as_deref() == Some(term)).then(|| id.clone())
            })?;
            self.remove(&old);
        }
        let interest = TerminalInterest::new();
        let order = self.next_order;
        self.next_order += 1;
        self.entries.insert(
            id,
            Attachment {
                interest: interest.clone(),
                method,
                order,
                active: false,
                term_id: None,
            },
        );
        Some(interest)
    }

    pub(super) fn for_effect(&self, term_id: &str) -> Option<Arc<TerminalInterest>> {
        self.entries.values().find_map(|entry| {
            (entry.active && entry.term_id.as_deref() == Some(term_id))
                .then(|| entry.interest.clone())
        })
    }

    /// Accept a server-issued rekey only from the exact producer that owns
    /// the interest. Its first event can precede its correlated reply.
    pub(super) fn observe_push(&mut self, interest: &Arc<TerminalInterest>, term_id: &str) -> bool {
        let Some((id, entry)) = self
            .entries
            .iter_mut()
            .find(|(_, entry)| Arc::ptr_eq(&entry.interest, interest))
        else {
            return false;
        };
        if entry.method == Method::AgentAttach && entry.term_id.as_deref() != Some(term_id) {
            entry.interest.rekey();
            entry.term_id = Some(term_id.into());
            if entry.active {
                let id = id.clone();
                return self.replace_older(&id, term_id);
            }
        }
        true
    }

    pub(super) fn attach_reply(&mut self, id: &str, term_id: Option<&str>) {
        let Some(term_id) = term_id else {
            self.remove(id);
            return;
        };
        let Some(entry) = self.entries.get_mut(id) else {
            return;
        };
        entry.active = true;
        let term_id = entry.term_id.get_or_insert_with(|| term_id.into()).clone();
        self.replace_older(id, &term_id);
    }

    fn replace_older(&mut self, id: &str, term_id: &str) -> bool {
        let latest = self
            .entries
            .iter()
            .filter(|(_, entry)| entry.active && entry.term_id.as_deref() == Some(term_id))
            .max_by_key(|(_, entry)| entry.order)
            .map(|(_, entry)| entry.order);
        self.entries.retain(|_, entry| {
            let retired = entry.active
                && entry.term_id.as_deref() == Some(term_id)
                && Some(entry.order) != latest;
            if retired {
                entry.interest.retire();
            }
            !retired
        });
        self.entries.contains_key(id)
    }

    pub(super) fn remove(&mut self, id: &str) -> bool {
        if let Some(entry) = self.entries.remove(id) {
            entry.interest.retire();
            return true;
        }
        false
    }

    pub(super) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

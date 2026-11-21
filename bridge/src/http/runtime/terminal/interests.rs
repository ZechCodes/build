//! Attachment identity and the current server-issued terminal ID on one socket.

use std::{collections::HashMap, sync::Arc};

use crate::execution::session::{TerminalInterest, TerminalSnapshotLane};

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
    delivered_cursor: Option<u64>,
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
        snapshots: Option<TerminalSnapshotLane>,
    ) -> Option<Arc<TerminalInterest>> {
        if self.entries.len() >= MAX_INTERESTS {
            return None;
        }
        let interest = TerminalInterest::new(snapshots);
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
                delivered_cursor: None,
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
        if entry.term_id.is_none() && entry.method == Method::Attach {
            entry.term_id = Some(term_id.into());
        }
        if entry.method == Method::AgentAttach && entry.term_id.as_deref() != Some(term_id) {
            entry.interest.rekey();
            entry.term_id = Some(term_id.into());
            entry.delivered_cursor = None;
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
                entry.interest.supersede();
            }
            !retired
        });
        self.entries.contains_key(id)
    }

    pub(super) fn permits_ack(&self, interest: &Arc<TerminalInterest>, cursor: u64) -> bool {
        self.entries.values().any(|entry| {
            Arc::ptr_eq(&entry.interest, interest)
                && entry
                    .delivered_cursor
                    .is_some_and(|delivered| cursor <= delivered)
        })
    }

    pub(super) fn record_delivery(
        &mut self,
        interest: &Arc<TerminalInterest>,
        term_id: &str,
        cursor: u64,
    ) {
        if let Some(entry) = self.entries.values_mut().find(|entry| {
            Arc::ptr_eq(&entry.interest, interest) && entry.term_id.as_deref() == Some(term_id)
        }) {
            entry.delivered_cursor = Some(
                entry
                    .delivered_cursor
                    .map_or(cursor, |previous| previous.max(cursor)),
            );
        }
    }

    pub(super) fn remove_failed(&mut self, interest: &Arc<TerminalInterest>) -> Option<String> {
        let id = self
            .entries
            .iter()
            .find(|(_, entry)| entry.active && Arc::ptr_eq(&entry.interest, interest))?
            .0
            .clone();
        self.remove(&id);
        Some(id)
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

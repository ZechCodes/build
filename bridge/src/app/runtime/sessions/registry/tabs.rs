//! Shell churn must not rehash the agent registry. Native agent iteration
//! order breaks ties in legacy checkout views and is part of projection layout;
//! changing that order would invalidate the next unrelated owner publication.
use super::{Tab, TabKey};
use std::collections::HashMap;
use std::ops::Index;

#[derive(Default)]
pub(super) struct Tabs {
    agents: HashMap<TabKey, Tab>,
    shells: HashMap<TabKey, Tab>,
}

impl Tabs {
    pub(super) fn new() -> Self {
        Self::default()
    }

    fn map(&self, key: &TabKey) -> &HashMap<TabKey, Tab> {
        if key.is_agent() {
            &self.agents
        } else {
            &self.shells
        }
    }

    fn map_mut(&mut self, key: &TabKey) -> &mut HashMap<TabKey, Tab> {
        if key.is_agent() {
            &mut self.agents
        } else {
            &mut self.shells
        }
    }

    pub(super) fn insert(&mut self, key: TabKey, tab: Tab) -> Option<Tab> {
        self.map_mut(&key).insert(key, tab)
    }

    pub(super) fn remove(&mut self, key: &TabKey) -> Option<Tab> {
        self.map_mut(key).remove(key)
    }

    pub(super) fn get(&self, key: &TabKey) -> Option<&Tab> {
        self.map(key).get(key)
    }

    pub(super) fn get_mut(&mut self, key: &TabKey) -> Option<&mut Tab> {
        self.map_mut(key).get_mut(key)
    }

    pub(super) fn contains_key(&self, key: &TabKey) -> bool {
        self.map(key).contains_key(key)
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.agents.len() + self.shells.len()
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = (&TabKey, &Tab)> {
        self.agents.iter().chain(self.shells.iter())
    }

    pub(super) fn keys(&self) -> impl Iterator<Item = &TabKey> {
        self.agents.keys().chain(self.shells.keys())
    }

    pub(super) fn values(&self) -> impl Iterator<Item = &Tab> {
        self.agents.values().chain(self.shells.values())
    }
}

impl Index<&TabKey> for Tabs {
    type Output = Tab;

    fn index(&self, key: &TabKey) -> &Tab {
        self.get(key).expect("the registered tab exists")
    }
}

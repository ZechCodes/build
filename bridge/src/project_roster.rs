//! A project's roster: exactly one pinned coordinator, and workers below it.
//!
//! A project's conversation owner holds an ordinary [`AgentRoster`]. What sets
//! its agents apart is PURPOSE. The pinned agent coordinates: it gets the
//! project tools and the read-only-by-instruction prompt. Every other agent is
//! a worker that reads and writes the project's original sources with the
//! coding tools. The pin is persisted by id on the owner and the purpose on each
//! agent, and the two are settled together here.
//!
//! Once a pin has been recorded it is never chosen again from a roster
//! position: a recorded pin that is missing leaves the roster without one, for
//! the bridge to mint a fresh coordinator, rather than promoting whichever
//! worker happens to sit first.

use serde::{Deserialize, Serialize};

use crate::agent::AgentRoster;

/// What an agent of a project's conversation owner is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectRole {
    /// The project's one pinned agent. Only the bridge makes it, and nothing
    /// but deleting the project retires it.
    Coordinator,
    /// An agent beside the pin that works in the project's original sources.
    Worker,
}

impl ProjectRole {
    pub fn as_str(self) -> &'static str {
        match self {
            ProjectRole::Coordinator => "coordinator",
            ProjectRole::Worker => "worker",
        }
    }

    pub fn from_wire(word: &str) -> Option<ProjectRole> {
        match word {
            "coordinator" => Some(ProjectRole::Coordinator),
            "worker" => Some(ProjectRole::Worker),
            _ => None,
        }
    }
}

/// One project owner's persisted pin and its roster, settled as one.
pub struct ProjectRoster<'a> {
    pinned: &'a mut Option<String>,
    agents: &'a mut AgentRoster,
}

impl<'a> ProjectRoster<'a> {
    pub fn new(pinned: &'a mut Option<String>, agents: &'a mut AgentRoster) -> Self {
        ProjectRoster { pinned, agents }
    }

    /// The pinned coordinator, when the recorded pin is on the roster.
    pub fn pin(&self) -> Option<&str> {
        let pinned = self.pinned.as_deref()?;
        self.agents.by_id(pinned).map(|agent| agent.id.as_str())
    }

    /// Name the pin and every other agent's purpose, and put the pin first in
    /// rail order so every verb that names no agent still reaches it. Answers
    /// whether anything changed, so a caller writes only a real migration.
    ///
    /// A record from before pins existed pins the agent it already called a
    /// coordinator, else its primary: the conversation the project always
    /// had. Every other agent keeps its id, history and settings as a worker.
    pub fn settle(&mut self) -> bool {
        let before = self.fingerprint();
        match self.pin_to_keep() {
            Some(pin) => self.pin_agent(&pin),
            None => self.demote_all(),
        }
        self.fingerprint() != before
    }

    /// Record `agent_id`, just added to the roster, as the pin.
    pub fn pin_minted(&mut self, agent_id: &str) {
        self.pin_agent(agent_id);
    }

    fn pin_to_keep(&self) -> Option<String> {
        if let Some(recorded) = self.pinned.as_deref() {
            return self.agents.by_id(recorded).map(|agent| agent.id.clone());
        }
        self.agents
            .iter()
            .find(|agent| agent.project_role == Some(ProjectRole::Coordinator))
            .or_else(|| self.agents.primary())
            .map(|agent| agent.id.clone())
    }

    fn pin_agent(&mut self, pin: &str) {
        self.agents.move_to_front(pin);
        for agent in self.agents.iter_mut() {
            agent.project_role = Some(if agent.id == pin {
                ProjectRole::Coordinator
            } else {
                ProjectRole::Worker
            });
        }
        *self.pinned = Some(pin.to_string());
    }

    fn demote_all(&mut self) {
        for agent in self.agents.iter_mut() {
            agent.project_role = Some(ProjectRole::Worker);
        }
    }

    /// What a settlement can change that is stored: the pin and each
    /// agent's purpose. Rail order is not among them — the store reads a
    /// roster back in ordinal order — so the pin is put first in memory on
    /// every settlement without that alone counting as a change to write.
    fn fingerprint(&self) -> (Option<String>, Vec<(String, Option<ProjectRole>)>) {
        let mut purposes: Vec<(String, Option<ProjectRole>)> = self
            .agents
            .iter()
            .map(|agent| (agent.id.clone(), agent.project_role))
            .collect();
        purposes.sort();
        (self.pinned.clone(), purposes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentOwner;
    use crate::models::ModelChoice;

    const NOW: &str = "2026-10-12T00:00:00Z";

    fn roster_of(count: usize) -> AgentRoster {
        let mut roster = AgentRoster::empty();
        for _ in 0..count {
            roster.add(
                AgentOwner::project("run-project"),
                ModelChoice::default(),
                NOW,
            );
        }
        roster
    }

    fn ids(roster: &AgentRoster) -> Vec<String> {
        roster.iter().map(|agent| agent.id.clone()).collect()
    }

    fn roles(roster: &AgentRoster) -> Vec<Option<ProjectRole>> {
        roster.iter().map(|agent| agent.project_role).collect()
    }

    #[test]
    fn a_legacy_roster_pins_its_primary_and_keeps_the_rest_as_workers() {
        let mut roster = roster_of(3);
        let before = ids(&roster);
        let mut pinned = None;

        assert!(ProjectRoster::new(&mut pinned, &mut roster).settle());

        assert_eq!(pinned.as_deref(), Some(before[0].as_str()));
        assert_eq!(ids(&roster), before, "ids and order are kept");
        assert_eq!(
            roles(&roster),
            vec![
                Some(ProjectRole::Coordinator),
                Some(ProjectRole::Worker),
                Some(ProjectRole::Worker)
            ]
        );
        assert!(
            !ProjectRoster::new(&mut pinned, &mut roster).settle(),
            "settling again changes nothing"
        );
    }

    #[test]
    fn a_recorded_pin_is_kept_wherever_it_sits_and_moved_first() {
        let mut roster = roster_of(3);
        let pin = ids(&roster)[2].clone();
        let mut pinned = Some(pin.clone());

        ProjectRoster::new(&mut pinned, &mut roster).settle();

        assert_eq!(roster.primary().unwrap().id, pin);
        assert_eq!(
            roster.primary().unwrap().project_role,
            Some(ProjectRole::Coordinator)
        );
        assert_eq!(pinned.as_deref(), Some(pin.as_str()));
    }

    #[test]
    fn a_missing_recorded_pin_is_never_replaced_by_roster_position() {
        let mut roster = roster_of(2);
        let mut pinned = Some("project-gone".to_string());
        let mut settled = ProjectRoster::new(&mut pinned, &mut roster);

        settled.settle();

        assert_eq!(settled.pin(), None, "no worker was promoted");
        assert_eq!(
            roles(&roster),
            vec![Some(ProjectRole::Worker), Some(ProjectRole::Worker)]
        );
        assert_eq!(pinned.as_deref(), Some("project-gone"));
    }

    #[test]
    fn a_coordinator_named_on_the_agent_wins_over_position() {
        let mut roster = roster_of(2);
        let second = ids(&roster)[1].clone();
        roster.by_id_mut(&second).unwrap().project_role = Some(ProjectRole::Coordinator);
        let mut pinned = None;

        ProjectRoster::new(&mut pinned, &mut roster).settle();

        assert_eq!(pinned.as_deref(), Some(second.as_str()));
        assert_eq!(roster.primary().unwrap().id, second);
    }

    #[test]
    fn an_empty_roster_has_no_pin_until_one_is_minted() {
        let mut roster = AgentRoster::empty();
        let mut pinned = None;
        assert!(!ProjectRoster::new(&mut pinned, &mut roster).settle());
        assert_eq!(pinned, None);

        let minted = roster
            .add(
                AgentOwner::project("run-project"),
                ModelChoice::default(),
                NOW,
            )
            .id
            .clone();
        ProjectRoster::new(&mut pinned, &mut roster).pin_minted(&minted);
        assert_eq!(pinned.as_deref(), Some(minted.as_str()));
        assert_eq!(
            roster.primary().unwrap().project_role,
            Some(ProjectRole::Coordinator)
        );
    }

    #[test]
    fn project_roles_have_their_wire_words() {
        for role in [ProjectRole::Coordinator, ProjectRole::Worker] {
            assert_eq!(ProjectRole::from_wire(role.as_str()), Some(role));
            assert_eq!(serde_json::to_value(role).unwrap(), role.as_str());
        }
        assert_eq!(ProjectRole::from_wire("planner"), None);
    }
}

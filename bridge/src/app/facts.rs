//! What the push bus is built over (wire spec Part 1, step 1.3): the board's
//! worktree entities, and the per-flush lookups that fill a `changes` item.
//!
//! [`ChangeBus`] knows nothing about [`AppState`]; it is handed two closures
//! at construction. Both run off the app mutex — the entity list is a map
//! snapshot the watchers hold, and the facts source takes the mutex for the
//! thread tails only, then runs the git status walks with it released.

use super::watchers::{WorktreeRoots, WorktreeWatchers};
use super::AppState;
use crate::changes::{ChangeBus, EntityFacts, FactsRequest, ThreadTip};
use crate::gitgui::{status_shape, GIT_STATUS_MAX_FILES};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

/// Where the facts source finds the state once it is shared. Filled by
/// [`AppState::shared`]; before that, or after the state is gone, the source
/// answers nothing and every item reads as "refetch".
pub(in crate::app) type FactsHandle = Arc<OnceLock<Weak<Mutex<AppState>>>>;

/// The daemon's bus: `{"kind":"all"}` resolves against the watchers' roots
/// snapshot, and a flush's keys come from [`entity_facts`].
pub(in crate::app) fn bus_with_sources(
    window: Duration,
    watchers: &Arc<WorktreeWatchers>,
    handle: &FactsHandle,
) -> Arc<ChangeBus> {
    let entities = Arc::clone(watchers);
    let handle = Arc::clone(handle);
    ChangeBus::with_sources(
        window,
        Arc::new(move || entities.entity_ids()),
        Arc::new(move |requests| entity_facts(&handle, requests)),
    )
}

/// Answer a flush's lookups. The mutex is held for the thread tails and the
/// roots only; every status walk runs with it released.
fn entity_facts(handle: &FactsHandle, requests: &[FactsRequest]) -> Vec<EntityFacts> {
    let Some(state) = handle.get().and_then(Weak::upgrade) else {
        return Vec::new();
    };
    let (roots, mut facts) = {
        let app = state.lock().unwrap();
        let facts = requests
            .iter()
            .map(|request| EntityFacts {
                entity_id: request.entity_id.clone(),
                threads: if request.thread {
                    app.thread_tips(&request.entity_id)
                } else {
                    Vec::new()
                },
                ..EntityFacts::default()
            })
            .collect::<Vec<_>>();
        (app.worktree_roots(), facts)
    };
    for (request, fact) in requests.iter().zip(facts.iter_mut()) {
        let Some(root) = roots.get(&request.entity_id).filter(|_| request.git) else {
            continue;
        };
        if let Ok((shape, key)) = status_shape(root, GIT_STATUS_MAX_FILES) {
            fact.head = shape["head"].as_str().map(str::to_string);
            fact.status_key = Some(key);
        }
    }
    facts
}

impl AppState {
    /// Every board entity with a checkout: a run's worktree, a project's
    /// primary checkout, and each external worktree the last scan found.
    pub(in crate::app) fn worktree_roots(&self) -> WorktreeRoots {
        let mut roots = WorktreeRoots::new();
        for (id, active) in &self.runs {
            if active.run.state != crate::run::RunState::Archived {
                roots.insert(id.clone(), active.worktree.path.clone());
            }
        }
        for project in self.projects.iter().filter(|project| project.is_git) {
            roots.insert(project.id.clone(), project.repo_path.clone());
            let Some(scan) = self.board.diff().external_scan_cache(&project.id) else {
                continue;
            };
            for checkout in &scan.worktrees {
                roots.insert(checkout.id.clone(), checkout.path.clone());
            }
        }
        roots
    }

    /// Where each of an entity's conversations stands — the `thread` item.
    pub(in crate::app) fn thread_tips(&self, entity_id: &str) -> Vec<ThreadTip> {
        let Ok(roster) = self.entity_agents(entity_id) else {
            return Vec::new();
        };
        roster
            .iter()
            .map(|agent| ThreadTip {
                agent_id: agent.id.clone(),
                last_sequence: self
                    .agent_conversation(entity_id, Some(&agent.id))
                    .unwrap_or(&agent.thread)
                    .last_sequence(),
            })
            .collect()
    }
}

//! What the push bus is built over (wire spec Part 1, step 1.3): the board's
//! worktree entities, and the per-flush lookups that fill a `changes` item.
//!
//! [`ChangeBus`] knows nothing about [`AppState`]; it is handed two closures
//! at construction. Both run off the app mutex — the entity list is a map
//! snapshot the watchers hold, and the facts source takes the mutex for the
//! thread tails and the state rows only, then runs the git status walks with
//! it released.

use super::watchers::{WorktreeRoots, WorktreeWatchers};
use super::AppState;
use crate::changes::{ChangeBus, EntityFacts, FactsRequest, ThreadTip};
use crate::gitgui::{status_shape, GIT_STATUS_MAX_FILES};
use serde_json::{json, Value};
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
        let roots = app.worktree_roots();
        let facts = requests
            .iter()
            .map(|request| EntityFacts {
                entity_id: request.entity_id.clone(),
                threads: if request.thread {
                    app.thread_tips(&request.entity_id, &request.thread_after)
                } else {
                    Vec::new()
                },
                state: request
                    .state
                    .then(|| app.entity_state_item(&request.entity_id))
                    .flatten(),
                terminals: request
                    .terminals
                    .then(|| roots.get(&request.entity_id))
                    .flatten()
                    .map(|root| json!({ "tabs": app.shell_tabs_at(root) })),
                ..EntityFacts::default()
            })
            .collect::<Vec<_>>();
        (roots, facts)
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

    /// Where this entity's row stands — the `state` item, which carries the
    /// WHOLE row `board.list` paints for it, so a client writes the item into
    /// its cache and repaints the feed without asking anything back.
    ///
    /// `None` for an id the board paints no row for: a project's primary
    /// checkout, a run that has been archived, a legacy issue. Those items
    /// stay the bare "refetch" they have always been.
    pub(in crate::app) fn entity_state_item(&self, entity_id: &str) -> Option<Value> {
        self.board_row(entity_id)
            .or_else(|| self.legacy_issue_digest(entity_id))
    }

    /// The row `board.list` carries for this entity: a run's folded feed row,
    /// or an external checkout's ride-along row.
    ///
    /// Read off the caches the board reads and never refreshed here. A flush
    /// answers with what the board already knows — the diffstat walk behind a
    /// row belongs to the board's own TTL, and a push must not start one per
    /// entity that moved.
    fn board_row(&self, entity_id: &str) -> Option<Value> {
        let Some(active) = self.runs.get(entity_id) else {
            return self.external_worktree_row_of(entity_id);
        };
        if active.run.state == crate::run::RunState::Archived {
            return None;
        }
        let stat = self
            .board
            .diff()
            .run_stat(entity_id)
            .map(|cached| cached.value.clone())
            .unwrap_or(Value::Null);
        Some(crate::branch::named_project_row(
            self.branch_candidate_from_run(entity_id, &stat).row,
        ))
    }

    /// The three-field digest a legacy issue still answers with. Issues left
    /// the board, so nothing paints a row for one; a client watching an old
    /// record hears the same lifecycle, agent count and attention it always
    /// heard.
    fn legacy_issue_digest(&self, entity_id: &str) -> Option<Value> {
        let lifecycle = super::plan_state_str(&self.plans.get(entity_id)?.plan.state);
        let agents = self
            .entity_agents(entity_id)
            .map(|roster| roster.iter().count())
            .unwrap_or(0);
        Some(json!({
            "run": lifecycle,
            "agents": agents,
            "attention": self.unread_for(entity_id, None).reason.unwrap_or("none"),
        }))
    }

    /// Where each of an entity's conversations stands, and what was said to
    /// get there — the `thread` item.
    ///
    /// `after` is what the subscription being flushed has already been sent,
    /// per agent. An agent it names gets the items since that sequence; one
    /// it does not — the first flush a subscription makes for a conversation
    /// its client has just read for itself — gets its tip alone.
    ///
    /// Every read here is off the tail this process holds. A cursor from
    /// under that tail is answered with the tip alone rather than with a
    /// store read: this runs under the app mutex, and the client can page
    /// forward for the gap without holding the daemon up.
    pub(in crate::app) fn thread_tips(
        &self,
        entity_id: &str,
        after: &[(String, u64)],
    ) -> Vec<ThreadTip> {
        let Ok(roster) = self.entity_agents(entity_id) else {
            return Vec::new();
        };
        roster
            .iter()
            .map(|agent| {
                let thread = self
                    .agent_conversation(entity_id, Some(&agent.id))
                    .unwrap_or(&agent.thread);
                let carried =
                    after
                        .iter()
                        .find(|(id, _)| *id == agent.id)
                        .and_then(|(_, since)| {
                            thread
                                .push_items_after(*since, crate::changes::THREAD_PUSH_MAX_ITEMS)
                                .map(|items| (*since, items))
                        });
                let (since_sequence, items) = match carried {
                    Some((since, items)) => (Some(since), items),
                    None => (None, Vec::new()),
                };
                ThreadTip {
                    agent_id: agent.id.clone(),
                    last_sequence: thread.last_sequence(),
                    items,
                    since_sequence,
                }
            })
            .collect()
    }
}

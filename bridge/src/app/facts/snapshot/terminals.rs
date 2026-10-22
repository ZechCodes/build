//! A terminal mutation publishes only the aliases of its physical checkout.
//! Newly adopted aliases need terminal facts before the next full projection.

use super::*;
use crate::app::runtime::terminals::TerminalScope;
use std::path::Path;

impl AppState {
    pub(in crate::app) fn publish_terminal_facts_at(&self, root: &Path) -> Vec<String> {
        #[cfg(test)]
        let _phase = crate::timing::phase_samples::phase("terminals.facts");
        let root = self.cached_snapshot_root(root);
        let previous = self.read_view.load();
        let mut scopes = BTreeMap::new();
        for (entity, subject) in &previous.facts.subjects {
            self.insert_terminal_alias(
                &mut scopes,
                &root,
                entity,
                &subject.root,
                subject.terminal_source.clone(),
            );
        }
        self.collect_live_terminal_aliases(&mut scopes, &root);
        let updates = scopes
            .into_iter()
            .map(|(entity, scope)| {
                let identity = previous.facts.entities.get(&entity).cloned();
                let tabs = Arc::new(json!({"tabs": self.shell_tabs_for(&scope)}));
                (entity, identity, tabs)
            })
            .collect::<Vec<_>>();
        if !updates.is_empty() {
            // Unlike board/detail facts, terminal registrations do not depend
            // on warmed observations or the current owner mutation finishing.
            self.read_view.publish_current(|current, version| {
                #[cfg(test)]
                let _phase = crate::timing::phase_samples::phase("terminals.facts.clone");
                let mut facts = (*current.facts).clone();
                for (entity, identity, tabs) in &updates {
                    if same_terminal_entity(identity.as_ref(), current.facts.entities.get(entity)) {
                        facts
                            .entities
                            .entry(entity.clone())
                            .or_insert_with(|| Arc::new(()));
                        facts.terminals.insert(entity.clone(), Arc::clone(tabs));
                    }
                }
                Some(crate::app::read_view::SnapshotDelta::new(version).set_facts(Arc::new(facts)))
            });
        }
        updates.into_iter().map(|(entity, _, _)| entity).collect()
    }

    /// A full publication also retains terminal-only aliases while their
    /// registry identity still exists. Rebuild their rows so an unrelated
    /// publication cannot erase an open/close notice queued for the next flush.
    pub(super) fn capture_terminal_aliases(&self, snapshot: &mut PushFacts, previous: &PushFacts) {
        let aliases = self
            .workspaces
            .list(None)
            .into_iter()
            .map(|workspace| (&workspace.id, &workspace.root))
            .chain(
                self.projects
                    .iter()
                    .map(|project| (&project.id, &project.repo_path)),
            );
        for (entity, root) in aliases {
            if previous.terminals.contains_key(entity) && !snapshot.terminals.contains_key(entity) {
                let scope = TerminalScope {
                    root: self.cached_snapshot_root(root),
                    source: None,
                };
                snapshot.terminals.insert(
                    entity.clone(),
                    Arc::new(json!({"tabs": self.shell_tabs_for(&scope)})),
                );
            }
        }
    }

    /// Only stored identities and observed paths participate. In particular,
    /// this does not call checkout_subjects, whose initial capture probes Git.
    fn collect_live_terminal_aliases(
        &self,
        scopes: &mut BTreeMap<String, TerminalScope>,
        root: &Path,
    ) {
        for (entity, active) in &self.runs {
            if active.run.state != crate::run::RunState::Archived {
                self.insert_terminal_alias(scopes, root, entity, &active.worktree.path, None);
            }
        }
        for workspace in self.workspaces.list(None) {
            self.insert_terminal_alias(scopes, root, &workspace.id, &workspace.root, None);
        }
        for project in self.projects.iter() {
            self.insert_terminal_alias(scopes, root, &project.id, &project.repo_path, None);
            for source in &project.sources {
                let source_path = source.path.display().to_string();
                let entity =
                    crate::changes::project_source_entity_id(&project.id, &source.id, &source_path);
                let scope = crate::changes::ProjectSourceScope::new(
                    project.id.clone(),
                    source.id.clone(),
                    source_path,
                );
                self.insert_terminal_alias(scopes, root, &entity, &source.path, Some(scope));
            }
            if let Some(scan) = self.board.diff().external_scan_cache(&project.id) {
                for checkout in &scan.worktrees {
                    self.insert_terminal_alias(scopes, root, &checkout.id, &checkout.path, None);
                }
            }
        }
    }

    fn insert_terminal_alias(
        &self,
        scopes: &mut BTreeMap<String, TerminalScope>,
        root: &Path,
        entity: &str,
        candidate: &Path,
        source: Option<crate::changes::ProjectSourceScope>,
    ) {
        if self.cached_snapshot_root(candidate) == root {
            scopes.insert(
                entity.to_owned(),
                TerminalScope {
                    root: root.to_owned(),
                    source,
                },
            );
        }
    }
}

fn same_terminal_entity(previous: Option<&Arc<()>>, current: Option<&Arc<()>>) -> bool {
    match (previous, current) {
        (Some(previous), Some(current)) => Arc::ptr_eq(previous, current),
        (None, None) => true,
        _ => false,
    }
}

//! Indexed task implementation homes. Owner changes never scan unrelated runs.
use crate::app::{conversations::AttachmentHomes, AppState};
use im::OrdMap;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

pub(super) enum HomeInput {
    Plan(Option<PathBuf>),
    Run {
        checkout: PathBuf,
        plan: Option<String>,
        created: String,
        terminal: bool,
    },
}

impl HomeInput {
    pub(super) fn capture(app: &AppState, id: &str) -> Option<Arc<Self>> {
        if let Some(run) = app.runs.get(id) {
            return Some(Arc::new(Self::Run {
                checkout: run.worktree.path.clone(),
                plan: run.run.plan_id.as_ref().map(|id| id.0.clone()),
                created: app
                    .board
                    .attention()
                    .clock(id)
                    .created_at
                    .unwrap_or_default(),
                terminal: run.run.state.is_terminal(),
            }));
        }
        app.plans.get(id).map(|plan| {
            Arc::new(Self::Plan(
                plan.workspace
                    .as_ref()
                    .map(|workspace| workspace.checkout.clone()),
            ))
        })
    }
    fn lineage(&self) -> Option<(String, String)> {
        match self {
            Self::Run {
                plan: Some(plan),
                created,
                ..
            } => Some((plan.clone(), created.clone())),
            _ => None,
        }
    }
}

#[derive(Clone, Default)]
pub(super) struct Homes {
    rows: OrdMap<String, Arc<HomeInput>>,
    lineages: OrdMap<String, Arc<Vec<String>>>,
    order: Arc<BTreeMap<String, usize>>,
}

impl Homes {
    pub(super) fn capture(app: &AppState) -> Self {
        let mut homes = Self::default();
        for owner in app.runs.keys().chain(app.plans.keys()) {
            #[cfg(test)]
            if app.runs.contains_key(owner) {
                AppState::note_implementation_lookup_visit();
            }
            homes.owner(owner, &HomeInput::capture(app, owner));
        }
        homes.topology(app.runs.keys().cloned().collect());
        homes
    }
    pub(super) fn contains(&self, id: &str) -> bool {
        self.rows.contains_key(id)
    }
    pub(super) fn owner(&mut self, id: &str, row: &Option<Arc<HomeInput>>) {
        let old = self.rows.get(id).and_then(|row| row.lineage());
        let new = row.as_ref().and_then(|row| row.lineage());
        if old != new {
            if let Some((plan, _)) = &old {
                self.change_lineage(plan, id, false);
            }
            if let Some((plan, _)) = &new {
                self.change_lineage(plan, id, true);
            }
        }
        match row {
            Some(row) => {
                self.rows.insert(id.into(), row.clone());
            }
            None => {
                self.rows.remove(id);
            }
        }
        if old != new {
            if let Some((plan, _)) = new {
                self.sort(&plan);
            }
        }
    }
    fn change_lineage(&mut self, plan: &str, id: &str, add: bool) {
        let mut ids = self
            .lineages
            .get(plan)
            .map(|ids| ids.as_ref().clone())
            .unwrap_or_default();
        ids.retain(|candidate| candidate != id);
        if add {
            ids.push(id.into());
        }
        self.lineages.insert(plan.into(), Arc::new(ids));
    }
    fn sort(&mut self, plan: &str) {
        let mut ids = self
            .lineages
            .get(plan)
            .map(|ids| ids.as_ref().clone())
            .unwrap_or_default();
        ids.sort_by_key(|id| {
            (
                self.rows[id]
                    .lineage()
                    .map(|(_, created)| created)
                    .unwrap_or_default(),
                self.order.get(id).copied().unwrap_or_default(),
            )
        });
        self.lineages.insert(plan.into(), Arc::new(ids));
    }
    pub(super) fn topology(&mut self, order: Vec<String>) {
        self.order = Arc::new(
            order
                .into_iter()
                .enumerate()
                .map(|(rank, id)| (id, rank))
                .collect(),
        );
        let plans = self.lineages.keys().cloned().collect::<Vec<_>>();
        for plan in plans {
            self.sort(&plan);
        }
    }
    pub(super) fn get(&self, id: &str, local: &std::path::Path) -> Option<AttachmentHomes> {
        let root = match self.rows.get(id)?.as_ref() {
            HomeInput::Run { checkout, .. } => Some(checkout.clone()),
            HomeInput::Plan(fallback) => self
                .lineages
                .get(id)
                .and_then(|ids| {
                    ids.iter()
                        .rev()
                        .find_map(|id| match self.rows.get(id)?.as_ref() {
                            HomeInput::Run {
                                checkout,
                                terminal: false,
                                ..
                            } => Some(checkout.clone()),
                            _ => None,
                        })
                })
                .or_else(|| fallback.clone()),
        };
        Some(AttachmentHomes::from_paths(
            root.map(|root| root.join(".build/attachments")),
            local.to_path_buf(),
        ))
    }
}

//! Environment-owned source references shared by project lines of work.
use crate::app::projects::ProjectSource;
use crate::isolation::Isolation;

pub(in crate::app) mod migration;
#[cfg(test)]
mod config_migration_tests;
#[cfg(test)]
mod runtime_tests;

#[derive(Clone, Debug)]
pub(in crate::app) struct Environment {
    pub(in crate::app) id: String,
    pub(in crate::app) name: String,
    pub(in crate::app) sources: Vec<ProjectSource>,
    pub(in crate::app) next_source: u64,
    pub(in crate::app) isolation: Option<Isolation>,
}

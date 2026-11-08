//! Environment-owned source references shared by project lines of work.
//!
//! Downgrade boundary: legacy `path`, `base_branch`, `sources` and `next_source`
//! remain in every project record. Before any sibling shares a home, the old
//! loader sees the same projects. With a sibling, it collapses entries by path
//! into the first project and applies the later source/isolation settings to
//! that project; opaque sibling keys are ignored. Roll forward after that point.
//! `frozen_legacy_loader` pins this behavior without depending on today's loader.
//!
//! Migration keeps the project isolation copy for legacy/downgrade compatibility
//! and copies it into the environment for future environment consumers. M0.1
//! mirrors setting/clearing both copies while environments are one-to-one, so
//! clearing still restores the device default. Parked projects migrate at once;
//! their raw source references are retained even while their folders are away.
use crate::app::projects::ProjectSource;
use crate::isolation::Isolation;

#[cfg(test)]
mod config_migration_tests;
pub(in crate::app) mod migration;
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

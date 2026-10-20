//! Catalog projection belongs to the probe producer, before its change watch.

use super::{executable, CliReading, Entry};
use crate::harness::harness_for;
use crate::models::{AgentProvider, ProviderCatalog};
use std::collections::HashMap;

pub(super) fn initial() -> Vec<ProviderCatalog> {
    AgentProvider::ALL
        .into_iter()
        .map(|provider| {
            let installed = executable::identify(harness_for(provider).binary()).is_some();
            row(provider, None, installed)
        })
        .collect()
}

pub(super) fn observed(
    entries: &HashMap<&'static str, Entry>,
    previous: &[ProviderCatalog],
) -> Vec<ProviderCatalog> {
    previous
        .iter()
        .map(|prior| {
            let entry = entries
                .get(prior.binary)
                .filter(|entry| entry.read_at.is_some());
            row(
                prior.id,
                entry.and_then(|entry| entry.reading.as_deref()),
                entry.map_or(prior.installed, |entry| {
                    entry.attempted_executable.is_some()
                }),
            )
        })
        .collect()
}

fn row(provider: AgentProvider, reading: Option<&CliReading>, installed: bool) -> ProviderCatalog {
    let harness = harness_for(provider);
    let offer = harness.offer(reading);
    ProviderCatalog {
        id: provider,
        label: harness.label(),
        models: offer.models,
        cli_name: harness.cli_name(),
        cli_version: offer.cli_version.map(|version| version.to_string()),
        unavailable: offer.unavailable,
        efforts: harness.effort_levels(),
        binary: harness.binary(),
        installed,
    }
}

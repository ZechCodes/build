//! The sole insertion point for producer-owned source remote observations.
use super::*;
use crate::app::projects::ProjectSource;
use std::path::Path;

type SourceInput = (String, String, bool);

fn source_input(row: &Value) -> SourceInput {
    (
        row["id"].as_str().unwrap_or_default().into(),
        row["path"].as_str().unwrap_or_default().into(),
        row["is_git"].as_bool().unwrap_or(false),
    )
}

impl ProjectObservation {
    pub(super) fn new(version: u64, value: Value) -> Self {
        let source_versions = value["sources"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|row| (source_input(row), version))
            .collect();
        Self {
            version,
            value,
            source_versions,
        }
    }
    pub(super) fn source_version(&self, row: &Value) -> u64 {
        self.source_versions
            .get(&source_input(row))
            .copied()
            .unwrap_or(self.version)
    }
}

/// No discovery or IO is permitted here. An observation may describe prospective
/// configuration: accept only source inputs still present in the current owner,
/// and compare versions independently so a changed source cannot age its siblings.
pub(super) fn publish_source_observation(
    observations: &mut BTreeMap<String, ProjectObservation>,
    id: &str,
    incoming: ProjectObservation,
    sources: &[ProjectSource],
    home: &Path,
    availability_matches: bool,
) {
    let previous = observations.get(id);
    let mut versions = BTreeMap::new();
    let rows = sources
        .iter()
        .filter_map(|source| {
            let key = (
                source.id.clone(),
                source.path.display().to_string(),
                source.is_git,
            );
            let selected = [Some(&incoming), previous]
                .into_iter()
                .flatten()
                .filter_map(|observation| {
                    let row = observation.value["sources"]
                        .as_array()?
                        .iter()
                        .find(|row| source_input(row) == key)?;
                    Some((observation.source_version(row), row))
                })
                .max_by_key(|(version, _)| *version)?;
            versions.insert(key, selected.0);
            Some(selected.1.clone())
        })
        .collect::<Vec<_>>();
    let available = [Some(&incoming).filter(|_| availability_matches), previous]
        .into_iter()
        .flatten()
        .filter(|observation| observation.value["path"] == home.display().to_string())
        .max_by_key(|observation| observation.version);
    let isolation_available = available
        .map(|o| o.value["isolation_available"].clone())
        .unwrap_or_else(|| {
            json!(crate::isolation::IsolationAvailability::unavailable(
                "repository observation pending"
            ))
        });
    let value = json!({"path":home,"remote":crate::app::projects::primary_remote(&rows),"sources":rows,"isolation_available":isolation_available});
    let version = available.map(|o| o.version).unwrap_or_default();
    observations.insert(
        id.to_owned(),
        ProjectObservation {
            version,
            value,
            source_versions: versions,
        },
    );
}

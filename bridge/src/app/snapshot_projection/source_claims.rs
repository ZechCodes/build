//! Read-only claim evidence uses the same off-lock renderer and committed facts.
use super::*;
use crate::app::projects::ProjectSource;

impl ProjectSnapshotRefresh {
    pub(in crate::app) fn include_source_claims(mut self, sources: Vec<ProjectSource>) -> Self {
        self.claims = sources
            .into_iter()
            .chain(self.rows.iter().flat_map(|row| row.sources.iter().cloned()))
            .chain(
                self.environments
                    .iter()
                    .flat_map(|row| row.sources.iter().cloned()),
            )
            .collect();
        self.claims
            .sort_by_key(|s| (s.id.clone(), s.path.clone(), s.is_git));
        self.claims
            .dedup_by(|a, b| a.id == b.id && a.path == b.path && a.is_git == b.is_git);
        self
    }
    pub(in crate::app) fn for_source_claims(self, sources: Vec<ProjectSource>) -> Self {
        let mut refresh = self.include_source_claims(sources);
        refresh.rows.clear();
        refresh.environments.clear();
        refresh
    }
    pub(in crate::app) fn source_claim_paths(&self) -> Vec<PathBuf> {
        self.claims.iter().map(|s| s.path.clone()).collect()
    }
}

impl RenderedProjectSnapshotRefresh {
    pub(in crate::app) fn validate_source_claim_observations(&self) -> Result<(), String> {
        match self
            .claims
            .iter()
            .find_map(|(_, value)| value.as_ref().err())
        {
            Some(error) => Err(error.clone()),
            None => Ok(()),
        }
    }
    pub(in crate::app) fn source_claim_inputs(&self) -> impl Iterator<Item = &ProjectSource> {
        self.claims.iter().map(|(source, _)| source)
    }
}

fn matches(source: &ProjectSource, row: &Value) -> bool {
    row["id"] == source.id
        && row["path"] == source.path.display().to_string()
        && row["is_git"] == source.is_git
}

impl AppState {
    /// Prefer newer committed evidence without probing. Missing capture inputs
    /// are a stale claim, never evidence that a remote is absent.
    pub(in crate::app) fn source_claim_remote(
        &self,
        captured: &RenderedProjectSnapshotRefresh,
        source: &ProjectSource,
    ) -> Result<Option<String>, String> {
        let (_, row) = captured
            .claims
            .iter()
            .find(|(input, _)| {
                input.id == source.id && input.path == source.path && input.is_git == source.is_git
            })
            .ok_or("stale unversioned mutation: source claim inputs changed; retry the edit")?;
        let row = row.as_ref().map_err(Clone::clone)?;
        let mut selected = (captured.version, row["remote"].clone());
        let projects = self.snapshot_observations.projects.lock().unwrap();
        let environments = self.snapshot_observations.environments.lock().unwrap();
        for observation in projects.values().chain(environments.values()) {
            if let Some(row) = observation.value["sources"]
                .as_array()
                .and_then(|rows| rows.iter().find(|row| matches(source, row)))
            {
                let version = observation.source_version(row);
                if version > selected.0 {
                    selected = (version, row["remote"].clone());
                }
            }
        }
        Ok(selected.1.as_str().map(str::to_owned))
    }
}

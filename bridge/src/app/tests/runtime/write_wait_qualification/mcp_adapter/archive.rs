//! Explicit drain-time import of real registry timestamps, never per read.
use super::*;
use crate::timing::write_wait::WriteCohort;

impl McpHarness {
    pub fn archive_registry_receipts(&self) -> McpRegistryArchive {
        let rows = self.inner.registry.snapshot();
        let mut archive = McpRegistryArchive {
            collected_at: Instant::now(),
            rows,
            unimported: Vec::new(),
        };
        self.inner.jobs.lock().unwrap().evidence.registry_archive = Some(archive.clone());
        for row in &archive.rows {
            if let Some(reason) = self.unimported_reason(row) {
                archive.unimported.push(UnimportedOrigin {
                    row: row.clone(),
                    reason,
                });
                continue;
            }
            if let Err(error) = self.import_origin(row) {
                self.inner.identity_error(&row.original_id, error.clone());
                archive.unimported.push(UnimportedOrigin {
                    row: row.clone(),
                    reason: error,
                });
            }
        }
        self.inner.jobs.lock().unwrap().evidence.registry_archive = Some(archive.clone());
        archive
    }

    fn unimported_reason(&self, row: &WriteSnapshot) -> Option<String> {
        let Some(schedule) = self.inner.watch.schedule_lookup(&row.original_id) else {
            return Some("not a scheduled client identity".into());
        };
        if schedule.intended_at.is_none() {
            return Some("relative schedule not released".into());
        }
        let jobs = self.inner.jobs.lock().unwrap();
        let Some(call) = jobs.evidence.calls.get(&row.original_id) else {
            return Some("another adapter/background/provisional origin".into());
        };
        if call.read
            || schedule.cohort != TrafficCohort::SemanticWrite
            || row.cohort != WriteCohort::SemanticWrite
        {
            return Some("not this adapter's semantic-write origin".into());
        }
        None
    }

    fn import_origin(&self, row: &WriteSnapshot) -> Result<(), String> {
        let previous = self
            .inner
            .jobs
            .lock()
            .unwrap()
            .imported
            .get(&row.original_id)
            .cloned();
        if let Some(previous) = &previous {
            if previous.id != row.id || previous.intake != row.intake {
                return Err("multiple/conflicting actual MCP server origins".into());
            }
            if previous.admitted_at.is_some() && previous.admitted_at != row.admitted_at {
                return Err("actual MCP admission timestamp changed".into());
            }
        } else {
            self.inner
                .watch
                .server_registered_at(&row.original_id, row.intake);
        }
        if previous
            .as_ref()
            .and_then(|previous| previous.admitted_at)
            .is_none()
        {
            if let Some(admitted) = row.admitted_at {
                self.inner.watch.admitted_at(&row.original_id, admitted);
            }
        }
        self.inner.jobs.lock().unwrap().imported.insert(
            row.original_id.clone(),
            ImportedOrigin {
                id: row.id,
                intake: row.intake,
                admitted_at: row.admitted_at,
            },
        );
        Ok(())
    }
}

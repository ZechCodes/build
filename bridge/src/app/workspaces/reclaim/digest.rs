//! The daily idle-workspace digest (#958).
//!
//! Each project agent hears about its quiet workspaces once a day: one
//! digest listing every workspace at or past the idle threshold, the ones it
//! heard about on earlier days included. A workspace going quiet sends
//! nothing by itself, and a day with nothing quiet sends nothing at all. The
//! digest goes out at the first sweep at or after the scan time, so a bridge
//! that was down then sends it at its first sweep after starting. The day
//! each project was sent is kept in the store, so a restart never sends it
//! twice.

use super::{notice, tasks_of, Quiet};
use crate::agent_message::MessageKind;
use crate::app::AppState;
use crate::reclaim::ReclaimPolicy;
use crate::tracker::TaskEventKind;
use jiff::civil::Date;
use serde_json::json;
use std::collections::BTreeMap;

impl AppState {
    /// Send today's digest to every project that has not had it, once the
    /// scan time has come. A project with nothing quiet has had it. A digest
    /// that failed to land is tried again next sweep.
    pub(super) fn send_idle_digests(&mut self, now_ms: i64, policy: &ReclaimPolicy) {
        let Some(day) = policy.digest.due_day(now_ms) else {
            return;
        };
        let owed: Vec<String> = self
            .projects
            .iter()
            .filter(|project| project.archived_at.is_none())
            .map(|project| project.id.clone())
            .filter(|project_id| {
                self.idle_digest_sent
                    .get(project_id)
                    .is_none_or(|sent| owed_after(sent, day))
            })
            .collect();
        if owed.is_empty() {
            return;
        }
        let mut quiet = self.quiet_workspaces_by_project();
        for project_id in owed {
            let workspaces = quiet.remove(&project_id).unwrap_or_default();
            if workspaces.is_empty()
                || self.send_idle_digest(&project_id, &workspaces, now_ms, policy)
            {
                self.idle_digest_sent.insert(project_id, day.to_string());
            }
        }
        let projects = &self.projects;
        self.idle_digest_sent
            .retain(|project_id, _| projects.iter().any(|project| &project.id == project_id));
        self.persist_idle_digests_sent();
    }

    /// Every quiet workspace the service measures, by project, as its last
    /// verdict stands.
    fn quiet_workspaces_by_project(&self) -> BTreeMap<String, Vec<Quiet>> {
        let linked = self.tasks_linking_workspaces();
        let mut quiet: BTreeMap<String, Vec<Quiet>> = BTreeMap::new();
        for workspace in self
            .measured_workspaces(None)
            .filter(|workspace| self.refuse_archived_project(&workspace.project_id).is_ok())
        {
            let Some(record) = self
                .workspace_lifecycle
                .get(&workspace.id)
                .filter(|record| record.idle)
            else {
                continue;
            };
            quiet
                .entry(workspace.project_id.clone())
                .or_default()
                .push(Quiet {
                    subject: self.reclaim_subject(workspace, tasks_of(&linked, &workspace.id)),
                    record: record.clone(),
                    first: !record.listed_since_quiet(),
                });
        }
        quiet
    }

    /// One digest for the project agent naming every quiet workspace in its
    /// project. Only a digest that landed counts as sent.
    fn send_idle_digest(
        &mut self,
        project_id: &str,
        workspaces: &[Quiet],
        now_ms: i64,
        policy: &ReclaimPolicy,
    ) -> bool {
        let body = notice::quiet_workspaces(workspaces, policy);
        if let Err(error) = self.tell_project_agent(project_id, body) {
            eprintln!(
                "workspace reclaim: tell project {project_id} about quiet workspaces: {error}"
            );
            return false;
        }
        for quiet in workspaces {
            // A workspace is recorded on its tasks the first time a digest
            // that lists it lands, not every day after.
            if quiet.first {
                self.note_on_linked_tasks(
                    &quiet.subject,
                    TaskEventKind::WorkspaceIdle,
                    &quiet.record,
                    policy,
                );
            }
            if let Some(record) = self
                .workspace_lifecycle
                .get_mut(&quiet.subject.workspace_id)
            {
                record.noticed_at_ms = Some(now_ms);
            }
        }
        true
    }

    fn tell_project_agent(&mut self, project_id: &str, body: String) -> Result<(), String> {
        let conversation =
            self.project_ensure_conversation(&json!({ "project_id": project_id }))?;
        let entity_id = conversation["run_id"]
            .as_str()
            .ok_or("the project conversation has no owner")?
            .to_string();
        let agent_id = self.ensure_primary_agent(&entity_id)?;
        self.post_from_build_and_wake_as(
            &entity_id,
            &agent_id,
            (notice::PHASE, Some(MessageKind::IdleWorkspaces)),
            |thread, now| {
                thread.post_user_from_build(body, now);
            },
        )
    }

    pub(in crate::app) fn restore_idle_digests_sent(&mut self) -> Result<(), String> {
        if let Some(store) = self.store.as_ref() {
            self.idle_digest_sent = store
                .load_idle_digests_sent()
                .map_err(|error| format!("idle digest restore: {error}"))?;
        }
        Ok(())
    }

    fn persist_idle_digests_sent(&self) {
        if let Some(store) = self.store.as_ref() {
            if let Err(error) = store.save_idle_digests_sent(&self.idle_digest_sent) {
                eprintln!("workspace reclaim: persist idle digest days: {error}");
            }
        }
    }
}

/// Whether a project whose digest last went out for `sent` is owed `day`'s.
/// A day already had is not owed again, nor one a zone ahead has had. A day
/// more than two days ahead was written by a clock that was wrong, and is
/// forgotten, so correcting the clock does not silence the digest until the
/// date catches up.
fn owed_after(sent: &str, day: Date) -> bool {
    sent.parse::<Date>()
        .ok()
        .and_then(|sent| day.until(sent).ok())
        .is_none_or(|ahead| !(0..=2).contains(&ahead.get_days()))
}

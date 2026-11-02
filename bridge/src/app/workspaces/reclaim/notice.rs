//! What the project agent is told about its quiet workspaces.

use super::Quiet;
use crate::reclaim::{LifecycleRecord, LinkedTask, ReclaimPolicy, Subject};
use crate::templates::{
    fill, MESSAGE_WORKSPACES_QUIET, MESSAGE_WORKSPACES_QUIET_MANY, MESSAGE_WORKSPACES_QUIET_ONE,
    MESSAGE_WORKSPACE_QUIET, MESSAGE_WORKSPACE_QUIET_HELD, MESSAGE_WORKSPACE_QUIET_LOCKED,
    MESSAGE_WORKSPACE_QUIET_NO_TASK, MESSAGE_WORKSPACE_QUIET_PRUNED,
    MESSAGE_WORKSPACE_QUIET_RECLAIMABLE, MESSAGE_WORKSPACE_QUIET_TASK,
};

/// The phase a quiet-workspace notice's turn is queued under.
pub(super) const PHASE: &str = "workspace_idle";

/// What the project agent reads about its quiet workspaces: one line per
/// workspace with everything a decision needs, then how to handle it and when
/// the user needs to hear about it.
pub(super) fn quiet_workspaces(workspaces: &[Quiet], policy: &ReclaimPolicy) -> String {
    let hours = (policy.idle_after.as_secs() / 3600).to_string();
    let count = workspaces.len().to_string();
    let these = if workspaces.len() == 1 {
        MESSAGE_WORKSPACES_QUIET_ONE
    } else {
        MESSAGE_WORKSPACES_QUIET_MANY
    };
    let quiet = fill(these, &[("count", count.as_str())]);
    let lines: Vec<String> = workspaces
        .iter()
        .map(|quiet| quiet_workspace_line(&quiet.subject, &quiet.record))
        .collect();
    fill(
        MESSAGE_WORKSPACES_QUIET,
        &[
            ("quiet", quiet.as_str()),
            ("hours", hours.as_str()),
            ("workspaces", lines.join("\n").as_str()),
        ],
    )
}

fn quiet_workspace_line(subject: &Subject, record: &LifecycleRecord) -> String {
    let branches: Vec<&str> = subject
        .repositories
        .iter()
        .filter_map(|(_, branch)| branch.as_deref())
        .collect();
    let branches = if branches.is_empty() {
        "none".to_string()
    } else {
        branches.join(", ")
    };
    let size = record
        .size_bytes
        .map(crate::reclaim::human_bytes)
        .unwrap_or_else(|| "unmeasured".to_string());
    let last_activity = record
        .last_activity_ms
        .and_then(rfc3339_of_ms)
        .unwrap_or_else(|| "unknown".to_string());
    let counts = [
        record.dirty_files.to_string(),
        record.unpushed_commits.to_string(),
        record.behind_commits.to_string(),
    ];
    fill(
        MESSAGE_WORKSPACE_QUIET,
        &[
            ("name", subject.name.as_str()),
            ("workspace_id", subject.workspace_id.as_str()),
            ("tasks", linked_tasks(&subject.tasks).as_str()),
            ("branches", branches.as_str()),
            ("dirty_files", counts[0].as_str()),
            ("unpushed_commits", counts[1].as_str()),
            ("behind_commits", counts[2].as_str()),
            ("size", size.as_str()),
            ("pruned", pruned(record).as_str()),
            ("last_activity", last_activity.as_str()),
            ("verdict", verdict(record).as_str()),
            ("locked", locked_note(record)),
        ],
    )
}

fn linked_tasks(tasks: &[LinkedTask]) -> String {
    if tasks.is_empty() {
        return MESSAGE_WORKSPACE_QUIET_NO_TASK.to_string();
    }
    tasks
        .iter()
        .map(|task| {
            let number = task.number.to_string();
            let column = task_column(task);
            fill(
                MESSAGE_WORKSPACE_QUIET_TASK,
                &[
                    ("number", number.as_str()),
                    ("title", task.title.as_str()),
                    ("column", column.as_str()),
                ],
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn pruned(record: &LifecycleRecord) -> String {
    if record.pruned_bytes == 0 {
        return String::new();
    }
    let size = crate::reclaim::human_bytes(record.pruned_bytes);
    fill(MESSAGE_WORKSPACE_QUIET_PRUNED, &[("size", size.as_str())])
}

fn verdict(record: &LifecycleRecord) -> String {
    if record.reclaimable {
        return MESSAGE_WORKSPACE_QUIET_RECLAIMABLE.to_string();
    }
    let holds = record
        .holds
        .iter()
        .map(|hold| crate::reclaim::hold_sentence(hold))
        .collect::<Vec<_>>()
        .join("; ");
    fill(MESSAGE_WORKSPACE_QUIET_HELD, &[("holds", holds.as_str())])
}

fn locked_note(record: &LifecycleRecord) -> &'static str {
    if record
        .holds
        .iter()
        .any(|hold| hold == crate::reclaim::HOLD_LOCKED)
    {
        MESSAGE_WORKSPACE_QUIET_LOCKED
    } else {
        ""
    }
}

fn rfc3339_of_ms(ms: i64) -> Option<String> {
    let at = time::OffsetDateTime::from_unix_timestamp(ms.div_euclid(1000)).ok()?;
    at.format(&time::format_description::well_known::Rfc3339)
        .ok()
}

fn task_column(task: &LinkedTask) -> String {
    if task.state == "closed" {
        return "closed".to_string();
    }
    crate::tracker::COLUMNS
        .iter()
        .find(|column| column.id == task.status)
        .map(|column| column.name.to_string())
        .unwrap_or_else(|| task.status.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::Duration;

    fn quiet(
        name: &str,
        tasks: Vec<LinkedTask>,
        branch: Option<&str>,
        record: LifecycleRecord,
    ) -> Quiet {
        Quiet {
            subject: Subject {
                workspace_id: format!("ws-{name}"),
                project_id: "project-1".into(),
                name: name.into(),
                root: PathBuf::from("/workspaces").join(name),
                boundary: None,
                repositories: branch
                    .map(|branch| vec![(PathBuf::from("/workspaces/repo"), Some(branch.into()))])
                    .unwrap_or_default(),
                holds: Vec::new(),
                tasks,
                conversation_activity_ms: None,
                previous: None,
            },
            record,
            first: true,
        }
    }

    /// Every word of the notice: a reclaimable workspace with its task and
    /// branch, and a locked one with none and nothing measured.
    #[test]
    fn the_quiet_workspaces_notice_is_said_exactly() {
        let finished = quiet(
            "alpha",
            vec![LinkedTask {
                task_id: "task-1".into(),
                number: 1,
                title: "Quiet work".into(),
                status: "done".into(),
                state: "open".into(),
            }],
            Some("task/1"),
            LifecycleRecord {
                last_activity_ms: Some(1_758_340_800_000),
                idle: true,
                reclaimable: true,
                behind_commits: 2,
                size_bytes: Some(1_500_000),
                ..LifecycleRecord::default()
            },
        );
        let locked = quiet(
            "beta",
            Vec::new(),
            None,
            LifecycleRecord {
                idle: true,
                holds: vec![crate::reclaim::HOLD_LOCKED.into()],
                dirty_files: 3,
                unpushed_commits: 1,
                pruned_bytes: 2_000,
                ..LifecycleRecord::default()
            },
        );
        let policy = ReclaimPolicy {
            idle_after: Duration::from_secs(24 * 3600),
            ..ReclaimPolicy::default()
        };
        assert_eq!(
            quiet_workspaces(&[finished, locked], &policy),
            "2 workspaces have had no activity for 24 hours. This message is from Build, not \
             from the user.\n\n\
             - alpha (ws-alpha): #1 Quiet work (Done). Branch task/1; 0 uncommitted files, 0 \
             unpushed commits, 2 behind. 1.5 MB on disk. Last activity 2025-09-20T04:00:00Z. \
             reclaimable: reclaim_workspace removes it.\n\
             - beta (ws-beta): no linked task. Branch none; 3 uncommitted files, 1 unpushed \
             commits, 0 behind. unmeasured on disk, 2.0 KB of build output dropped. Last \
             activity unknown. held: the workspace is locked. The user locked this workspace on \
             purpose; evaluate whether the user should be reminded about it. Do not reclaim it \
             or ask the user to unlock it by default.\n\n\
             Only message the user if necessary, such as a decision only they can make. \
             Otherwise merge, reclaim, or leave the workspace without posting to the user. If a \
             user decision is necessary, ask one question on the linked task and leave the \
             workspace alone.\n\n\
             For each unlocked workspace, decide: merge it (branch reviewed and green: merge to \
             main, then reclaim_workspace), delete it (abandoned or superseded: \
             delete_workspace), or leave it. Never delete a workspace with uncommitted or \
             unpushed work without asking the user first."
        );
    }
}

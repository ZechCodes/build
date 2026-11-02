//! What the project agent is told, once a day, about its quiet workspaces
//! (#958).

use super::Quiet;
use crate::reclaim::{LifecycleRecord, LinkedTask, ReclaimPolicy, Subject, HOLD_LOCKED};
use crate::templates::{
    fill, MESSAGE_WORKSPACES_QUIET, MESSAGE_WORKSPACES_QUIET_DECIDE,
    MESSAGE_WORKSPACES_QUIET_LOCKED, MESSAGE_WORKSPACES_QUIET_MANY, MESSAGE_WORKSPACES_QUIET_ONE,
    MESSAGE_WORKSPACE_QUIET, MESSAGE_WORKSPACE_QUIET_HELD, MESSAGE_WORKSPACE_QUIET_LOCKED,
    MESSAGE_WORKSPACE_QUIET_LOCKED_HELD, MESSAGE_WORKSPACE_QUIET_NO_TASK,
    MESSAGE_WORKSPACE_QUIET_PRUNED, MESSAGE_WORKSPACE_QUIET_RECLAIMABLE,
    MESSAGE_WORKSPACE_QUIET_TASK,
};

/// The phase a quiet-workspace digest's turn is queued under.
pub(super) const PHASE: &str = "workspace_idle";

/// The project agent's daily digest: one line per quiet workspace with
/// everything a decision needs, the ones the user locked in their own group
/// with what else holds them, then when the user needs to hear about it and,
/// if any is not locked, how to decide about those.
pub(super) fn quiet_workspaces(workspaces: &[Quiet], policy: &ReclaimPolicy) -> String {
    let hours = (policy.idle_after.as_secs() / 3600).to_string();
    let count = workspaces.len().to_string();
    let these = if workspaces.len() == 1 {
        MESSAGE_WORKSPACES_QUIET_ONE
    } else {
        MESSAGE_WORKSPACES_QUIET_MANY
    };
    let quiet = fill(these, &[("count", count.as_str())]);
    let (locked, unlocked): (Vec<&Quiet>, Vec<&Quiet>) = workspaces
        .iter()
        .partition(|quiet| is_locked(&quiet.record));
    let mut groups = Vec::new();
    if !unlocked.is_empty() {
        groups.push(lines(&unlocked));
    }
    if !locked.is_empty() {
        groups.push(format!(
            "{MESSAGE_WORKSPACES_QUIET_LOCKED}\n{}",
            lines(&locked)
        ));
    }
    let digest = fill(
        MESSAGE_WORKSPACES_QUIET,
        &[
            ("quiet", quiet.as_str()),
            ("hours", hours.as_str()),
            ("workspaces", groups.join("\n\n").as_str()),
        ],
    );
    if unlocked.is_empty() {
        digest
    } else {
        format!("{digest}\n\n{MESSAGE_WORKSPACES_QUIET_DECIDE}")
    }
}

fn lines(workspaces: &[&Quiet]) -> String {
    workspaces
        .iter()
        .map(|quiet| quiet_workspace_line(&quiet.subject, &quiet.record))
        .collect::<Vec<_>>()
        .join("\n")
}

fn is_locked(record: &LifecycleRecord) -> bool {
    record.holds.iter().any(|hold| hold == HOLD_LOCKED)
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
    let others: Vec<&str> = record
        .holds
        .iter()
        .filter(|hold| *hold != HOLD_LOCKED)
        .map(|hold| crate::reclaim::hold_sentence(hold))
        .collect();
    let holds = others.join("; ");
    let template = match (is_locked(record), others.is_empty()) {
        (true, true) => MESSAGE_WORKSPACE_QUIET_LOCKED,
        (true, false) => MESSAGE_WORKSPACE_QUIET_LOCKED_HELD,
        (false, _) if record.reclaimable => MESSAGE_WORKSPACE_QUIET_RECLAIMABLE,
        (false, _) => MESSAGE_WORKSPACE_QUIET_HELD,
    };
    fill(template, &[("holds", holds.as_str())])
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

    /// Every word of the digest: a reclaimable workspace with its task and
    /// branch, then, in their own group, a locked one with unpushed work and
    /// nothing measured, and one only the lock holds.
    #[test]
    fn the_quiet_workspaces_digest_is_said_exactly() {
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
                holds: vec![
                    crate::reclaim::HOLD_LOCKED.into(),
                    crate::workspace::FINISH_BLOCKER_UNPUSHED.into(),
                ],
                dirty_files: 3,
                unpushed_commits: 1,
                pruned_bytes: 2_000,
                ..LifecycleRecord::default()
            },
        );
        let only_locked = quiet(
            "gamma",
            Vec::new(),
            Some("task/3"),
            LifecycleRecord {
                idle: true,
                holds: vec![crate::reclaim::HOLD_LOCKED.into()],
                size_bytes: Some(1_000),
                ..LifecycleRecord::default()
            },
        );
        let policy = ReclaimPolicy {
            idle_after: Duration::from_secs(24 * 3600),
            ..ReclaimPolicy::default()
        };
        assert_eq!(
            quiet_workspaces(&[locked, finished, only_locked], &policy),
            "3 workspaces have had no activity for 24 hours or more. This is Build's daily \
             list of every quiet workspace, including ones it listed on earlier days. This \
             message is from Build, not from the user.\n\n\
             - alpha (ws-alpha): #1 Quiet work (Done). Branch task/1; 0 uncommitted files, 0 \
             unpushed commits, 2 behind. 1.5 MB on disk. Last activity 2025-09-20T04:00:00Z. \
             reclaimable: reclaim_workspace removes it.\n\n\
             Locked by the user, so kept on purpose. Do not reclaim these or ask the user to \
             unlock them; evaluate whether the user should be reminded about one:\n\
             - beta (ws-beta): no linked task. Branch none; 3 uncommitted files, 1 unpushed \
             commits, 0 behind. unmeasured on disk, 2.0 KB of build output dropped. Last \
             activity unknown. kept because the user locked it; also held: it has commits no \
             remote has.\n\
             - gamma (ws-gamma): no linked task. Branch task/3; 0 uncommitted files, 0 \
             unpushed commits, 0 behind. 1.0 KB on disk. Last activity unknown. kept because \
             the user locked it.\n\n\
             Only message the user if necessary, such as a decision only they can make. \
             Otherwise handle or leave the workspace without posting to the user. If a \
             user decision is necessary, ask one question on the linked task and leave the \
             workspace alone.\n\n\
             For each workspace that is not locked, decide: merge it (branch reviewed and \
             green: merge to main, then reclaim_workspace), delete it (abandoned or \
             superseded: delete_workspace), or leave it. Never delete a workspace with \
             uncommitted or unpushed work without asking the user first."
        );
    }

    /// With nothing locked there is no locked group, and with only locked
    /// workspaces there is nothing above it and no word on deciding about
    /// the unlocked ones.
    #[test]
    fn the_locked_group_is_there_only_when_something_is_locked() {
        let open = quiet("open", Vec::new(), None, LifecycleRecord::default());
        let policy = ReclaimPolicy::default();
        assert!(!quiet_workspaces(&[open], &policy).contains("Locked by the user"));
        let locked = quiet(
            "shut",
            Vec::new(),
            None,
            LifecycleRecord {
                holds: vec![crate::reclaim::HOLD_LOCKED.into()],
                ..LifecycleRecord::default()
            },
        );
        let digest = quiet_workspaces(&[locked], &policy);
        assert!(
            digest.starts_with("1 workspace has had no activity"),
            "{digest}"
        );
        assert!(digest.contains(
            "earlier days. This message is from Build, not from the user.\n\nLocked by the user"
        ));
        assert!(
            digest.ends_with("ask one question on the linked task and leave the workspace alone."),
            "nothing to decide about when everything is locked: {digest}"
        );
    }

    /// A workspace name or task title shaped like a placeholder is said as
    /// written.
    #[test]
    fn placeholder_shaped_names_are_said_as_written() {
        let task = LinkedTask {
            task_id: "task-1".into(),
            number: 1,
            title: "{holds}".into(),
            status: "done".into(),
            state: "open".into(),
        };
        let named = quiet("{verdict}", vec![task], None, LifecycleRecord::default());
        assert!(quiet_workspaces(&[named], &ReclaimPolicy::default())
            .contains("- {verdict} (ws-{verdict}): #1 {holds} (Done). Branch none;"));
    }
}

//! What the project agent is told about its quiet workspaces.

use super::Quiet;
use crate::reclaim::{LifecycleRecord, LinkedTask, ReclaimPolicy, Subject};

/// The phase a quiet-workspace notice's turn is queued under.
pub(super) const PHASE: &str = "workspace_idle";

/// What the project agent reads about its quiet workspaces: one line per
/// workspace with everything a decision needs, then how to handle it and when
/// the user needs to hear about it.
pub(super) fn quiet_workspaces(workspaces: &[Quiet], policy: &ReclaimPolicy) -> String {
    let hours = policy.idle_after.as_secs() / 3600;
    let count = workspaces.len();
    let these = if count == 1 {
        "workspace has"
    } else {
        "workspaces have"
    };
    let lines: Vec<String> = workspaces
        .iter()
        .map(|quiet| quiet_workspace_line(&quiet.subject, &quiet.record))
        .collect();
    format!(
        "{count} {these} had no activity for {hours} hours. This message is from Build, not from \
         the user.\n\n{}\n\nOnly message the user if necessary, such as a decision only they can make. \
         Otherwise merge, reclaim, or leave the workspace without posting to the user. \
         If a user decision is necessary, ask one question on the linked task and leave the \
         workspace alone.\n\nFor each unlocked workspace, decide: merge it (branch reviewed and \
         green: merge to main, then reclaim_workspace), delete it (abandoned or superseded: \
         delete_workspace), or leave it. Never delete a workspace with uncommitted or unpushed \
         work without asking the user first.",
        lines.join("\n")
    )
}

fn quiet_workspace_line(subject: &Subject, record: &LifecycleRecord) -> String {
    let tasks = if subject.tasks.is_empty() {
        "no linked task".to_string()
    } else {
        subject
            .tasks
            .iter()
            .map(|task| format!("#{} {} ({})", task.number, task.title, task_column(task)))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let branches: Vec<&str> = subject
        .repositories
        .iter()
        .filter_map(|(_, branch)| branch.as_deref())
        .collect();
    let verdict = if record.reclaimable {
        "reclaimable: reclaim_workspace removes it".to_string()
    } else {
        format!(
            "held: {}",
            record
                .holds
                .iter()
                .map(|hold| crate::reclaim::hold_sentence(hold))
                .collect::<Vec<_>>()
                .join("; ")
        )
    };
    format!(
        "- {} ({}): {tasks}. Branch {}; {} uncommitted files, {} unpushed commits, {} behind. {} on \
         disk{}. Last activity {}. {verdict}.{}",
        subject.name,
        subject.workspace_id,
        if branches.is_empty() { "none".to_string() } else { branches.join(", ") },
        record.dirty_files,
        record.unpushed_commits,
        record.behind_commits,
        record
            .size_bytes
            .map(crate::reclaim::human_bytes)
            .unwrap_or_else(|| "unmeasured".to_string()),
        if record.pruned_bytes > 0 {
            format!(", {} of build output dropped", crate::reclaim::human_bytes(record.pruned_bytes))
        } else {
            String::new()
        },
        record
            .last_activity_ms
            .and_then(rfc3339_of_ms)
            .unwrap_or_else(|| "unknown".to_string()),
        if record.holds.iter().any(|hold| hold == crate::reclaim::HOLD_LOCKED) {
            " The user locked this workspace on purpose; evaluate whether the user should be \
              reminded about it. Do not reclaim it or ask the user to unlock it by default."
        } else {
            ""
        },
    )
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

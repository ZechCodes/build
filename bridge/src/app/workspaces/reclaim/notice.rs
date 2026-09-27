//! What the project agent is told about its quiet workspaces.

use super::Quiet;
use crate::reclaim::{LifecycleRecord, LinkedTask, ReclaimPolicy, Subject};

/// The phase a quiet-workspace notice's turn is queued under.
pub(super) const PHASE: &str = "workspace_idle";

/// What the project agent reads about its quiet workspaces: one line per
/// workspace with everything a decision needs, then the three choices and the
/// one rule.
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
         the user.\n\n{}\n\nFor each one, decide: merge it (branch reviewed and green: merge to main, \
         then reclaim_workspace), delete it (abandoned or superseded: delete_workspace), or surface \
         it (something needs the user: one question on the linked task, and leave the workspace \
         alone). Never delete a workspace with uncommitted or unpushed work without asking the user \
         first. Record what you did on the linked task.",
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
         disk{}. Last activity {}. {verdict}.",
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

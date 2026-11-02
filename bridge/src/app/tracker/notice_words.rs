//! What Build says to an agent about a task's prerequisites: the words of the
//! dependency notices and the context an assignment carries about them.
//!
//! These are message bodies Build writes onto an agent's thread, so they are
//! stored as said and name Build's tools plainly; the turn that delivers them
//! attaches each tool by the name its harness gives it.

use crate::tracker::{Task, TaskEvent, TaskEventKind};

/// The line a dependency notice opens with: the task was released, could not
/// be delivered, or was blocked again. `coordinator_release` is the project
/// agent being told an unassigned task is free to give to someone.
pub(super) fn dependency_line(task: &Task, event: &TaskEvent, coordinator_release: bool) -> String {
    match event.kind {
        TaskEventKind::Unblocked if coordinator_release => format!("Task #{} — {} ({}) has no unresolved prerequisites. Decide who should take it; read the current task with get_task.", task.number, task.title, task.id),
        TaskEventKind::Unblocked => format!("Task #{} — {} ({}) is unblocked. This is informational; it does not assign the task or dispatch another assignment. Read the current task with get_task.", task.number, task.title, task.id),
        TaskEventKind::DispatchFailed => format!("Task #{} — {} ({}) could not be delivered: {}. Read the task with get_task and decide how to recover the saved assignment.", task.number, task.title, task.id, event.payload["error"].as_str().unwrap_or("delivery failed")),
        _ => format!("Task #{} — {} ({}) is blocked again by prerequisites {}. This is informational; it does not dispatch another assignment. Read the current task with get_task.", task.number, task.title, task.id, event.payload["unresolved_task_ids"]),
    }
}

/// A removed prerequisite as the release names it: `#7 — Title (task-id)`.
pub(super) fn removed_prerequisite(number: u64, title: &str, task_id: &str) -> String {
    format!("#{number} — {title} ({task_id})")
}

/// The prerequisite edges a release removed, each already named.
pub(super) fn prerequisites_removed(names: &[String]) -> String {
    format!(
        "Prerequisite edges were removed: {}. Removing an edge does not mark its task Done.",
        names.join(", ")
    )
}

/// One prerequisite that was closed without being done, as `(number, title,
/// task_id)`.
pub(super) type ClosedPrerequisite<'a> = (u64, &'a str, &'a str);

/// The prerequisites of `task` that were closed without Done, and what to do
/// about them.
pub(super) fn prerequisites_closed(closed: &[ClosedPrerequisite], task: &Task) -> String {
    let lines: Vec<String> = closed
        .iter()
        .map(|(number, title, task_id)| format!("- #{number} — {title} (task_id: {task_id})"))
        .collect();
    format!(
        "Prerequisites closed without Done:\n{}\n\nRead each listed task with get_task and assess whether its closure clearly permits this task to proceed. When it does, proceed. If unclear, use comment_task with mention_user: true on dependent #{} (task_id: {}), naming the uncertain prerequisites and the decision needed.",
        lines.join("\n"), task.number, task.id
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tracker::Actor;
    use serde_json::json;

    fn task() -> Task {
        let mut task = Task::drafted("/repo", "Kanban drag", Actor::User, "2026-09-20T15:00:00Z");
        task.number = 13;
        task.id = "task-13".into();
        task
    }

    fn event(kind: TaskEventKind, payload: serde_json::Value) -> TaskEvent {
        serde_json::from_value(json!({
            "id": "event-1",
            "task_id": "task-13",
            "at": "2026-09-20T15:00:00Z",
            "actor": {"kind": "user"},
            "kind": kind,
            "payload": payload,
        }))
        .unwrap()
    }

    #[test]
    fn each_dependency_notice_opens_with_its_own_line() {
        let task = task();
        let unblocked = event(TaskEventKind::Unblocked, json!({}));
        assert_eq!(
            dependency_line(&task, &unblocked, true),
            "Task #13 — Kanban drag (task-13) has no unresolved prerequisites. Decide who \
             should take it; read the current task with get_task."
        );
        assert_eq!(
            dependency_line(&task, &unblocked, false),
            "Task #13 — Kanban drag (task-13) is unblocked. This is informational; it does \
             not assign the task or dispatch another assignment. Read the current task with \
             get_task."
        );
        assert_eq!(
            dependency_line(
                &task,
                &event(
                    TaskEventKind::DispatchFailed,
                    json!({"error": "agent gone"})
                ),
                false
            ),
            "Task #13 — Kanban drag (task-13) could not be delivered: agent gone. Read the \
             task with get_task and decide how to recover the saved assignment."
        );
        assert_eq!(
            dependency_line(
                &task,
                &event(TaskEventKind::DispatchFailed, json!({})),
                false
            ),
            "Task #13 — Kanban drag (task-13) could not be delivered: delivery failed. Read \
             the task with get_task and decide how to recover the saved assignment."
        );
        assert_eq!(
            dependency_line(
                &task,
                &event(
                    TaskEventKind::Blocked,
                    json!({"unresolved_task_ids": ["task-7"]})
                ),
                false
            ),
            "Task #13 — Kanban drag (task-13) is blocked again by prerequisites [\"task-7\"]. \
             This is informational; it does not dispatch another assignment. Read the \
             current task with get_task."
        );
    }

    #[test]
    fn a_release_names_the_edges_it_removed() {
        let names = vec![
            removed_prerequisite(7, "Parser", "task-7"),
            "task-8".to_string(),
        ];
        assert_eq!(
            prerequisites_removed(&names),
            "Prerequisite edges were removed: #7 — Parser (task-7), task-8. Removing an edge \
             does not mark its task Done."
        );
    }

    #[test]
    fn closed_prerequisites_are_listed_with_what_to_do() {
        assert_eq!(
            prerequisites_closed(&[(7, "Parser", "task-7"), (8, "Lexer", "task-8")], &task()),
            "Prerequisites closed without Done:\n- #7 — Parser (task_id: task-7)\n- #8 — \
             Lexer (task_id: task-8)\n\nRead each listed task with get_task and assess \
             whether its closure clearly permits this task to proceed. When it does, \
             proceed. If unclear, use comment_task with mention_user: true on dependent #13 \
             (task_id: task-13), naming the uncertain prerequisites and the decision needed."
        );
    }
}

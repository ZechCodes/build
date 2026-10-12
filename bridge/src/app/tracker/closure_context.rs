use super::StoredAnswer;
use crate::app::AppState;
use crate::tracker::Task;

impl AppState {
    /// Read retained edges directly so queued lifecycle reconciliation cannot
    /// omit closures that predate the final release or the assignment.
    pub(in crate::app) fn assignment_closure_context(&self, task: &Task) -> Result<String, String> {
        let summary = self
            .tracker_store()?
            .waiting_on_summary(&task.project_path, &task.id)
            .stored()?;
        let closed: Vec<_> = summary
            .waiting_on
            .iter()
            .filter(|dependency| dependency.resolved_by.as_deref() == Some("close"))
            .map(|dependency| {
                format!(
                    "- #{} — {} (task_id: {})",
                    dependency.number, dependency.title, dependency.task_id
                )
            })
            .collect();
        if closed.is_empty() {
            return Ok(String::new());
        }
        Ok(format!(
            "Prerequisites closed without Done:\n{}\n\nRead each listed task with get_task and assess whether its closure clearly permits this task to proceed. When it does, proceed. If unclear, use comment_task with mention_user: true on dependent #{} (task_id: {}), naming the uncertain prerequisites and the decision needed.",
            closed.join("\n"), task.number, task.id
        ))
    }
}

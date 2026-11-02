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
                (
                    dependency.number,
                    dependency.title.as_str(),
                    dependency.task_id.as_str(),
                )
            })
            .collect();
        if closed.is_empty() {
            return Ok(String::new());
        }
        Ok(super::notice_words::prerequisites_closed(&closed, task))
    }
}

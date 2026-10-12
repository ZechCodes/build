use crate::app::AppState;
use crate::tracker::Task;

impl AppState {
    pub(in crate::app) fn assignment_closure_context(&self, _task: &Task) -> Result<String, String> {
        Ok(String::new())
    }
}

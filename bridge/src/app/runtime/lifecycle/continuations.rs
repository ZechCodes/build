use crate::app::AppState;
use serde_json::Value;

pub trait ImplementationCaller: Send {
    fn opened(self: Box<Self>, state: &mut AppState, run_id: &str) -> Result<Value, String>;
    fn refused(self: Box<Self>, state: &mut AppState, error: String) -> String;
    fn settle(
        self: Box<Self>,
        state: &mut AppState,
        result: Result<&str, String>,
    ) -> Result<Value, String> {
        match result {
            Ok(run_id) => self.opened(state, run_id),
            Err(error) => Err(self.refused(state, error)),
        }
    }
}

pub trait PlanSessionOpening: Send {
    fn open(
        self: Box<Self>,
        state: &mut AppState,
        workspace: crate::orchestrator::PlanWorkspace,
    ) -> Result<Value, String>;
    fn refused(self: Box<Self>, _state: &mut AppState, error: String) -> Result<Value, String> {
        Err(error)
    }
}

use super::{AppState, ProjectScope};
use crate::store::{ReadConnection, ReadPool};
use serde_json::Value;

type Query = Box<dyn FnOnce(Option<&mut ReadConnection>) -> Result<ReadWork, String> + Send>;
type Run = Box<dyn FnOnce() -> Result<Value, String> + Send>;
type Finalize = Box<dyn FnOnce(&AppState, Value) -> Result<Value, String> + Send>;

pub(super) struct PreparedRead {
    pub(super) pool: Option<ReadPool>,
    pub(super) query: Query,
    pub(super) scope: Option<ProjectScope>,
}

impl PreparedRead {
    pub(super) fn query(
        pool: Option<ReadPool>,
        query: impl FnOnce(Option<&mut ReadConnection>) -> Result<ReadWork, String> + Send + 'static,
    ) -> Self {
        Self {
            pool,
            query: Box::new(query),
            scope: None,
        }
    }

    pub(super) fn ready(result: Result<Value, String>) -> Self {
        Self::query(None, move |_| Ok(ReadWork::result(result)))
    }

    pub(super) fn for_project(mut self, scope: ProjectScope) -> Self {
        self.scope = Some(scope);
        self
    }
}

pub(super) struct ReadWork {
    run: Run,
    finalize: Finalize,
}

impl ReadWork {
    pub(super) fn result(result: Result<Value, String>) -> Self {
        Self::external(move || result)
    }

    pub(super) fn external(run: impl FnOnce() -> Result<Value, String> + Send + 'static) -> Self {
        Self {
            run: Box::new(run),
            finalize: Box::new(|_, value| Ok(value)),
        }
    }

    /// Finalizers perform immutable checks/decoration only, never SQL or I/O.
    pub(super) fn finish_with(
        mut self,
        finalize: impl FnOnce(&AppState, Value) -> Result<Value, String> + Send + 'static,
    ) -> Self {
        self.finalize = Box::new(finalize);
        self
    }

    pub(super) fn run(self) -> Outcome {
        Outcome {
            result: (self.run)(),
            finalize: Some(self.finalize),
        }
    }
}

pub(super) struct Outcome {
    result: Result<Value, String>,
    finalize: Option<Finalize>,
}

impl Outcome {
    pub(super) fn error(error: String) -> Self {
        Self {
            result: Err(error),
            finalize: None,
        }
    }

    pub(super) fn finish(self, app: &AppState) -> Result<Value, String> {
        match self.finalize {
            Some(finalize) => self.result.and_then(|value| finalize(app, value)),
            None => self.result,
        }
    }
}

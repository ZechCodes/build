//! The application adapter for `github.repos`: the verb hands `gh` to the
//! off-lock drain and answers what it printed. Nothing is kept here.

use serde_json::{json, Value};

use crate::app::{AppState, DeferredWork};
use crate::github::GithubCli;

impl AppState {
    /// Run a different `gh` — how a test stands a fake in for the real one.
    pub fn with_github_cli(mut self, github: GithubCli) -> Self {
        self.github = github;
        self
    }

    /// Queue the listing for the drain, which runs it with the mutex released.
    pub(crate) fn defer_github_repos(&mut self) -> Value {
        let github = self.github.clone();
        self.deferred_work = Some(DeferredWork::External(Box::new(move || {
            github.list_repos().map(|repos| json!({ "repos": repos }))
        })));
        Value::Null
    }
}

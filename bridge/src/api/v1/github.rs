//! The GitHub repositories this machine's `gh` can reach (`github.repos`).
//!
//! The handler defers: `gh` talks to the network for up to
//! [`crate::github::GH_DEADLINE`], so it runs on the off-lock drain and what
//! this returns is the placeholder [`Answer`] documents.

use super::{answer, Answer, Handler, NoParams};
use crate::api::ApiError;
use crate::app::AppState;
use crate::github::GithubRepo;
use crate::{v1_method, v1_methods};
use serde::{Deserialize, Serialize};

pub fn methods() -> &'static [(&'static str, Handler)] {
    v1_methods![v1_method!(
        "github.repos",
        repos,
        NoParams,
        GithubReposResult
    )]
}

/// What `github.repos` answers: the signed-in account's repositories and its
/// organisations', each once.
#[derive(Debug, Deserialize, Serialize)]
pub struct GithubReposResult {
    pub repos: Vec<GithubRepo>,
}

fn repos(app: &mut AppState, _params: NoParams) -> Result<Answer<GithubReposResult>, ApiError> {
    answer(Ok(app.defer_github_repos()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_github_repos_fixture_round_trips() {
        crate::api::v1::testing::fixture_round_trips(methods(), "github.repos");
    }
}

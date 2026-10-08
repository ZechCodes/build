//! Durable PR integration and publication, independent of browser lifetime.

use super::actions::ActionSource;
use super::model::ReviewMergeRequest;
use super::records::Review;
use crate::store::Store;

pub struct MergeJob {
    pub project_path: String,
    pub request_id: String,
    pub request: ReviewMergeRequest,
    /// Trusted current source identities, resolved by the adapter.
    pub sources: Vec<ActionSource>,
}

pub fn merge(_store: &Store, _job: &MergeJob, _notify: impl Fn()) -> Result<Review, String> {
    Err("PR merge is not implemented".into())
}

#[cfg(test)]
mod tests;

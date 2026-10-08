//! PR opinions and terminal state, independent of board organization.

use super::model::PullRequestStatus;
use crate::tracker::{
    Actor, ReviewVerdict, TaskComment, DONE_STATUS, IN_PROGRESS_STATUS, IN_REVIEW_STATUS,
};
mod reopen;
pub use reopen::{reopen, ReopenHooks, ReopenRequest};

pub fn column(status: PullRequestStatus) -> &'static str {
    match status {
        PullRequestStatus::Open | PullRequestStatus::Approved => IN_REVIEW_STATUS,
        PullRequestStatus::ChangesRequested => IN_PROGRESS_STATUS,
        PullRequestStatus::Merged | PullRequestStatus::Closed => DONE_STATUS,
    }
}

#[cfg(test)]
mod tests;

/// Historical opinions remain readable; only each actor's latest opinion on
/// the current immutable snapshot contributes to its status.
pub fn opinion_status(snapshot_id: &str, comments: &[TaskComment]) -> PullRequestStatus {
    let mut ordered: Vec<_> = comments.iter().collect();
    ordered
        .sort_by(|left, right| (&left.created_at, &left.id).cmp(&(&right.created_at, &right.id)));
    let mut latest: Vec<(&Actor, ReviewVerdict)> = Vec::new();
    for comment in ordered {
        let Some(opinion) = comment
            .opinion
            .as_ref()
            .filter(|opinion| opinion.snapshot_id == snapshot_id)
        else {
            continue;
        };
        if let Some((_, verdict)) = latest
            .iter_mut()
            .find(|(actor, _)| *actor == &comment.author)
        {
            *verdict = opinion.verdict;
        } else {
            latest.push((&comment.author, opinion.verdict));
        }
    }
    if latest
        .iter()
        .any(|(_, verdict)| *verdict == ReviewVerdict::RequestChanges)
    {
        PullRequestStatus::ChangesRequested
    } else if latest.is_empty() {
        PullRequestStatus::Open
    } else {
        PullRequestStatus::Approved
    }
}

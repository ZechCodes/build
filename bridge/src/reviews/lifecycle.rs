//! PR opinions and terminal state, independent of board organization.

use super::model::PullRequestStatus;
use crate::tracker::{
    Actor, Assignee, ReviewVerdict, TaskComment, DONE_STATUS, IN_PROGRESS_STATUS, IN_REVIEW_STATUS,
};
mod reopen;
pub use reopen::{reopen, ReopenHooks, ReopenRequest};

pub fn column(status: PullRequestStatus) -> &'static str {
    match status {
        PullRequestStatus::Open | PullRequestStatus::Approved => IN_REVIEW_STATUS,
        PullRequestStatus::ChangesRequested => IN_PROGRESS_STATUS,
        PullRequestStatus::Merged | PullRequestStatus::Closed | PullRequestStatus::Rejected => {
            DONE_STATUS
        }
    }
}

#[cfg(test)]
mod tests;

/// The user always decides; an agent decides only while assigned the review.
/// Other agents' saved opinions remain advisory.
pub fn is_deciding_reviewer(actor: &Actor, assignee: Option<&Assignee>) -> bool {
    match actor {
        Actor::User => true,
        Actor::Agent { agent_id } => {
            assignee.and_then(Assignee::agent_id) == Some(agent_id.as_str())
        }
        Actor::Build => false,
    }
}

/// Only each deciding reviewer's latest opinion on the current immutable
/// snapshot contributes. Historical and advisory opinions remain readable.
pub fn opinion_status(
    snapshot_id: &str,
    comments: &[TaskComment],
    assignee: Option<&Assignee>,
) -> PullRequestStatus {
    let mut ordered: Vec<_> = comments.iter().collect();
    ordered
        .sort_by(|left, right| (&left.created_at, &left.id).cmp(&(&right.created_at, &right.id)));
    let mut latest: Vec<(&Actor, ReviewVerdict)> = Vec::new();
    for comment in ordered
        .into_iter()
        .filter(|comment| is_deciding_reviewer(&comment.author, assignee))
    {
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
        .any(|(_, verdict)| *verdict == ReviewVerdict::Reject)
    {
        PullRequestStatus::Rejected
    } else if latest
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

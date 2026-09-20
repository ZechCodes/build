//! Fencing the typed references a comment carries.
//!
//! The same two-part rule the Issue Security Checklist records for a thread
//! message's links (controls 8 and 9), with the ownership half scoped to the
//! ISSUE rather than to a conversation:
//!
//! 1. **Shape** — `validate_thread_links`, unchanged and shared, so a path that
//!    escapes a checkout or a commit that is not a sha is refused in exactly
//!    the same words wherever it arrives.
//! 2. **Ownership** — a reference must name something this issue is about.
//!
//! A reference that fails either half refuses the whole call. Nothing partially
//! lands: half a comment is a comment whose references lie about what it read.

use crate::app::validate_thread_links;
use crate::thread::ThreadLink;
use crate::tracker::Issue;
use serde_json::Value;
use std::collections::BTreeSet;

/// The references a comment asked to carry, or why the comment is refused.
///
/// `checkouts` is the set of checkout ids the issue's linked workspaces derive
/// — resolved by the caller, which is the only half of this that needs the
/// workspace registry. Everything else here is the issue's own record.
pub(super) fn fenced_refs(
    params: &Value,
    issue: &Issue,
    checkouts: &BTreeSet<String>,
) -> Result<Vec<ThreadLink>, String> {
    let links = parse_refs(params)?;
    validate_thread_links(&links)?;
    for link in &links {
        owned_by_issue(link, issue, checkouts)?;
    }
    Ok(links)
}

fn parse_refs(params: &Value) -> Result<Vec<ThreadLink>, String> {
    match params.get("refs") {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(values)) => values
            .iter()
            .cloned()
            .map(|value| serde_json::from_value(value).map_err(|error| format!("refs: {error}")))
            .collect(),
        Some(_) => Err("refs must be an array".to_string()),
    }
}

/// Whether this issue is about the thing the reference names.
///
/// The plan flow's four kinds are refused outright rather than checked: the
/// tracker does not extend that flow, so a reference into it could only ever
/// point at something this issue has no relationship with. Refusing by name
/// beats accepting a link no surface will resolve.
fn owned_by_issue(
    link: &ThreadLink,
    issue: &Issue,
    checkouts: &BTreeSet<String>,
) -> Result<(), String> {
    match link {
        // A file path is checkout-relative, so it only means something once
        // the issue says which checkout. The shape check already fenced it
        // inside a worktree; this says there is a worktree for it to be inside.
        ThreadLink::File { .. } if issue.links.workspace_ids.is_empty() => {
            Err("a file reference needs the issue to link the workspace it is in".to_string())
        }
        ThreadLink::File { .. } => Ok(()),
        ThreadLink::Commit { sha } if !issue.links.links_commit(sha) => Err(format!(
            "commit reference {sha} is not one this issue links"
        )),
        ThreadLink::Commit { .. } => Ok(()),
        ThreadLink::Worktree { worktree_id } => owned_worktree(worktree_id, checkouts),
        ThreadLink::PlanStage { .. }
        | ThreadLink::IssueStage { .. }
        | ThreadLink::Run { .. }
        | ThreadLink::Implementation { .. }
        | ThreadLink::Recovery { .. } => Err(
            "plan-flow references do not belong on a tracker issue — link a workspace, \
             a branch, a commit or a conversation instead"
                .to_string(),
        ),
    }
}

/// A worktree reference has to name a checkout of a workspace this issue
/// links. The id is derived from the checkout's PATH, so this is an exact
/// comparison rather than a name match: two directories called `bridge` in two
/// workspaces derive two ids.
fn owned_worktree(worktree_id: &str, checkouts: &BTreeSet<String>) -> Result<(), String> {
    if checkouts.contains(worktree_id) {
        return Ok(());
    }
    Err(format!(
        "worktree reference {worktree_id} is not a checkout of a workspace this issue links"
    ))
}

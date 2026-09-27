//! Fencing the typed references a comment carries.
//!
//! The same two-part rule the Task Security Checklist records for a thread
//! message's links (controls 8 and 9), with the ownership half scoped to the
//! TASK rather than to a conversation:
//!
//! 1. **Shape** — [`validate_shape`], which is the check `post_thread_message`
//!    carried until agent-attached links were dropped from it. Nothing else
//!    writes a `ThreadLink` now, so the rule lives beside its one caller rather
//!    than in the conversation module it no longer has anything to do with. The
//!    refusals keep their old words: a path that escapes a checkout and a
//!    commit that is not a sha read the same as they always did.
//! 2. **Ownership** — a reference must name something this task is about.
//!
//! A reference that fails either half refuses the whole call. Nothing partially
//! lands: half a comment is a comment whose references lie about what it read.

use crate::thread::ThreadLink;
use crate::tracker::Task;
use serde_json::Value;
use std::collections::BTreeSet;

/// The references a comment asked to carry, or why the comment is refused.
///
/// `checkouts` is the set of checkout ids the task's linked workspaces derive
/// — resolved by the caller, which is the only half of this that needs the
/// workspace registry. Everything else here is the task's own record.
pub(super) fn fenced_refs(
    params: &Value,
    task: &Task,
    checkouts: &BTreeSet<String>,
) -> Result<Vec<ThreadLink>, String> {
    let links = parse_refs(params)?;
    validate_shape(&links)?;
    for link in &links {
        owned_by_task(link, task, checkouts)?;
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

/// Whether this task is about the thing the reference names.
///
/// The plan flow's four kinds are refused outright rather than checked: the
/// tracker does not extend that flow, so a reference into it could only ever
/// point at something this task has no relationship with. Refusing by name
/// beats accepting a link no surface will resolve.
fn owned_by_task(
    link: &ThreadLink,
    task: &Task,
    checkouts: &BTreeSet<String>,
) -> Result<(), String> {
    match link {
        // A file path is checkout-relative, so it only means something once
        // the task says which checkout. The shape check already fenced it
        // inside a worktree; this says there is a worktree for it to be inside.
        ThreadLink::File { .. } if task.links.workspace_ids.is_empty() => {
            Err("a file reference needs the task to link the workspace it is in".to_string())
        }
        ThreadLink::File { .. } => Ok(()),
        ThreadLink::Commit { sha } if !task.links.links_commit(sha) => {
            Err(format!("commit reference {sha} is not one this task links"))
        }
        ThreadLink::Commit { .. } => Ok(()),
        ThreadLink::Worktree { worktree_id } => owned_worktree(worktree_id, checkouts),
        ThreadLink::PlanStage { .. }
        | ThreadLink::TaskStage { .. }
        | ThreadLink::Run { .. }
        | ThreadLink::Implementation { .. }
        | ThreadLink::Recovery { .. } => Err(
            "plan-flow references do not belong on a tracker task — link a workspace, \
             a branch, a commit or a conversation instead"
                .to_string(),
        ),
    }
}

/// A worktree reference has to name a checkout of a workspace this task
/// links. The id is derived from the checkout's PATH, so this is an exact
/// comparison rather than a name match: two directories called `bridge` in two
/// workspaces derive two ids.
fn owned_worktree(worktree_id: &str, checkouts: &BTreeSet<String>) -> Result<(), String> {
    if checkouts.contains(worktree_id) {
        return Ok(());
    }
    Err(format!(
        "worktree reference {worktree_id} is not a checkout of a workspace this task links"
    ))
}

/// The shape half of the fence: how many, and whether each is built the way
/// its kind is built.
///
/// Only the three kinds the tracker accepts are checked in detail. The plan
/// flow's five are refused outright by [`owned_by_task`] whatever they hold,
/// so a second opinion on their internals here would be a rule nothing reads.
fn validate_shape(links: &[ThreadLink]) -> Result<(), String> {
    if links.len() > MAX_REFS {
        return Err(format!("refs must contain at most {MAX_REFS} entries"));
    }
    for link in links {
        match link {
            ThreadLink::File {
                path,
                line_start,
                line_end,
            } => {
                if path.is_empty() || !crate::plan::is_worktree_contained_path(path) {
                    return Err("file link path escapes the worktree".to_string());
                }
                if line_start.is_some_and(|line| line == 0)
                    || line_end.is_some_and(|line| line == 0)
                    || matches!((line_start, line_end), (Some(start), Some(end)) if start > end)
                {
                    return Err("file link line range is invalid".to_string());
                }
            }
            ThreadLink::Commit { sha }
                if sha.len() != 40
                    || !sha
                        .bytes()
                        .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()) =>
            {
                return Err("commit link is invalid".to_string());
            }
            ThreadLink::Worktree { worktree_id }
                if !worktree_id.starts_with("wt-") || worktree_id.len() != 15 =>
            {
                return Err("worktree link is invalid".to_string());
            }
            _ => {}
        }
    }
    Ok(())
}

/// How many references one comment may carry — the bound a thread message's
/// links carried, kept because the reason for it is unchanged: a reference
/// list long enough to be a document is not a reference list.
const MAX_REFS: usize = 20;

//! Review context attached to ordinary task comments. The snapshot identity
//! is checked on write; older comments remain readable after replacement.

use crate::app::AppState;
use crate::tracker::{ReviewCommentAnchor, ReviewOpinion};
use serde_json::Value;
use std::path::{Component, Path};

// The same four-KiB path bound used for a thread's viewing context.
const MAX_ANCHOR_PATH_BYTES: usize = 4 * 1024;

pub(super) struct CommentMetadata {
    pub anchor: Option<ReviewCommentAnchor>,
    pub reply_to: Option<String>,
    pub opinion: Option<ReviewOpinion>,
}

impl CommentMetadata {
    pub fn from_params(app: &AppState, task_id: &str, params: &Value) -> Result<Self, String> {
        let anchor: Option<ReviewCommentAnchor> = parse_optional(params, "anchor")?;
        let reply_to: Option<String> = parse_optional(params, "reply_to")?;
        let opinion: Option<ReviewOpinion> = parse_optional(params, "opinion")?;
        if let Some(comment_id) = reply_to.as_deref() {
            if comment_id.trim().is_empty() {
                return Err("invalid review reply_to: comment id is empty".into());
            }
            let reply = app
                .tracker_store()?
                .load_tracker_comment(comment_id)
                .map_err(|error| error.to_string())?
                .ok_or_else(|| format!("unknown reply_to: {comment_id}"))?;
            if reply.task_id != task_id {
                return Err(format!(
                    "invalid review reply_to: comment belongs to another task: {comment_id}"
                ));
            }
        }
        if anchor.is_some() || opinion.is_some() {
            let review = app
                .tracker_store()?
                .load_review(task_id)
                .map_err(|error| error.to_string())?
                .ok_or_else(|| format!("unknown review for task_id: {task_id}"))?;
            if let Some(anchor) = &anchor {
                let snapshot = review
                    .snapshots
                    .iter()
                    .find(|snapshot| snapshot.id == anchor.snapshot_id)
                    .ok_or_else(|| format!("unknown snapshot_id: {}", anchor.snapshot_id))?;
                if !snapshot
                    .directories
                    .iter()
                    .any(|directory| directory.id == anchor.directory_id)
                {
                    return Err(format!(
                        "invalid review anchor: directory_id is not in snapshot: {}",
                        anchor.directory_id
                    ));
                }
                if anchor.line == 0
                    || anchor.path.len() > MAX_ANCHOR_PATH_BYTES
                    || !safe_relative_path(&anchor.path)
                {
                    return Err(format!("invalid review anchor: path must be relative and at most {MAX_ANCHOR_PATH_BYTES} bytes; line must be positive"));
                }
            }
            if let Some(opinion) = &opinion {
                if !review
                    .snapshots
                    .iter()
                    .any(|snapshot| snapshot.id == opinion.snapshot_id)
                {
                    return Err(format!("unknown snapshot_id: {}", opinion.snapshot_id));
                }
                if anchor
                    .as_ref()
                    .is_some_and(|anchor| anchor.snapshot_id != opinion.snapshot_id)
                {
                    return Err(
                        "invalid review comment: anchor and opinion name different snapshots"
                            .into(),
                    );
                }
            }
        }
        Ok(Self {
            anchor,
            reply_to,
            opinion,
        })
    }
}

fn parse_optional<T: serde::de::DeserializeOwned>(
    params: &Value,
    key: &str,
) -> Result<Option<T>, String> {
    params
        .get(key)
        .filter(|value| !value.is_null())
        .map(|value| {
            serde_json::from_value(value.clone()).map_err(|error| format!("invalid {key}: {error}"))
        })
        .transpose()
}

fn safe_relative_path(path: &str) -> bool {
    !path.is_empty()
        && !path.contains('\\')
        && Path::new(path)
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
        && !path
            .split('/')
            .any(|part| part == "." || part == ".." || part.is_empty())
}

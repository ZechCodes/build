//! MCP definitions for the task's saved review. The daemon resolves every ID
//! against the caller's project; none of these tools accepts a repository path
//! or an author supplied by the agent.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::body_page::FileRange;
use crate::renamed_ids::current_id;
use crate::reviews::actions::{ReviewActParams, SourceSelection};
use crate::reviews::read::ReviewReadMode;
use crate::reviews::records::{review_description, MAX_REVIEW_DESCRIPTION_BYTES};

use super::{acted, optional_argument, refused, required_argument, BridgeAction, Handled};

const MAX_PATHS: usize = 100;
const MAX_BASE_OVERRIDES: usize = 100;

pub(super) fn tools() -> Vec<Value> {
    let task_id = json!({"type":"string", "description":"A task of your project."});
    let range = json!({
        "type":"object",
        "description":"A byte page. Use the returned span.end as the next offset. raw is available for blob bytes only.",
        "properties":{
            "offset":{"type":"integer", "minimum":0},
            "bytes":{"type":"integer", "minimum":1},
            "raw":{"type":"boolean"}
        },
        "required":["offset","bytes"]
    });
    vec![
        json!({
            "name":"snapshot_review",
            "description":"Save the committed state of every directory in a workspace on this task. The saved Git heads and bases remain readable after branches move; uncommitted files are counted but excluded. This does not move the task. Any agent in this project may create a snapshot; Build records the caller as author.",
            "inputSchema":{
                "type":"object",
                "properties":{
                    "task_id":task_id,
                    "workspace_id":{"type":"string", "description":"A workspace in your project, containing all directories to review."},
                    "expected_version":{"type":"integer", "minimum":0, "description":"The review version from get_review; use 0 for the first snapshot. A stale version is refused."},
                    "base_overrides":{
                        "type":"object", "maxProperties":MAX_BASE_OVERRIDES,
                        "description":"Optional per-directory Git base revspecs, keyed by workspace directory ID. Omitted directories use their configured base, then upstream, then the empty tree.",
                        "additionalProperties":{"type":"string", "minLength":1}
                    }
                },
                "required":["task_id","workspace_id","expected_version"]
            }
        }),
        json!({
            "name":"get_review",
            "description":"Read this task's review, its snapshots and each saved directory's status. Get the latest version before snapshot_review or complete_review. The task timeline remains available through get_task.",
            "inputSchema":{
                "type":"object", "properties":{"task_id":task_id}, "required":["task_id"]
            }
        }),
        json!({
            "name":"read_review",
            "description":"Read a directory from a saved review snapshot. changes returns committed changed files and optional patch; tree lists the entire saved Git tree, including unchanged files; blob reads one file at the saved head. Use the IDs from get_review, not a live repository path. Non-Git directories are labelled live in get_review.",
            "inputSchema":{
                "type":"object",
                "properties":{
                    "task_id":task_id,
                    "snapshot_id":{"type":"string", "description":"The saved snapshot ID from get_review."},
                    "directory_id":{"type":"string", "description":"A directory ID from that snapshot."},
                    "mode":{"type":"string", "enum":["changes","tree","blob"], "description":"Defaults to changes. tree lists all committed files; blob reads one file's saved bytes."},
                    "path":{"type":"string", "description":"Required for blob; an optional directory prefix for tree. Relative to the saved directory."},
                    "paths":{"type":"array", "maxItems":MAX_PATHS, "items":{"type":"string", "minLength":1}, "description":"Optional paths to narrow changes. Relative to the saved directory."},
                    "patch":{"type":"boolean", "description":"For changes, include the patch. Defaults to true; pass false for a cheap changed-file list."},
                    "range":range
                },
                "required":["task_id","snapshot_id","directory_id"]
            }
        }),
        json!({
            "name":"act_review",
            "description":"Run selected Merge and Push steps for saved Git sources, or leave a selected source unchanged. Choose each directory and destination from get_review. Merge precedes Push when both are selected. Results are recorded on the review; read get_review after interruption. This does not complete the task or remove its workspace.",
            "inputSchema":{
                "type":"object",
                "properties":{
                    "task_id":task_id,
                    "expected_version":{"type":"integer", "minimum":0},
                    "snapshot_id":{"type":"string"},
                    "sources":{"type":"array", "minItems":1, "maxItems":100, "items":{
                        "type":"object", "additionalProperties":false,
                        "properties":{
                            "directory_id":{"type":"string", "minLength":1},
                            "merge":{"type":"object", "additionalProperties":false, "properties":{"branch":{"type":"string", "minLength":1}}, "required":["branch"]},
                            "push":{"type":"object", "additionalProperties":false, "properties":{"remote":{"type":"string", "minLength":1}, "branch":{"type":"string", "minLength":1}, "merge_action_id":{"type":"string", "minLength":1}}, "required":["remote","branch"]}
                        },
                        "required":["directory_id"]
                    }}
                },
                "required":["task_id","expected_version","snapshot_id","sources"]
            }
        }),
        json!({
            "name":"complete_review",
            "description":"Finish this task's review and move the task to Done. Describe the action actually taken, such as 'merged API to dev; pushed web'. You may use your own Git tools and complete without a Build Git action. Any agent in this project may complete it; Build records the caller as actor. This does not delete the workspace or close the task.",
            "inputSchema":{
                "type":"object",
                "properties":{
                    "task_id":task_id,
                    "expected_version":{"type":"integer", "minimum":0, "description":"The latest review version from get_review."},
                    "description":{"type":"string", "minLength":1, "description":format!("A brief factual description of what was done, at most {MAX_REVIEW_DESCRIPTION_BYTES} UTF-8 bytes after trimming.")}
                },
                "required":["task_id","expected_version","description"]
            }
        }),
    ]
}

pub(super) fn handle_call(id: &Value, name: &str, params: Option<&Value>) -> Option<Handled> {
    let action = match name {
        "snapshot_review" => snapshot_action(params),
        "get_review" => task_id(params).map(|task_id| BridgeAction::TrackerGetReview { task_id }),
        "read_review" => read_action(params),
        "act_review" => act_action(params),
        "complete_review" => complete_action(params),
        _ => return None,
    };
    Some(match action {
        Ok(action) => acted(id.clone(), action),
        Err(message) => refused(id.clone(), message),
    })
}

fn task_id(params: Option<&Value>) -> Result<String, String> {
    required_argument(params, "task_id").map(|id| current_id(&id))
}

fn arguments(params: Option<&Value>) -> Result<&Value, String> {
    params
        .and_then(|p| p.get("arguments"))
        .filter(|value| value.is_object())
        .ok_or_else(|| "arguments are required".to_string())
}

fn required_version(params: Option<&Value>) -> Result<u64, String> {
    arguments(params)?
        .get("expected_version")
        .and_then(Value::as_u64)
        .ok_or_else(|| "expected_version must be a non-negative integer".to_string())
}

fn base_overrides(params: Option<&Value>) -> Result<BTreeMap<String, String>, String> {
    let Some(value) = arguments(params)?.get("base_overrides") else {
        return Ok(BTreeMap::new());
    };
    let overrides: BTreeMap<String, String> = serde_json::from_value(value.clone())
        .map_err(|error| format!("base_overrides: {error}"))?;
    if overrides.len() > MAX_BASE_OVERRIDES
        || overrides
            .iter()
            .any(|(directory, revspec)| directory.trim().is_empty() || revspec.trim().is_empty())
    {
        return Err(
            "base_overrides must name at most 100 directories with non-empty revspecs".to_string(),
        );
    }
    Ok(overrides)
}

fn snapshot_action(params: Option<&Value>) -> Result<BridgeAction, String> {
    Ok(BridgeAction::TrackerSnapshotReview {
        task_id: task_id(params)?,
        workspace_id: required_argument(params, "workspace_id")?,
        expected_version: required_version(params)?,
        base_overrides: base_overrides(params)?,
    })
}

fn complete_action(params: Option<&Value>) -> Result<BridgeAction, String> {
    let description = required_argument(params, "description")?;
    let description = review_description(&description)
        .ok_or_else(|| {
            format!(
                "description must contain 1 to {MAX_REVIEW_DESCRIPTION_BYTES} UTF-8 bytes after trimming"
            )
        })?
        .to_string();
    Ok(BridgeAction::TrackerCompleteReview {
        task_id: task_id(params)?,
        expected_version: required_version(params)?,
        description,
    })
}

fn act_action(params: Option<&Value>) -> Result<BridgeAction, String> {
    let args = arguments(params)?;
    let task_id = task_id(params)?;
    let expected_version = required_version(params)?;
    let snapshot_id = required_argument(params, "snapshot_id")?;
    let sources: Vec<SourceSelection> =
        serde_json::from_value(args.get("sources").cloned().ok_or("sources are required")?)
            .map_err(|error| format!("sources: {error}"))?;
    if sources.is_empty() || sources.len() > 100 {
        return Err("sources must contain 1 to 100 selections".to_string());
    }
    if sources.iter().any(|source| {
        source.directory_id.trim().is_empty()
            || source
                .merge
                .as_ref()
                .is_some_and(|merge| merge.branch.trim().is_empty())
            || source.push.as_ref().is_some_and(|push| {
                push.remote.trim().is_empty()
                    || push.branch.trim().is_empty()
                    || push
                        .merge_action_id
                        .as_ref()
                        .is_some_and(|id| id.trim().is_empty())
            })
    }) {
        return Err(
            "each source needs a directory ID and non-empty selected Git destinations".to_string(),
        );
    }
    Ok(BridgeAction::TrackerActReview {
        params: ReviewActParams {
            task_id,
            expected_version,
            snapshot_id,
            sources,
        },
    })
}

fn read_action(params: Option<&Value>) -> Result<BridgeAction, String> {
    let args = arguments(params)?;
    let mode = match args.get("mode") {
        None => ReviewReadMode::Changes,
        Some(value) => serde_json::from_value::<ReviewReadMode>(value.clone())
            .map_err(|error| format!("mode: {error}"))?,
    };
    let path = optional_argument(params, "path");
    let paths = paths(args)?;
    let patch = optional_bool(args, "patch")?.unwrap_or(true);
    let range = FileRange::from_params(args)?;
    validate_read_options(mode, path.as_deref(), &paths, range)?;
    Ok(BridgeAction::TrackerReadReview {
        task_id: task_id(params)?,
        snapshot_id: required_argument(params, "snapshot_id")?,
        directory_id: required_argument(params, "directory_id")?,
        mode,
        path,
        paths,
        patch,
        range,
    })
}

fn paths(args: &Value) -> Result<Vec<String>, String> {
    let Some(value) = args.get("paths") else {
        return Ok(Vec::new());
    };
    let paths: Vec<String> =
        serde_json::from_value(value.clone()).map_err(|error| format!("paths: {error}"))?;
    if paths.len() > MAX_PATHS || paths.iter().any(|path| path.trim().is_empty()) {
        return Err("paths must contain at most 100 non-empty paths".to_string());
    }
    Ok(paths)
}

fn optional_bool(args: &Value, field: &str) -> Result<Option<bool>, String> {
    match args.get(field) {
        None => Ok(None),
        Some(value) => value
            .as_bool()
            .map(Some)
            .ok_or_else(|| format!("{field} must be a boolean")),
    }
}

fn validate_read_options(
    mode: ReviewReadMode,
    path: Option<&str>,
    paths: &[String],
    range: Option<FileRange>,
) -> Result<(), String> {
    match mode {
        ReviewReadMode::Changes if path.is_some() => Err("path is for tree or blob".to_string()),
        ReviewReadMode::Tree if !paths.is_empty() || range.is_some() => {
            Err("tree accepts path but not paths or range".to_string())
        }
        ReviewReadMode::Blob if path.is_none() || !paths.is_empty() => {
            Err("blob requires path and does not accept paths".to_string())
        }
        _ if mode != ReviewReadMode::Blob && range.is_some_and(|page| page.raw == Some(true)) => {
            Err("range.raw is only for blob".to_string())
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_completion_description_boundary_counts_utf8_bytes() {
        let tools = tools();
        let completion = tools
            .iter()
            .find(|tool| tool["name"] == "complete_review")
            .unwrap();
        let schema = &completion["inputSchema"]["properties"]["description"];
        assert!(
            schema.get("maxLength").is_none(),
            "JSON length is not a byte limit"
        );
        assert!(schema["description"]
            .as_str()
            .unwrap()
            .contains("2000 UTF-8 bytes"));
        for (description, accepted) in [
            ("a".repeat(2_000), true),
            ("a".repeat(2_001), false),
            (format!(" {} ", "a".repeat(2_000)), true),
            (" \t ".into(), false),
            ("é".repeat(1_000), true),
            ("é".repeat(1_001), false),
            ("🦀".repeat(500), true),
            ("🦀".repeat(501), false),
        ] {
            let params = json!({"arguments": {
                "task_id":"task-1", "expected_version":1, "description":description
            }});
            let parsed = complete_action(Some(&params));
            assert_eq!(
                parsed.is_ok(),
                accepted,
                "{} UTF-8 bytes",
                description.len()
            );
        }
    }
}

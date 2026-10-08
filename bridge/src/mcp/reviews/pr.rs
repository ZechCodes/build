//! Strict PR tool contracts. These accept saved identities and leases only;
//! the app adapter supplies the actor, project and filesystem locations.

use serde::de::DeserializeOwned;
use serde_json::{json, Value};

use super::super::{BridgeAction, Handled};
use crate::api::v1::reviews::{
    ReviewCloseParams, ReviewMergeParams, ReviewOpenParams, ReviewPushParams, ReviewUpdateParams,
    ReviewVersionParams,
};
use crate::renamed_ids::current_id;

fn string() -> Value {
    json!({"type":"string","minLength":1})
}

fn version() -> Value {
    json!({"type":"integer","minimum":0,"description":"The latest version from get_review. A stale version is refused."})
}

fn head() -> Value {
    json!({"type":"string","pattern":"^[0-9a-fA-F]{40}$","description":"A full 40-character Git commit OID."})
}

fn bases(minimum: usize) -> Value {
    json!({
        "type":"array","minItems":minimum,"maxItems":100,
        "description":if minimum == 0 { "Optional local source base branches by directory ID. Omitted included Git directories use their configured base." } else { "Select unique Git directory IDs and their new local source base branches." },
        "items":{"type":"object","additionalProperties":false,
            "properties":{"directory_id":string(),"branch":string()},
            "required":["directory_id","branch"]}
    })
}

fn tool(name: &str, description: &str, properties: Value, required: &[&str]) -> Value {
    json!({"name":name,"description":description,"inputSchema":{
        "type":"object","additionalProperties":false,"properties":properties,"required":required
    }})
}

pub(super) fn tools() -> Vec<Value> {
    vec![
        tool("open_review", "Open a PR task from a managed workspace in your project. Build creates dedicated branches and durable local receivers, publishes the committed heads, and records you as creator. Included Git directories use their configured base unless you select another local source branch; explicitly exclude any Git directory outside the PR. Non-Git directories stay live. Reuse request_id only for the identical opening to recover a lost response. An optional reviewer is dispatched after publication. Read the returned push instructions before your next push.", json!({
            "workspace_id":string(),"request_id":string(),
            "title":{"type":"string","minLength":1,"description":"One nonempty line, at most 200 UTF-8 bytes."},
            "description":{"type":"string","description":"Task description, at most 32000 UTF-8 bytes."},
            "reviewer":{"oneOf":[
                {"type":"null"},
                {"type":"object","additionalProperties":false,"properties":{"kind":{"const":"user"}},"required":["kind"]},
                {"type":"object","additionalProperties":false,"properties":{"kind":{"const":"project_agent"}},"required":["kind"]},
                {"type":"object","additionalProperties":false,"properties":{"kind":{"const":"agent"},"agent_id":string()},"required":["kind","agent_id"]}
            ]},
            "bases":bases(0),
            "excluded_git_directory_ids":{"type":"array","maxItems":100,"uniqueItems":true,"items":string(),"description":"Git directory IDs intentionally excluded from this PR. Non-Git directories are included as live context."}
        }), &["workspace_id","request_id","title","description"]),
        tool("push_review", "Publish selected bound PR branches into Build's local review receivers. Supply the exact working expected_head from your checkout and latest expected_received_head from get_review; force_with_lease explicitly permits a rewrite against that received head. Each source has its own recorded outcome. Read get_review after an interrupted or partial result before trying again.", json!({
            "task_id":string(),"expected_version":version(),
            "sources":{"type":"array","minItems":1,"maxItems":100,"items":{
                "type":"object","additionalProperties":false,
                "properties":{"directory_id":string(),"expected_head":head(),"expected_received_head":{"type":["string","null"],"pattern":"^[0-9a-fA-F]{40}$"},"force_with_lease":{"type":"boolean","default":false}},
                "required":["directory_id","expected_head"]
            }}
        }), &["task_id","expected_version","sources"]),
        tool("merge_review", "Merge the selected published PR snapshot into its recorded source base branches. Read get_review first and supply each exact expected_base_head. Optional publication to a named remote and branch follows integration; a failed push keeps the completed integration recorded. Successful integration marks the PR merged and moves its task to Done. Inspect recorded outcomes before retrying partial or interrupted work.", json!({
            "task_id":string(),"expected_version":version(),"snapshot_id":string(),
            "sources":{"type":"array","minItems":1,"maxItems":100,"items":{
                "type":"object","additionalProperties":false,
                "properties":{"directory_id":string(),"expected_base_head":head(),"push":{
                    "type":["object","null"],"additionalProperties":false,
                    "properties":{"remote":string(),"branch":string()},"required":["remote","branch"]
                }},"required":["directory_id","expected_base_head"]
            }}
        }), &["task_id","expected_version","snapshot_id","sources"]),
        tool("close_review", "Close an active PR without merging it, record your factual description, and move the task to Done. The workspace and saved review history remain available. Read get_review for the current version first.", json!({
            "task_id":string(),"expected_version":version(),
            "description":{"type":"string","minLength":1,"description":"A factual description, at most 2000 UTF-8 bytes after trimming."}
        }), &["task_id","expected_version","description"]),
        tool("reopen_review", "Reopen a closed, unmerged PR whose managed workspace is retained. Build verifies its recorded placement and receiver health before moving it to In review. This preserves the review history; it does not unlock branch bindings.", json!({"task_id":string(),"expected_version":version()}), &["task_id","expected_version"]),
        tool("refresh_review", "Refresh a PR's received refs and health, including a terminal PR with a retained workspace. Build records a new immutable snapshot when published work changes and the PR is active. Terminal review history stays saved; get_review returns the resulting state.", json!({"task_id":string(),"expected_version":version()}), &["task_id","expected_version"]),
        tool("update_review_base", "Change selected comparison base branches for an active PR. Build resolves local branches from the recorded source and saves a new immutable snapshot; opinions on older snapshots remain historical. Read get_review for the latest version and directory IDs first.", json!({"task_id":string(),"expected_version":version(),"bases":bases(1)}), &["task_id","expected_version","bases"]),
    ]
}

fn params<T: DeserializeOwned>(params: Option<&Value>) -> Result<T, String> {
    serde_json::from_value(super::arguments(params)?.clone()).map_err(|error| error.to_string())
}

pub(super) fn handle_call(id: &Value, name: &str, arguments: Option<&Value>) -> Option<Handled> {
    let action = match name {
        "open_review" => params::<ReviewOpenParams>(arguments).and_then(|params| {
            params.validate()?;
            Ok(BridgeAction::TrackerOpenReview { params })
        }),
        "push_review" => params::<ReviewPushParams>(arguments).and_then(|mut params| {
            params.task_id = current_id(&params.task_id);
            params.validate()?;
            Ok(BridgeAction::TrackerPushReview { params })
        }),
        "merge_review" => params::<ReviewMergeParams>(arguments).and_then(|mut params| {
            params.task_id = current_id(&params.task_id);
            params.validate()?;
            Ok(BridgeAction::TrackerMergeReview { params })
        }),
        "close_review" => params::<ReviewCloseParams>(arguments).and_then(|mut params| {
            params.task_id = current_id(&params.task_id);
            params.validate()?;
            Ok(BridgeAction::TrackerCloseReview { params })
        }),
        "reopen_review" => {
            version_params(arguments).map(|params| BridgeAction::TrackerReopenReview { params })
        }
        "refresh_review" => {
            version_params(arguments).map(|params| BridgeAction::TrackerRefreshReview { params })
        }
        "update_review_base" => params::<ReviewUpdateParams>(arguments).and_then(|mut params| {
            params.task_id = current_id(&params.task_id);
            params.validate()?;
            Ok(BridgeAction::TrackerUpdateReviewBase { params })
        }),
        _ => return None,
    };
    Some(match action {
        Ok(action) => super::super::acted(id.clone(), action),
        Err(error) => super::super::refused(id.clone(), error),
    })
}

fn version_params(arguments: Option<&Value>) -> Result<ReviewVersionParams, String> {
    let mut params = params::<ReviewVersionParams>(arguments)?;
    params.task_id = current_id(&params.task_id);
    params.validate()?;
    Ok(params)
}

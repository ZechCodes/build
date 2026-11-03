//! MCP definitions for the task's saved review. The daemon resolves every ID
//! against the caller's project; none of these tools accepts a repository path
//! or an author supplied by the agent.

use serde_json::{json, Value};

use crate::body_page::FileRange;
use crate::renamed_ids::current_id;
use crate::reviews::read::ReviewReadMode;

use super::{acted, optional_argument, refused, required_argument, BridgeAction, Handled};

mod pr;

const MAX_PATHS: usize = 100;

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
    let mut tools = vec![
        json!({
            "name":"get_review",
            "description":"Read this task's review, its snapshots and each saved directory's status. The task timeline remains available through get_task.",
            "inputSchema":{
                "type":"object", "properties":{"task_id":task_id}, "required":["task_id"]
            }
        }),
        json!({
            "name":"read_review",
            "description":"Read a directory from a saved review snapshot. commits lists the revision’s commits with a bounded cursor; commit reads a first-parent diff; since compares an earlier revision’s head. changes returns committed changed files and optional patch; tree lists the entire saved Git tree, including unchanged files; blob reads one file at the saved head. Use the IDs from get_review, not a live repository path. Non-Git directories are labelled live in get_review.",
            "inputSchema":{
                "type":"object",
                "properties":{
                    "task_id":task_id,
                    "snapshot_id":{"type":"string", "description":"The saved snapshot ID from get_review."},
                    "directory_id":{"type":"string", "description":"A directory ID from that snapshot."},
                    "mode":{"type":"string", "enum":["changes","tree","blob","commits","commit","since"], "description":"Defaults to changes. commits lists the revision range; commit reads a first-parent diff; since compares revision heads. tree lists all committed files; blob reads one file's saved bytes."},
                    "commit":{"type":"string", "description":"Full commit OID in this revision's base..head; required for commit mode."},
                    "from_snapshot_id":{"type":"string", "description":"Earlier revision in this task; required for since mode."},
                    "limit":{"type":"integer", "minimum":1, "maximum":200, "description":"Commit page size, default 100, capped at 200."},
                    "cursor":{"type":"string", "description":"next_cursor from the previous commits page."},
                    "path":{"type":"string", "description":"Required for blob; an optional directory prefix for tree. Relative to the saved directory."},
                    "paths":{"type":"array", "maxItems":MAX_PATHS, "items":{"type":"string", "minLength":1}, "description":"Optional paths to narrow changes, commit, or since diffs. Relative to the saved directory."},
                    "patch":{"type":"boolean", "description":"For changes, commit, or since, include the patch. Defaults to true; pass false for a changed-file list."},
                    "range":range
                },
                "required":["task_id","snapshot_id","directory_id"]
            }
        }),
    ];
    tools.extend(pr::tools());
    tools
}

pub(super) fn handle_call(id: &Value, name: &str, params: Option<&Value>) -> Option<Handled> {
    if let Some(handled) = pr::handle_call(id, name, params) {
        return Some(handled);
    }
    let action = match name {
        "get_review" => task_id(params).map(|task_id| BridgeAction::TrackerGetReview { task_id }),
        "read_review" => read_action(params),
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
    let commit = read_optional(args, "commit")?;
    let from_snapshot_id = read_optional(args, "from_snapshot_id")?;
    let limit = read_optional(args, "limit")?;
    let cursor = read_optional(args, "cursor")?;
    crate::reviews::read::validate_request(&crate::reviews::read::ReviewReadRequest {
        mode,
        path: path.clone(),
        paths: paths.clone(),
        patch,
        range,
        commit: commit.clone(),
        from_snapshot_id: from_snapshot_id.clone(),
        limit,
        cursor: cursor.clone(),
    })?;
    Ok(BridgeAction::TrackerReadReview {
        task_id: task_id(params)?,
        snapshot_id: required_argument(params, "snapshot_id")?,
        directory_id: required_argument(params, "directory_id")?,
        mode,
        commit,
        from_snapshot_id,
        limit,
        cursor,
        path,
        paths,
        patch,
        range,
    })
}

fn read_optional<T: serde::de::DeserializeOwned>(
    args: &Value,
    field: &str,
) -> Result<Option<T>, String> {
    args.get(field)
        .map(|value| {
            serde_json::from_value(value.clone()).map_err(|error| format!("{field}: {error}"))
        })
        .transpose()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_review_schema_exposes_commit_and_revision_reads() {
        let tools = tools();
        let schema = &tools
            .iter()
            .find(|tool| tool["name"] == "read_review")
            .unwrap()["inputSchema"]["properties"];
        for mode in ["commits", "commit", "since"] {
            assert!(schema["mode"]["enum"]
                .as_array()
                .unwrap()
                .contains(&json!(mode)));
        }
        for field in ["commit", "from_snapshot_id", "limit", "cursor"] {
            assert!(schema.get(field).is_some(), "missing {field}");
        }
    }

    #[test]
    fn read_review_new_modes_preserve_typed_options_and_refuse_invalid_combinations() {
        use super::super::McpSurface;
        for (mode, extra) in [
            ("commits", json!({"limit":2,"cursor":"page"})),
            (
                "commit",
                json!({"commit":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","paths":["src/lib.rs"]}),
            ),
            ("since", json!({"from_snapshot_id":"snap-1"})),
        ] {
            let mut args = json!({"task_id":"task-1","snapshot_id":"snap-2","directory_id":"dir-1","mode":mode});
            args.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            let action = call(McpSurface::Coding, "read_review", args)
                .action
                .unwrap();
            let encoded = serde_json::to_value(action).unwrap();
            for (key, value) in extra.as_object().unwrap() {
                assert_eq!(&encoded[key], value);
            }
        }
        for extra in [
            json!({"mode":"commit"}),
            json!({"mode":"since"}),
            json!({"mode":"commits","limit":0}),
            json!({"mode":"changes","commit":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}),
        ] {
            let mut args =
                json!({"task_id":"task-1","snapshot_id":"snap-2","directory_id":"dir-1"});
            args.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            assert!(call(McpSurface::Coding, "read_review", args)
                .action
                .is_none());
        }
    }

    const PR_CALLS: &[(&str, &str)] = &[
        (
            "open_review",
            r#"{"workspace_id":"ws-1","request_id":"open-1","title":"Review API","description":"Committed API change","bases":[{"directory_id":"dir-1","branch":"main"}],"excluded_git_directory_ids":[],"reviewer":{"kind":"agent","agent_id":"agent-1"}}"#,
        ),
        (
            "push_review",
            r#"{"task_id":"task-1","expected_version":1,"sources":[{"directory_id":"dir-1","expected_head":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","expected_received_head":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","force_with_lease":true}]}"#,
        ),
        (
            "merge_review",
            r#"{"task_id":"task-1","expected_version":1,"snapshot_id":"snap-1","sources":[{"directory_id":"dir-1","expected_base_head":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","push":{"remote":"origin","branch":"main"}}]}"#,
        ),
        (
            "close_review",
            r#"{"task_id":"task-1","expected_version":1,"description":"Superseded"}"#,
        ),
        (
            "reopen_review",
            r#"{"task_id":"task-1","expected_version":1}"#,
        ),
        (
            "refresh_review",
            r#"{"task_id":"task-1","expected_version":1}"#,
        ),
        (
            "update_review_base",
            r#"{"task_id":"task-1","expected_version":1,"bases":[{"directory_id":"dir-1","branch":"dev"}]}"#,
        ),
        (
            "retry_review_delivery",
            r#"{"task_id":"task-1","comment_id":"comment-1"}"#,
        ),
    ];

    fn call(surface: super::super::McpSurface, name: &str, args: Value) -> Handled {
        let owner = match surface {
            super::super::McpSurface::Coding => "agent-1",
            super::super::McpSurface::Project => "project-1",
            super::super::McpSurface::Router => "router-1",
        };
        super::super::DoneServer::for_owner(owner).handle_message(
            &json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":name,"arguments":args}}).to_string(),
        )
    }

    #[test]
    fn open_review_advertises_and_preserves_optional_work_task_link() {
        let listed = tools();
        let schema = &listed
            .iter()
            .find(|tool| tool["name"] == "open_review")
            .unwrap()["inputSchema"];
        assert_eq!(schema["properties"]["for_task_id"]["type"], "string");
        assert!(!schema["required"]
            .as_array()
            .unwrap()
            .contains(&json!("for_task_id")));
        let mut args: Value = serde_json::from_str(PR_CALLS[0].1).unwrap();
        args["for_task_id"] = json!("task-work-1");
        let action = call(
            super::super::McpSurface::Coding,
            "open_review",
            args.clone(),
        )
        .action
        .unwrap();
        let encoded = serde_json::to_value(action).unwrap();
        assert_eq!(encoded["params"]["for_task_id"], "task-work-1");
        args["for_task_id"] = json!("");
        assert!(call(super::super::McpSurface::Coding, "open_review", args)
            .action
            .is_none());
    }

    #[test]
    fn pr_tools_share_project_bound_surfaces_and_typed_actions() {
        use super::super::{DoneServer, McpSurface};
        for &(name, raw) in PR_CALLS {
            for surface in [McpSurface::Coding, McpSurface::Project] {
                assert!(
                    DoneServer::tool_names_of(surface).contains(&name.to_string()),
                    "{name}"
                );
                let action = call(surface, name, serde_json::from_str(raw).unwrap())
                    .action
                    .unwrap();
                assert_eq!(action.tool_name(), name);
                assert!(action.allowed_on(surface));
                assert!(!action.allowed_on(McpSurface::Router));
                let encoded = serde_json::to_value(&action).unwrap();
                let decoded: BridgeAction = serde_json::from_value(encoded).unwrap();
                assert_eq!(action, decoded);
            }
            assert!(!DoneServer::tool_names_of(McpSurface::Router).contains(&name.to_string()));
        }
    }

    #[test]
    fn pr_tools_refuse_unknown_identity_and_nested_parameters() {
        use super::super::McpSurface;
        for &(name, raw) in PR_CALLS {
            let original: Value = serde_json::from_str(raw).unwrap();
            for field in [
                "author",
                "actor",
                "project_id",
                "path",
                "source_path",
                "workspace_path",
            ] {
                let mut args = original.clone();
                args[field] = json!("forged");
                assert!(
                    call(McpSurface::Coding, name, args).action.is_none(),
                    "{name} accepted {field}"
                );
            }
        }
        for (name, raw, pointer) in [
            ("open_review", PR_CALLS[0].1, "/reviewer"),
            ("open_review", PR_CALLS[0].1, "/bases/0"),
            ("push_review", PR_CALLS[1].1, "/sources/0"),
            ("merge_review", PR_CALLS[2].1, "/sources/0"),
            ("merge_review", PR_CALLS[2].1, "/sources/0/push"),
            ("update_review_base", PR_CALLS[6].1, "/bases/0"),
        ] {
            let mut args: Value = serde_json::from_str(raw).unwrap();
            args.pointer_mut(pointer).unwrap()["path"] = json!("/tmp/forged");
            assert!(
                call(McpSurface::Coding, name, args).action.is_none(),
                "{name} accepted {pointer}/path"
            );
        }
    }

    #[test]
    fn pr_tool_schemas_describe_strict_ids_and_explicit_leases() {
        for &(name, _) in PR_CALLS {
            let listed = tools();
            let schema = &listed.iter().find(|tool| tool["name"] == name).unwrap()["inputSchema"];
            assert_eq!(schema["additionalProperties"], false, "{name}");
            for field in [
                "author",
                "actor",
                "project_id",
                "path",
                "source_path",
                "workspace_path",
            ] {
                assert!(
                    schema["properties"].get(field).is_none(),
                    "{name} advertises {field}"
                );
            }
        }
        let listed = tools();
        let push = &listed
            .iter()
            .find(|tool| tool["name"] == "push_review")
            .unwrap()["inputSchema"]["properties"]["sources"]["items"];
        assert_eq!(push["additionalProperties"], false);
        assert!(push["required"]
            .as_array()
            .unwrap()
            .contains(&json!("expected_head")));
        let merge = &listed
            .iter()
            .find(|tool| tool["name"] == "merge_review")
            .unwrap()["inputSchema"]["properties"]["sources"]["items"];
        assert!(merge["required"]
            .as_array()
            .unwrap()
            .contains(&json!("expected_base_head")));
    }

    #[test]
    fn pr_tool_prompts_reach_coding_and_project_agents_with_publication_rules() {
        let templates = crate::templates::Templates::default();
        for prompt in [&templates.build, &templates.project_agent] {
            for &(name, _) in PR_CALLS {
                assert!(prompt.contains(name), "prompt omits {name}");
            }
            for rule in [
                "native `git push`",
                "plain `git push --force`",
                "opinions on older snapshots",
                "branch bindings",
            ] {
                assert!(prompt.contains(rule), "prompt omits {rule}");
            }
        }
        let workspace = include_str!("../../templates/notes/workspace.md")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        assert!(workspace.contains("open_review"));
        assert!(workspace.contains("retained workspace"));
    }

    #[test]
    fn pr_tools_refuse_malformed_leases_and_duplicate_selections() {
        use super::super::McpSurface;
        for (index, pointer, invalid) in [
            (0, "/workspace_id", json!(" ")),
            (0, "/request_id", json!("a".repeat(201))),
            (0, "/title", json!("Two\nlines")),
            (0, "/reviewer/agent_id", json!("")),
            (1, "/expected_version", json!(-1)),
            (1, "/sources/0/expected_head", json!("abc")),
            (1, "/sources/0/expected_received_head", json!("")),
            (1, "/sources/0/force_with_lease", json!("true")),
            (2, "/sources/0/expected_base_head", json!("not-a-head")),
            (2, "/sources/0/push/branch", json!("main~1")),
            (3, "/description", json!("é".repeat(1_001))),
            (4, "/task_id", json!("")),
            (5, "/expected_version", json!(1.5)),
            (6, "/bases/0/branch", json!("main~1")),
            (7, "/task_id", json!("")),
            (7, "/comment_id", json!("x".repeat(201))),
        ] {
            let (name, raw) = PR_CALLS[index];
            let mut args: Value = serde_json::from_str(raw).unwrap();
            *args.pointer_mut(pointer).unwrap() = invalid;
            assert!(
                call(McpSurface::Coding, name, args).action.is_none(),
                "{name} accepted {pointer}"
            );
        }
        for (index, field) in [(0, "bases"), (1, "sources"), (2, "sources"), (6, "bases")] {
            let (name, raw) = PR_CALLS[index];
            let mut args: Value = serde_json::from_str(raw).unwrap();
            let selection = args[field][0].clone();
            args[field] = json!([selection, selection]);
            assert!(
                call(McpSurface::Coding, name, args).action.is_none(),
                "{name} accepted duplicate {field}"
            );
        }
    }
}

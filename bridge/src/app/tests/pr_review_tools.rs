//! PR tools retain the authenticated actor and fence project identities before
//! admitting deferred filesystem or Git work.

use super::project_agent::{added_project, project_agent, workspace};
use super::tracker::{filed, tracked};
use super::tracker_tools::coding_agent;
use super::*;
use crate::mcp::{BridgeAction, DoneServer};

fn action(agent: &str, name: &str, arguments: Value) -> BridgeAction {
    DoneServer::for_owner(agent)
        .handle_message(&json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":name,"arguments":arguments}}).to_string())
        .action
        .unwrap_or_else(|| panic!("{name} must parse"))
}

fn open_arguments(state: &AppState, workspace_id: &str, request_id: &str) -> Value {
    let workspace = state.workspaces.get(workspace_id).unwrap();
    json!({
        "workspace_id":workspace_id, "request_id":request_id,
        "title":"PR tool fixture", "description":"Committed work for review",
        "bases":workspace.directories.iter().filter(|directory| directory.is_git).map(|directory| json!({"directory_id":directory.id,"branch":"main"})).collect::<Vec<_>>(),
        "excluded_git_directory_ids":[],
    })
}

#[test]
fn pr_tools_fence_foreign_projects_before_deferred_admission_on_both_surfaces() {
    let tmp = tempfile::tempdir().unwrap();
    let (_repo, mut state, project) = tracked(tmp.path());
    let callers = [
        project_agent(&mut state, &project),
        coding_agent(&mut state, &project, "caller"),
    ];
    let (_foreign_home, foreign_repo) = init_repo();
    let foreign_project = added_project(&mut state, &foreign_repo);
    let foreign_workspace = workspace(&mut state, &foreign_project, "foreign");
    let foreign_task = filed(&mut state, &foreign_project, "Private task");
    for (owner, agent) in callers {
        let open = action(
            &agent,
            "open_review",
            open_arguments(&state, &foreign_workspace, "foreign-open"),
        );
        let refused = state.agent_action(&owner, &agent, open).unwrap_err();
        assert!(refused.starts_with("unknown workspace_id"), "{refused}");
        assert!(state.deferred_work.is_none());
        for (name, extra) in [
            (
                "push_review",
                json!({"sources":[{"directory_id":"dir-1","expected_head":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","force_with_lease":false}]}),
            ),
            (
                "merge_review",
                json!({"snapshot_id":"snap-1","sources":[{"directory_id":"dir-1","expected_base_head":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}]}),
            ),
            ("close_review", json!({"description":"Closed"})),
            ("reopen_review", json!({})),
            ("refresh_review", json!({})),
            (
                "update_review_base",
                json!({"bases":[{"directory_id":"dir-1","branch":"dev"}]}),
            ),
        ] {
            let mut arguments = extra;
            arguments["task_id"] = foreign_task["id"].clone();
            arguments["expected_version"] = json!(1);
            let action = action(&agent, name, arguments);
            let refused = state.agent_action(&owner, &agent, action).unwrap_err();
            assert!(refused.starts_with("unknown task_id"), "{name}: {refused}");
            assert!(state.deferred_work.is_none(), "{name} admitted work");
        }
    }
    assert!(state
        .store
        .as_ref()
        .unwrap()
        .load_review(foreign_task["id"].as_str().unwrap())
        .unwrap()
        .is_none());
}

#[test]
fn pr_opening_and_terminal_tools_keep_the_authenticated_agent() {
    for coding in [false, true] {
        let tmp = tempfile::tempdir().unwrap();
        let (_repo, mut state, project) = tracked(tmp.path());
        let (owner, agent) = if coding {
            coding_agent(&mut state, &project, "caller")
        } else {
            project_agent(&mut state, &project)
        };
        let workspace_id = workspace(&mut state, &project, "review");
        let open = action(
            &agent,
            "open_review",
            open_arguments(&state, &workspace_id, "open-actor"),
        );
        let opened = state.agent_action(&owner, &agent, open).unwrap();
        let review = &opened["review"];
        assert_eq!(
            review["pull_request"]["creator"]["agent_id"], agent,
            "{opened}"
        );
        assert_eq!(
            review["snapshots"][0]["author"]["agent_id"], agent,
            "{opened}"
        );
        let task_id = review["task_id"].as_str().unwrap();
        let close = action(
            &agent,
            "close_review",
            json!({"task_id":task_id,"expected_version":review["version"],"description":"Superseded"}),
        );
        let closed = state.agent_action(&owner, &agent, close).unwrap();
        assert_eq!(closed["review"]["pull_request"]["status"], "closed");
        let reopen = action(
            &agent,
            "reopen_review",
            json!({"task_id":task_id,"expected_version":closed["review"]["version"]}),
        );
        let reopened = state.agent_action(&owner, &agent, reopen).unwrap();
        assert_eq!(reopened["review"]["pull_request"]["status"], "open");
        let task = state.handle(req("tasks.get", json!({"task_id":task_id})));
        for kind in ["review_completed", "reopened"] {
            let event = task["result"]["timeline"]
                .as_array()
                .unwrap()
                .iter()
                .find(|event| event["kind"] == kind)
                .unwrap();
            assert_eq!(event["actor"]["agent_id"], agent, "{task}");
        }
    }
}

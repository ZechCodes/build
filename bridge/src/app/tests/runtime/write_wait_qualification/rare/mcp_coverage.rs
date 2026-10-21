//! Semantic MCP variants need socket/effect proofs in addition to RPC98.
use super::*;
use std::collections::BTreeSet;

const WRITER_ACTIONS: &[&str] = &[
    "post_thread_message",
    "set_topic",
    "set_name",
    "compact_self",
    "compact_agent",
    "dispatch_branch",
    "ask_user",
    "router_message",
    "create_workspace",
    "add_workspace_agent",
    "add_project_agent",
    "remove_workspace_agent",
    "message_agent",
    "message_workspace_agent",
    "delete_workspace",
    "add_project_source",
    "remove_project_source",
    "reclaim_workspace",
    "add_workspace_directory",
    "remove_workspace_directory",
    "tracker_create_task",
    "tracker_with_waiting_on",
    "tracker_unlink_waiting_on",
    "tracker_retry_task_delivery",
    "tracker_comment_task",
    "tracker_review_comment_task",
    "tracker_assign_task",
    "tracker_move_task",
    "tracker_label_task",
    "tracker_close_task",
    "tracker_track_task",
    "tracker_untrack_task",
    "tracker_link_task",
    "tracker_snapshot_review",
    "tracker_act_review",
    "tracker_complete_review",
    "tracker_open_review",
    "tracker_push_review",
    "tracker_update_review_base",
    "tracker_merge_review",
    "tracker_close_review",
    "tracker_reopen_review",
    "tracker_refresh_review",
];

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_semantic_mcp_action_has_a_real_socket_effect_and_terminal_origin() {
    crate::git_fixture::environment::isolated_git_test!();
    let mut steps = super::mcp_tracker::execute().await;
    steps.extend(super::mcp_lifecycle::execute().await);
    steps.extend(super::mcp_compaction::execute().await);
    steps.extend(super::mcp_common::execute().await);
    steps.extend(super::mcp_router::execute().await);
    let observed: BTreeSet<_> = steps
        .iter()
        .map(|step| {
            assert_eq!(step.reply["ok"], true);
            step.mcp_action
                .as_ref()
                .expect("the actual socket action was captured")["action"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    assert_eq!(
        observed,
        WRITER_ACTIONS
            .iter()
            .map(|name| (*name).to_owned())
            .collect()
    );
}

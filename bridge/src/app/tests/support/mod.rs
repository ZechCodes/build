mod sessions;
pub(in crate::app::tests) use sessions::*;
mod workflow;
pub(in crate::app::tests) use workflow::*;

pub(in crate::app::tests) use super::board::attention::{
    attention_of, board_entry, push_to_issue_conversation,
};
pub(in crate::app::tests) use super::board::mute_dismiss::{commit_in, work_item_row_for};
pub(in crate::app::tests) use super::conversations::attachments::ONE_PIXEL_PNG;
pub(in crate::app::tests) use super::filesystem::{add_external_worktree, local_branch_exists};
pub(in crate::app::tests) use super::git::repository::{
    init_repo_with_origin, origin_with_pushed_branch,
};
pub(in crate::app::tests) use super::git::status::{file_entry, git_gui_state, has_file_entry};
pub(in crate::app::tests) use super::protocol::activity::{
    activity_of, activity_rows, run_on_a_headless_provider,
};
pub(in crate::app::tests) use super::protocol::session::{tool_call_rows, tool_calls_of};
pub(in crate::app::tests) use super::protocol::status::{
    insert_agent_tab, insert_dictated_agent_tab,
};
pub(in crate::app::tests) use super::push::{change_events, greeted_push_session, settled_pushes};
pub(in crate::app::tests) use super::routing::captures::capture_rows;
pub(in crate::app::tests) use super::routing::router::{capture_record, captured};
pub(in crate::app::tests) use super::rtc::{offer, signaling_fixture};
pub(in crate::app::tests) use super::runtime::agent_tabs::{
    agent_tab_fixture, agent_tab_fixture_at, open_session_count, wait_for_agent_screen,
};
pub(in crate::app::tests) use super::runtime::delivery::{
    open_a_turn, primary_thread, primary_thread_mut,
};
pub(in crate::app::tests) use super::runtime::frame_locks::{
    gated_agent_role, gated_tab, on_the_terminal_provider, shared_state_and_handler,
    spawns_parked_at, GatedHarness,
};
pub(in crate::app::tests) use super::runtime::idle_sessions::{
    approved_side_plan, dispatch_side_run, fake_run_record, insert_run, insert_run_with_agent_tab,
    test_agent_session_request, warm_tui_spec, QUIET_THRESHOLD,
};
pub(in crate::app::tests) use super::runtime::run_agents::{
    insert_live_run, insert_plan_without_agent, insert_run_without_agent,
    insert_unmanaged_agent_tab,
};
pub(in crate::app::tests) use super::runtime::terminal_lifecycle::{
    a_headless_provider_running, a_provider_running,
};
pub(in crate::app::tests) use super::runtime::terminals::{
    output_text, process_reaped, settles, wait_for_push, wait_for_pushes,
};
pub(in crate::app::tests) use super::workflow::branch_feed::{branch_row, work_item_rows};
pub(in crate::app::tests) use super::workflow::finish::external_id;
pub(in crate::app::tests) use super::workflow::lifecycle_offlock::{
    frame_on_a_thread, pending_on_the_board,
};

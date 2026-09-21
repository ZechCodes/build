//! A harness out of usage, from the snapshot that says so to the turn that
//! runs once it lifts (issue #58).
//!
//! The session's own report is pinned in `harness/adk`; what these pin is what
//! the device does with it: one record per harness on the board, no turn
//! started while it holds, the conversation told once, and the agent whose turn
//! died at the limit started again when it lifts.

use super::resume::standing;
use super::*;
use crate::harness::usage_limit::UsageLimited;
use crate::harness::{AgentStatus, SessionStatusSnapshot};
use time::OffsetDateTime;

const SAID: &str = "You've hit your session limit · resets 6:20pm (America/New_York)";

fn limited(resets_at: Option<OffsetDateTime>) -> SessionStatusSnapshot {
    SessionStatusSnapshot::new(AgentStatus::Waiting)
        .limited(UsageLimited {
            said: SAID.to_string(),
            resets_at,
        })
        .expect("a limit is news to a fresh snapshot")
}

fn working() -> SessionStatusSnapshot {
    SessionStatusSnapshot::new(AgentStatus::Working)
}

fn an_hour_from_now() -> OffsetDateTime {
    OffsetDateTime::now_utc() + time::Duration::hours(1)
}

fn usage_limits(state: &mut AppState) -> Vec<Value> {
    let board = state.handle(req("board.list", json!({})));
    board["result"]["usage_limits"]
        .as_array()
        .unwrap_or_else(|| panic!("board.list carries usage_limits: {board:?}"))
        .clone()
}

/// Build's own notices on a conversation that mention `needle`.
fn build_notices(state: &mut AppState, run_id: &str, agent_id: &str, needle: &str) -> Vec<String> {
    super::project_agent::items(state, run_id, agent_id)
        .into_iter()
        .filter(|item| item["data"]["from_build"] == json!(true))
        .filter_map(|item| item["data"]["body"].as_str().map(str::to_string))
        .filter(|body| body.contains(needle))
        .collect()
}

fn post(state: &mut AppState, run_id: &str, agent_id: &str, body: &str) {
    let posted = state.handle(req(
        "thread.post",
        json!({ "entity_id": run_id, "agent_id": agent_id, "body": body }),
    ));
    assert_eq!(posted["ok"], true, "{posted:?}");
}

fn provider_of(state: &AppState, run_id: &str, agent_id: &str) -> crate::models::AgentProvider {
    state
        .entity_agents(run_id)
        .unwrap()
        .by_id(agent_id)
        .unwrap()
        .choice
        .provider
}

#[test]
fn a_turn_that_stopped_at_the_limit_puts_the_harness_on_the_board() {
    let mut standing = standing();
    let (state, _root, run_id, agent_id) = standing.parts();
    assert_eq!(
        usage_limits(state),
        Vec::<Value>::new(),
        "none to start with"
    );

    let resets_at = an_hour_from_now();
    let mut recorded = None;
    assert!(state.record_usage_limit(&run_id, &agent_id, &limited(Some(resets_at)), &mut recorded));

    let limits = usage_limits(state);
    assert_eq!(limits.len(), 1, "{limits:?}");
    let harness = provider_of(state, &run_id, &agent_id).wire_id();
    assert_eq!(limits[0]["harness"], harness);
    assert_eq!(limits[0]["said"], SAID);
    assert!(limits[0]["since"].is_string());
    let wire_reset = limits[0]["resets_at"].as_str().unwrap();
    assert_eq!(
        OffsetDateTime::parse(wire_reset, &time::format_description::well_known::Rfc3339).unwrap(),
        resets_at,
        "an RFC 3339 instant, so the client does no zone arithmetic"
    );
}

#[test]
fn the_same_limit_on_every_later_snapshot_is_recorded_once() {
    let mut standing = standing();
    let (state, _root, run_id, agent_id) = standing.parts();
    let snapshot = limited(Some(an_hour_from_now()));
    let mut recorded = None;
    assert!(state.record_usage_limit(&run_id, &agent_id, &snapshot, &mut recorded));
    assert!(
        !state.record_usage_limit(&run_id, &agent_id, &snapshot, &mut recorded),
        "an idle session carries its limit on every snapshot until a turn runs"
    );
}

#[test]
fn a_message_while_limited_stays_queued_and_the_conversation_is_told_once() {
    let mut standing = standing();
    let (state, _root, run_id, agent_id) = standing.parts();
    state.delivery_queue.clear_queued();
    let mut recorded = None;
    state.record_usage_limit(
        &run_id,
        &agent_id,
        &limited(Some(an_hour_from_now())),
        &mut recorded,
    );

    post(state, &run_id, &agent_id, "the first thing");
    assert!(
        state.take_pending_turns().is_empty(),
        "no turn starts on a harness out of usage"
    );
    assert_eq!(state.delivery_queue.queued_len(), 1, "it stays queued");

    post(state, &run_id, &agent_id, "one more thing");
    assert!(state.take_pending_turns().is_empty());
    assert_eq!(
        state.delivery_queue.queued_len(),
        1,
        "the second rides the turn already waiting, as it does for any agent"
    );

    let told = build_notices(state, &run_id, &agent_id, "run out of usage");
    assert_eq!(told.len(), 1, "told once, however many wait: {told:?}");
    assert!(
        told[0].contains(SAID),
        "in the harness's own words: {}",
        told[0]
    );
    assert!(
        told[0].contains("delivered in order when it resets"),
        "{}",
        told[0]
    );
}

#[test]
fn when_the_reset_passes_the_queued_turns_go_in_order() {
    let mut standing = standing();
    let (state, _root, run_id, agent_id) = standing.parts();
    state.delivery_queue.clear_queued();
    let resets_at = an_hour_from_now();
    let mut recorded = None;
    state.record_usage_limit(&run_id, &agent_id, &limited(Some(resets_at)), &mut recorded);
    post(state, &run_id, &agent_id, "the first thing");
    post(state, &run_id, &agent_id, "one more thing");
    assert!(state.take_pending_turns().is_empty());

    state.release_usage_limits_due_at(resets_at);

    let mut taken = state.take_pending_turns();
    let (turn, _mark) = taken.next_turn().expect("the queued turn goes");
    assert_eq!(turn.agent_id, agent_id);
    let cold = &turn.said().cold;
    let first = cold
        .find("user: the first thing")
        .expect("the first message");
    let second = cold
        .find("user: one more thing")
        .expect("the second message");
    assert!(first < second, "in the order sent: {cold}");
    assert_eq!(state.delivery_queue.queued_len(), 0);
    assert_eq!(
        usage_limits(state).len(),
        1,
        "released is not cleared: the banner goes when a turn runs"
    );
}

#[test]
fn an_agent_whose_turn_died_at_the_limit_is_started_again_when_it_lifts() {
    let mut standing = standing();
    let (state, _root, run_id, agent_id) = standing.parts();
    state.delivery_queue.clear_queued();
    let resets_at = an_hour_from_now();
    let mut recorded = None;
    state.record_usage_limit(&run_id, &agent_id, &limited(Some(resets_at)), &mut recorded);
    assert_eq!(state.delivery_queue.queued_len(), 0);

    state.release_usage_limits_due_at(resets_at - time::Duration::seconds(1));
    assert_eq!(state.delivery_queue.queued_len(), 0, "not before its reset");

    state.release_usage_limits_due_at(resets_at);
    let told = build_notices(state, &run_id, &agent_id, "usage limit has reset");
    assert_eq!(told.len(), 1, "{told:?}");
    assert!(
        told[0].contains("pick up where you left off"),
        "{}",
        told[0]
    );
    assert_eq!(state.delivery_queue.queued_len(), 1, "a turn to carry it");
    let queued = state.delivery_queue.queued_nth(0).unwrap();
    assert_eq!(queued.agent_id, agent_id);
    assert_eq!(queued.phase, "resume");

    state.release_usage_limits_due_at(resets_at + time::Duration::minutes(5));
    assert_eq!(state.delivery_queue.queued_len(), 1, "started once");
}

#[test]
fn an_agent_with_a_message_already_waiting_is_told_but_not_given_a_second_turn() {
    let mut standing = standing();
    let (state, _root, run_id, agent_id) = standing.parts();
    state.delivery_queue.clear_queued();
    let resets_at = an_hour_from_now();
    let mut recorded = None;
    state.record_usage_limit(&run_id, &agent_id, &limited(Some(resets_at)), &mut recorded);
    post(state, &run_id, &agent_id, "are you still on this?");

    state.release_usage_limits_due_at(resets_at);

    assert_eq!(
        build_notices(state, &run_id, &agent_id, "usage limit has reset").len(),
        1
    );
    assert_eq!(
        state.delivery_queue.queued_len(),
        1,
        "the waiting message starts it"
    );
}

#[test]
fn a_turn_running_on_the_harness_clears_the_limit() {
    let mut standing = standing();
    let (state, _root, run_id, agent_id) = standing.parts();
    let mut recorded = None;
    state.record_usage_limit(
        &run_id,
        &agent_id,
        &limited(Some(an_hour_from_now())),
        &mut recorded,
    );
    assert_eq!(usage_limits(state).len(), 1);

    state.record_usage_limit(&run_id, &agent_id, &working(), &mut recorded);

    assert_eq!(usage_limits(state), Vec::<Value>::new());
    assert!(!state.usage_limits.stopped(&run_id, &agent_id));
    // And a limit the SAME session reports again afterwards is news again.
    assert!(state.record_usage_limit(
        &run_id,
        &agent_id,
        &limited(Some(an_hour_from_now())),
        &mut recorded
    ));
}

#[test]
fn no_reset_named_holds_nothing_but_still_says_so_on_the_board() {
    let mut standing = standing();
    let (state, _root, run_id, agent_id) = standing.parts();
    state.delivery_queue.clear_queued();
    let mut recorded = None;
    state.record_usage_limit(&run_id, &agent_id, &limited(None), &mut recorded);
    let limits = usage_limits(state);
    assert_eq!(limits[0]["resets_at"], Value::Null, "reset time unknown");

    post(state, &run_id, &agent_id, "try again?");
    let mut taken = state.take_pending_turns();
    assert!(
        taken.next_turn().is_some(),
        "with nothing to wait for, the next message is how the limit is found over"
    );
    assert!(build_notices(state, &run_id, &agent_id, "run out of usage").is_empty());
}

#[test]
fn the_board_item_carries_the_list_when_it_moves() {
    let mut standing = standing();
    let (state, _root, run_id, agent_id) = standing.parts();
    let facts = state.board_lists(crate::changes::BoardLists::USAGE_LIMITS);
    assert_eq!(facts.render(), json!({ "usage_limits": [] }));

    let mut recorded = None;
    state.record_usage_limit(&run_id, &agent_id, &limited(None), &mut recorded);
    let rendered = state
        .board_lists(crate::changes::BoardLists::USAGE_LIMITS)
        .render();
    assert_eq!(rendered["usage_limits"][0]["said"], SAID, "{rendered}");
    assert_eq!(
        state
            .board_lists(crate::changes::BoardLists::PROJECTS)
            .render()
            .get("usage_limits"),
        None,
        "and only when it moved"
    );
}

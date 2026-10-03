//! Usage reports inform the board without holding user messages. Known resets
//! schedule one retry, and only a successful response clears the report.

use super::resume::standing;
use super::*;
use crate::harness::usage_limit::UsageLimited;
use crate::harness::{AgentStatus, SessionStatusSnapshot};
use time::OffsetDateTime;

const SAID: &str = "You've hit your session limit · resets 6:20pm (America/New_York)";

fn limited(resets_at: Option<OffsetDateTime>) -> SessionStatusSnapshot {
    SessionStatusSnapshot::new(AgentStatus::Waiting).limited(UsageLimited {
        said: SAID.to_string(),
        resets_at,
    })
}

fn successful() -> SessionStatusSnapshot {
    SessionStatusSnapshot::new(AgentStatus::Waiting).successful_response()
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
fn internal_wake_retries_a_new_usage_limit_after_clear() {
    let mut standing = standing();
    let (state, _, owner, agent) = standing.parts();
    super::resume::clear_conversation(state, &owner, &agent);
    let resets_at = an_hour_from_now();
    let mut observation = Default::default();
    state.record_usage_limit(&owner, &agent, &limited(Some(resets_at)), &mut observation);
    state.release_usage_limits_due_at(resets_at);
    assert_eq!(build_notices(state, &owner, &agent, "Retrying").len(), 1);
    assert_eq!(state.delivery_queue.queued_len(), 1);
}

#[test]
fn internal_wake_does_not_retry_a_usage_limit_observed_before_clear() {
    let mut standing = standing();
    let (state, _, owner, agent) = standing.parts();
    let resets_at = an_hour_from_now();
    let mut observation = Default::default();
    state.record_usage_limit(&owner, &agent, &limited(Some(resets_at)), &mut observation);
    super::resume::clear_conversation(state, &owner, &agent);
    state.release_usage_limits_due_at(resets_at);
    assert!(state
        .agent_conversation(&owner, Some(&agent))
        .unwrap()
        .items
        .is_empty());
    assert!(state.delivery_queue.queued_is_empty());
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
    let mut recorded = Default::default();
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
    let mut recorded = Default::default();
    assert!(state.record_usage_limit(&run_id, &agent_id, &snapshot, &mut recorded));
    assert!(
        !state.record_usage_limit(&run_id, &agent_id, &snapshot, &mut recorded),
        "an idle session carries its limit on every snapshot until a response succeeds"
    );
}

#[test]
fn a_message_on_the_same_model_is_ready_before_the_reported_reset() {
    let mut standing = standing();
    let (state, _root, run_id, agent_id) = standing.parts();
    state.delivery_queue.clear_queued();
    let mut recorded = Default::default();
    state.record_usage_limit(
        &run_id,
        &agent_id,
        &limited(Some(an_hour_from_now())),
        &mut recorded,
    );

    post(state, &run_id, &agent_id, "try again after I added credits");
    let mut taken = state.take_pending_turns();
    let (turn, mark) = taken
        .next_turn()
        .expect("a remembered limit never holds a message");
    assert_eq!(turn.agent_id, agent_id);
    assert_eq!(state.delivery_queue.queued_len(), 0);
    mark.settle(state);
    assert!(
        state.take_pending_turns().is_empty(),
        "no automatic retry loop"
    );
    assert!(build_notices(state, &run_id, &agent_id, "stay queued").is_empty());
    assert_eq!(
        usage_limits(state).len(),
        1,
        "the report remains informative"
    );
}

fn post_operation(
    state: &mut AppState,
    run_id: &str,
    agent_id: &str,
    operation_id: &str,
    revision: u64,
) {
    let agent = state
        .entity_agents(run_id)
        .unwrap()
        .by_id(agent_id)
        .unwrap();
    let posted = state.handle(req(
        "thread.post",
        json!({
            "entity_id": run_id,
            "agent_id": agent_id,
            "conversation_id": agent.conversation_id(),
            "operation_id": operation_id,
            "body": operation_id,
            "choice_revision": revision,
        }),
    ));
    assert_eq!(posted["ok"], true, "{posted:?}");
}

#[test]
fn switching_from_fable_to_opus_delivers_both_operations_in_order_before_reset() {
    let mut standing = standing();
    let (state, _root, run_id, agent_id) = standing.parts();
    state.delivery_queue.clear_queued();
    let fable = crate::models::ModelChoice {
        provider: crate::models::AgentProvider::ClaudeAdk,
        model: Some("claude-fable-5".into()),
        ..Default::default()
    };
    state
        .set_agent_model_choice(&run_id, &agent_id, fable.clone())
        .unwrap();
    let first_revision = state
        .entity_agents(&run_id)
        .unwrap()
        .by_id(&agent_id)
        .unwrap()
        .choice_revision;
    let mut recorded = Default::default();
    state.record_usage_limit(
        &run_id,
        &agent_id,
        &limited(Some(an_hour_from_now())),
        &mut recorded,
    );
    post_operation(state, &run_id, &agent_id, "before-switch", first_revision);
    let opus = crate::models::ModelChoice {
        model: Some("claude-opus-5".into()),
        ..fable.clone()
    };
    state
        .set_agent_model_choice(&run_id, &agent_id, opus.clone())
        .unwrap();
    let second_revision = state
        .entity_agents(&run_id)
        .unwrap()
        .by_id(&agent_id)
        .unwrap()
        .choice_revision;
    post_operation(state, &run_id, &agent_id, "after-switch", second_revision);

    let mut taken = state.take_pending_turns();
    let (first, first_mark) = taken
        .next_turn()
        .expect("the older Fable operation is ready");
    let (second, second_mark) = taken.next_turn().expect("the Opus operation is ready");
    assert_eq!(first.operation_id.as_deref(), Some("before-switch"));
    assert_eq!(first.model_choice, fable);
    assert_eq!(second.operation_id.as_deref(), Some("after-switch"));
    assert_eq!(second.model_choice, opus);
    assert!(taken.is_empty());
    assert_eq!(state.delivery_queue.queued_len(), 0);
    first_mark.settle(state);
    second_mark.settle(state);
}

#[test]
fn usage_reports_do_not_bypass_pending_rows_or_in_flight_delivery() {
    let mut standing = standing();
    let (state, _root, run_id, agent_id) = standing.parts();
    state.delivery_queue.clear_queued();
    let mut recorded = Default::default();
    state.record_usage_limit(
        &run_id,
        &agent_id,
        &limited(Some(an_hour_from_now())),
        &mut recorded,
    );
    post_operation(state, &run_id, &agent_id, "first", 0);
    state.pending_rows.push(
        crate::lifecycle::PendingRow::creating(run_id.clone(), None, "pending".into()).into(),
    );
    assert!(state.take_pending_turns().is_empty());
    assert_eq!(state.delivery_queue.queued_len(), 1);
    state.pending_rows.clear();
    let mut taken = state.take_pending_turns();
    let (_, mark) = taken.next_turn().expect("ordinary blocker is gone");
    post_operation(state, &run_id, &agent_id, "second", 0);
    assert!(
        state.take_pending_turns().is_empty(),
        "an in-flight delivery still holds its agent"
    );
    mark.settle(state);
    assert!(state.take_pending_turns().next_turn().is_some());
}

#[test]
fn messages_queued_before_reset_keep_their_order() {
    let mut standing = standing();
    let (state, _root, run_id, agent_id) = standing.parts();
    state.delivery_queue.clear_queued();
    let resets_at = an_hour_from_now();
    let mut recorded = Default::default();
    state.record_usage_limit(&run_id, &agent_id, &limited(Some(resets_at)), &mut recorded);
    post(state, &run_id, &agent_id, "the first thing");
    post(state, &run_id, &agent_id, "one more thing");
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
        "delivery does not clear the report: a response must succeed"
    );
}

#[test]
fn an_agent_whose_turn_died_at_the_limit_is_started_again_when_it_lifts() {
    let mut standing = standing();
    let (state, _root, run_id, agent_id) = standing.parts();
    state.delivery_queue.clear_queued();
    let resets_at = an_hour_from_now();
    let mut recorded = Default::default();
    state.record_usage_limit(&run_id, &agent_id, &limited(Some(resets_at)), &mut recorded);
    assert_eq!(state.delivery_queue.queued_len(), 0);

    state.release_usage_limits_due_at(resets_at - time::Duration::seconds(1));
    assert_eq!(state.delivery_queue.queued_len(), 0, "not before its reset");

    state.release_usage_limits_due_at(resets_at);
    let told = build_notices(state, &run_id, &agent_id, "after its reported usage limit");
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
    let mut recorded = Default::default();
    state.record_usage_limit(&run_id, &agent_id, &limited(Some(resets_at)), &mut recorded);
    post(state, &run_id, &agent_id, "are you still on this?");

    state.release_usage_limits_due_at(resets_at);

    assert_eq!(
        build_notices(state, &run_id, &agent_id, "after its reported usage limit").len(),
        1
    );
    assert_eq!(
        state.delivery_queue.queued_len(),
        1,
        "the waiting message starts it"
    );
}

#[test]
fn the_first_successful_response_clears_the_limit() {
    let mut standing = standing();
    let (state, _root, run_id, agent_id) = standing.parts();
    let mut recorded = Default::default();
    state.record_usage_limit(
        &run_id,
        &agent_id,
        &limited(Some(an_hour_from_now())),
        &mut recorded,
    );
    assert_eq!(usage_limits(state).len(), 1);

    state.record_usage_limit(&run_id, &agent_id, &successful(), &mut recorded);

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
    let mut recorded = Default::default();
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

    let mut recorded = Default::default();
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

#[test]
fn starting_or_rejecting_a_retry_does_not_clear_the_limit() {
    let mut standing = standing();
    let (state, _root, run_id, agent_id) = standing.parts();
    let snapshot = limited(Some(an_hour_from_now()));
    let mut recorded = Default::default();
    state.record_usage_limit(&run_id, &agent_id, &snapshot, &mut recorded);
    for status in [AgentStatus::Working, AgentStatus::Waiting] {
        state.record_usage_limit(
            &run_id,
            &agent_id,
            &SessionStatusSnapshot::new(status),
            &mut recorded,
        );
        assert_eq!(
            usage_limits(state).len(),
            1,
            "status is not proof of success"
        );
    }
    state.record_usage_limit(&run_id, &agent_id, &snapshot, &mut recorded);
    assert_eq!(
        usage_limits(state).len(),
        1,
        "a rejected retry preserves the report"
    );
}

#[test]
fn a_stale_success_marker_does_not_clear_a_later_limit() {
    let mut standing = standing();
    let (state, _root, run_id, agent_id) = standing.parts();
    let mut recorded = Default::default();
    let success = successful();
    state.record_usage_limit(&run_id, &agent_id, &success, &mut recorded);
    let mut snapshot = limited(Some(an_hour_from_now()));
    snapshot.successful_response_count = success.successful_response_count;
    state.record_usage_limit(&run_id, &agent_id, &snapshot, &mut recorded);
    state.record_usage_limit(&run_id, &agent_id, &success, &mut recorded);
    assert_eq!(
        usage_limits(state).len(),
        1,
        "a previous response is not a new success"
    );
    state.record_usage_limit(
        &run_id,
        &agent_id,
        &success.successful_response(),
        &mut recorded,
    );
    assert!(usage_limits(state).is_empty());
}

#[test]
fn the_latest_limit_wins_when_success_and_a_new_error_are_coalesced() {
    let mut standing = standing();
    let (state, _root, run_id, agent_id) = standing.parts();
    let mut recorded = Default::default();
    let mut snapshot = limited(Some(an_hour_from_now()));
    snapshot.successful_response_count = 1;
    state.record_usage_limit(&run_id, &agent_id, &snapshot, &mut recorded);
    assert_eq!(usage_limits(state).len(), 1);
    state.record_usage_limit(
        &run_id,
        &agent_id,
        &snapshot.successful_response(),
        &mut recorded,
    );
    assert!(usage_limits(state).is_empty());
}

#[test]
fn a_replacement_sessions_first_success_clears_the_existing_report() {
    let mut standing = standing();
    let (state, _root, run_id, agent_id) = standing.parts();
    let mut original = Default::default();
    state.record_usage_limit(
        &run_id,
        &agent_id,
        &limited(Some(an_hour_from_now())),
        &mut original,
    );
    let mut replacement = Default::default();
    state.record_usage_limit(
        &run_id,
        &agent_id,
        &SessionStatusSnapshot::new(AgentStatus::Working),
        &mut replacement,
    );
    assert_eq!(usage_limits(state).len(), 1);
    state.record_usage_limit(&run_id, &agent_id, &successful(), &mut replacement);
    assert!(usage_limits(state).is_empty());
    assert!(!state.usage_limits.stopped(&run_id, &agent_id));
}

#[test]
fn a_new_identical_rejection_restores_a_report_cleared_by_another_agent() {
    let mut standing = standing();
    let (state, _root, run_id, agent_id) = standing.parts();
    let added = state.handle(req("agent.add", json!({ "entity_id": run_id })));
    let other_agent = added["result"]["agent"]["id"].as_str().unwrap().to_owned();
    let mut recorded = Default::default();
    let snapshot = limited(Some(an_hour_from_now()));
    state.record_usage_limit(&run_id, &agent_id, &snapshot, &mut recorded);
    state.record_usage_limit(
        &run_id,
        &other_agent,
        &successful(),
        &mut Default::default(),
    );
    assert!(usage_limits(state).is_empty());
    state.record_usage_limit(&run_id, &agent_id, &snapshot, &mut recorded);
    assert!(
        usage_limits(state).is_empty(),
        "idle snapshots cannot restore a cleared report"
    );

    let rejected_again = snapshot.limited(snapshot.usage_limit.clone().unwrap());
    state.record_usage_limit(&run_id, &agent_id, &rejected_again, &mut recorded);
    assert_eq!(
        usage_limits(state).len(),
        1,
        "the new rejection is shown even with identical wording"
    );
}

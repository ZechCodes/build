use super::*;

struct EndSessionOnDrop<'a>(&'a AdkSession);

impl Drop for EndSessionOnDrop<'_> {
    fn drop(&mut self) {
        self.0.end();
    }
}

#[test]
fn self_started_top_level_activity_opens_a_turn_until_its_result() {
    for activity in [NARRATION, THINKING, TOOL_USE, TOOL_RESULT] {
        let (mut reader, _heard) = reader_and_what_it_reports();
        reader.read_line(INIT);
        reader.state.lock().unwrap().turn_open = true;
        reader.read_line(RESULT);
        assert_eq!(
            reader.state.lock().unwrap().live_status(),
            AgentStatus::Waiting
        );

        reader.read_line(activity);
        assert_eq!(
            reader.state.lock().unwrap().live_status(),
            AgentStatus::Working,
            "activity after a closed turn must open the child's next turn: {activity}"
        );
        reader.read_line(RESULT);
        assert_eq!(
            reader.state.lock().unwrap().live_status(),
            AgentStatus::Waiting
        );
    }
}

#[test]
fn child_activity_outside_the_parent_turn_does_not_open_it() {
    let mut reader = reader_over_a_silent_session();
    reader.read_line(INIT);
    reader.read_line(RESULT);
    for passive in [
        SUBAGENT_TEXT_WITH_USAGE,
        r#"{"type":"assistant","parent_tool_use_id":"parent","message":{"content":[{"type":"tool_use","id":"nested","name":"Read","input":{}}]}}"#,
        r#"{"type":"user","parent_tool_use_id":"parent","message":{"content":[{"type":"tool_result","tool_use_id":"nested","content":"done"}]}}"#,
        r#"{"type":"assistant","message":{"content":[]}}"#,
        TASK_ROSTER_EMPTY,
        TASK_NOTIFICATION,
        r#"{"type":"user","message":{"content":[{"type":"text","text":"echoed prompt"}]}}"#,
        r#"{"type":"control_response","response":{"subtype":"success","request_id":"unknown"}}"#,
        "not protocol",
    ] {
        reader.read_line(passive);
        assert_eq!(
            reader.state.lock().unwrap().live_status(),
            AgentStatus::Waiting
        );
    }
}

#[test]
fn self_started_turn_holds_working_when_background_work_finishes() {
    let mut reader = reader_over_a_silent_session();
    reader.read_line(INIT);
    for line in [
        TASK_STARTED,
        RESULT,
        THINKING,
        TASK_ROSTER_EMPTY,
        TASK_UPDATED_DONE,
        TASK_NOTIFICATION,
    ] {
        reader.read_line(line);
        assert_eq!(
            reader.state.lock().unwrap().live_status(),
            AgentStatus::Working,
            "{line}"
        );
    }
    reader.read_line(RESULT);
    assert_eq!(
        reader.state.lock().unwrap().live_status(),
        AgentStatus::Waiting
    );
}

#[test]
fn self_started_turn_success_and_usage_limit_are_counted_once_per_turn() {
    let (mut reader, status) = reader_and_its_status(Arc::new(Mutex::new(None)));
    for line in [INIT, NARRATION, RESULT] {
        reader.read_line(line);
    }
    assert_eq!(status.borrow().successful_response_count, 1);
    reader.read_line(NARRATION);
    assert_eq!(status.borrow().status, AgentStatus::Working);
    assert_eq!(status.borrow().successful_response_count, 2);
    reader.read_line(NARRATION);
    assert_eq!(status.borrow().successful_response_count, 2);
    reader.read_line(RESULT);
    assert_eq!(status.borrow().successful_response_count, 2);

    reader.read_line(r#"{"type":"assistant","error":"rate_limit","message":{"content":[{"type":"text","text":"You have hit your limit"}]}}"#);
    assert_eq!(status.borrow().status, AgentStatus::Working);
    reader.read_line(FAILED_RESULT);
    assert_eq!(status.borrow().usage_limit_count, 1);
    assert!(status.borrow().usage_limit.is_some());
    reader.read_line(NARRATION);
    assert_eq!(status.borrow().successful_response_count, 3);
    assert!(status.borrow().usage_limit.is_none());
}

#[tokio::test]
async fn self_started_turn_can_be_interrupted_without_a_crash_epitaph() {
    let (session, mut activity) = AdkSession::spawn(
        &stream_json_harness(&[RESULT, THINKING]),
        None,
        &adk_choice(),
    )
    .unwrap();
    let _cleanup = EndSessionOnDrop(&session);
    session.send_turn(&Turn::new("start")).unwrap();
    assert!(matches!(
        next_activity(&mut activity).await,
        AgentActivity::Reasoning { .. }
    ));
    assert_eq!(session.status(), AgentStatus::Working);
    assert!(session.can_interrupt());
    session.interrupt().unwrap();
    wait_for_status(&session, AgentStatus::Waiting);
    assert!(session.epitaph().is_none());
}

#[test]
fn self_started_turn_hands_working_to_a_steering_turn() {
    let mut reader = reader_over_a_silent_session();
    for line in [INIT, RESULT, THINKING] {
        reader.read_line(line);
    }
    assert!(reader.state.lock().unwrap().turn_open);
    reader.state.lock().unwrap().pending_interrupt =
        Some(super::super::protocol::PendingInterrupt {
            request_id: "stop".into(),
            acked: false,
            steered: true,
        });
    reader.read_line(
        r#"{"type":"control_response","response":{"subtype":"success","request_id":"stop"}}"#,
    );
    reader.read_line(FAILED_RESULT);
    assert_eq!(
        reader.state.lock().unwrap().live_status(),
        AgentStatus::Working
    );
    assert!(reader.state.lock().unwrap().reported_error.is_none());
    reader.read_line(RESULT);
    assert_eq!(
        reader.state.lock().unwrap().live_status(),
        AgentStatus::Waiting
    );
}

#[test]
fn buffered_activity_does_not_reopen_a_session_build_ended() {
    let mut reader = reader_over_a_silent_session();
    reader.read_line(INIT);
    reader.read_line(RESULT);
    reader.state.lock().unwrap().closed = true;
    reader.read_line(THINKING);
    assert!(!reader.state.lock().unwrap().turn_open);
}

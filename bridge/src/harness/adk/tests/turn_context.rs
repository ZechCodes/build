use super::*;
use crate::harness::{SessionStatusSnapshot, TurnContext};
use tokio::sync::watch;

/// A reader mid-turn, and the status watch the app would be holding.
fn reader_mid_turn() -> (ProtocolReader, watch::Receiver<SessionStatusSnapshot>) {
    let (updates, watched) = watch::channel(SessionStatusSnapshot::new(AgentStatus::Starting));
    let reader = reader_publishing_status_into(Arc::new(Mutex::new(None)), updates);
    {
        let mut state = reader.state.lock().unwrap();
        state.announced = true;
        state.turn_open = true;
    }
    reader.publish_status(AgentStatus::Working);
    (reader, watched)
}

fn context_of(watched: &watch::Receiver<SessionStatusSnapshot>) -> Option<TurnContext> {
    watched.borrow().context
}

#[test]
fn the_turn_context_is_read_from_the_last_top_level_assistant_usage() {
    let (mut reader, watched) = reader_mid_turn();
    reader.read_line(NARRATION_WITH_USAGE);
    reader.read_line(RESULT_WITH_USAGE);

    assert_eq!(
        context_of(&watched),
        Some(TurnContext {
            context_tokens: 33_000,
            cache_read_tokens: 45_000,
        })
    );
}

#[test]
fn a_subagents_usage_is_not_the_sessions_context() {
    let (mut reader, watched) = reader_mid_turn();
    reader.read_line(NARRATION_WITH_USAGE);
    reader.read_line(SUBAGENT_TEXT_WITH_USAGE);
    reader.read_line(RESULT_WITH_USAGE);

    assert_eq!(context_of(&watched).unwrap().context_tokens, 33_000);
}

#[test]
fn cache_reads_add_up_across_turns_and_a_turn_without_usage_keeps_the_last_context() {
    let (mut reader, watched) = reader_mid_turn();
    reader.read_line(NARRATION_WITH_USAGE);
    reader.read_line(RESULT_WITH_USAGE);
    reader.state.lock().unwrap().turn_open = true;
    reader.read_line(RESULT_WITH_USAGE);

    assert_eq!(
        context_of(&watched),
        Some(TurnContext {
            context_tokens: 33_000,
            cache_read_tokens: 90_000,
        })
    );
}

#[test]
fn the_waiting_snapshot_already_carries_the_turns_context() {
    let (mut reader, mut watched) = reader_mid_turn();
    reader.read_line(NARRATION_WITH_USAGE);
    watched.borrow_and_update();
    assert_eq!(context_of(&watched), None);

    let observer = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        runtime.block_on(async {
            let waiting = watched.wait_for(|snapshot| snapshot.status == AgentStatus::Waiting);
            tokio::time::timeout(Duration::from_secs(10), waiting)
                .await
                .expect("the turn ended")
                .unwrap()
                .context
        })
    });
    reader.read_line(RESULT_WITH_USAGE);

    assert_eq!(observer.join().unwrap().unwrap().context_tokens, 33_000);
}

#[test]
fn a_compaction_resets_the_context_to_what_survived_it() {
    let (mut reader, watched) = reader_mid_turn();
    reader.read_line(NARRATION_WITH_USAGE);
    reader.read_line(RESULT_WITH_USAGE);

    reader.read_line(COMPACT_BOUNDARY_WITH_POST_TOKENS);
    assert_eq!(
        context_of(&watched),
        Some(TurnContext {
            context_tokens: 8_000,
            cache_read_tokens: 45_000,
        })
    );

    reader.read_line(COMPACT_BOUNDARY);
    assert_eq!(context_of(&watched).unwrap().context_tokens, 0);
}

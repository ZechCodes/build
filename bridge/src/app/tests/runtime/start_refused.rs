//! A session Build ends before it runs a turn says why on the agent's record
//! (issue #72): two agents asked for `opus` were ended over their `init` line
//! and sat idle for a day with `start_error` empty and nothing in the log.

use super::*;
use crate::harness::SessionStatusSnapshot;

const RUN: &str = "run-refused";
const REFUSED: &str = "Build stopped this agent's Claude Code session because it opened claude-fable-5-1, and the agent asks for claude-opus-5.";

#[tokio::test]
async fn a_session_ended_over_its_start_puts_the_reason_on_the_agent() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let root = insert_run(&mut app, &repo, dir.path(), RUN, RunState::Building);
    let agent_id = crate::agent::derived_agent_id(RUN);
    let (status, watched) =
        tokio::sync::watch::channel(SessionStatusSnapshot::new(AgentStatus::Starting));
    insert_agent_tab(
        &mut app,
        &root,
        RUN,
        &agent_id,
        DictatedSession::reporting(AgentStatus::Starting)
            .watching_status(watched)
            .refusing_start(REFUSED),
    );
    let state = app.shared();
    let key = TabKey::agent(&AppState::canonical_root(&root), &agent_id);
    let (session, instance) = {
        let app = state.lock().unwrap();
        let tab = app.session_registry.test_tab(&key).unwrap();
        (Arc::clone(&tab.session), tab.session_instance.clone())
    };
    spawn_status_pump(&state, key, session, instance, Some(status.subscribe()));
    let start_error = || {
        state.lock().unwrap().runs[RUN]
            .agents
            .by_id(&agent_id)
            .unwrap()
            .start_error
            .clone()
    };
    assert_eq!(start_error(), None, "nothing to say while it is starting");

    let ended = status
        .borrow()
        .transition(AgentStatus::Ended { code: Some(137) })
        .unwrap();
    status.send(ended).unwrap();

    let recorded = wait_for(Duration::from_secs(2), start_error)
        .await
        .expect("the reason reaches the agent's record");
    assert_eq!(recorded, REFUSED);
}

/// Issue #73: a child handed a turn that never announces itself is ended at
/// the startup deadline, and the agent's record says so in the same way a
/// refused model does — through the real session and the real status pump,
/// so the path from the watchdog to `start_error` is the one the daemon runs.
#[tokio::test]
async fn a_session_ended_at_the_startup_deadline_puts_the_reason_on_the_agent() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let root = insert_run(&mut app, &repo, dir.path(), RUN, RunState::Building);
    let agent_id = crate::agent::derived_agent_id(RUN);
    let silent = HarnessSpec::new("sh").arg("-c").arg("cat >/dev/null");
    let (session, _activity) = crate::harness::adk::AdkSession::spawn_with_startup_deadline(
        &silent,
        None,
        &ModelChoice::default(),
        Duration::from_millis(200),
    )
    .expect("the silent child spawns");
    let changed = session.status_changed();
    insert_agent_tab(&mut app, &root, RUN, &agent_id, session);
    let state = app.shared();
    let key = TabKey::agent(&AppState::canonical_root(&root), &agent_id);
    let (session, instance) = {
        let app = state.lock().unwrap();
        let tab = app.session_registry.test_tab(&key).unwrap();
        (Arc::clone(&tab.session), tab.session_instance.clone())
    };
    spawn_status_pump(&state, key, Arc::clone(&session), instance, changed);
    let start_error = || {
        state.lock().unwrap().runs[RUN]
            .agents
            .by_id(&agent_id)
            .unwrap()
            .start_error
            .clone()
    };

    session
        .send_turn(&Turn::new("go"))
        .expect("the turn is written");
    let recorded = wait_for(Duration::from_secs(5), start_error)
        .await
        .expect("the deadline's reason reaches the agent's record");
    assert_eq!(
        recorded,
        "Build stopped this agent's Claude Code session because it did not start within 0.2 seconds."
    );
    session.end();
}

//! Resume after a bridge roll.
//!
//! Rolling the binary ends every harness on the device at once. What these pin
//! is the contract that closes the gap: who a shutdown writes down, who it
//! deliberately does not, and what a boot does with the list.

use super::project_agent::{added_project, rooted, workspace};
use super::*;
use crate::agent::AgentLifecycle;
use crate::resume::{LiveRoster, ResumeRoster, LIVE_ROSTER_FILE, OPT_OUT_FILE};

/// A project with one workspace, a conversation on it, and one agent standing
/// in it. The shape every roll actually finds.
pub(super) struct Standing {
    state: AppState,
    state_root: std::path::PathBuf,
    run_id: String,
    agent_id: String,
    _home: tempfile::TempDir,
    _tmp: tempfile::TempDir,
}

pub(super) fn standing() -> Standing {
    standing_on(rooted)
}

/// The same, on a state root `make` builds the app on.
fn standing_on(make: impl FnOnce(&std::path::Path) -> AppState) -> Standing {
    let (home, repo) = init_repo();
    let repo = std::fs::canonicalize(&repo).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let mut state = make(&state_root);
    let project_id = added_project(&mut state, &repo);
    let workspace_id = workspace(&mut state, &project_id, "the work");
    let ensured = state.handle(req(
        "workspace.ensure_conversation",
        json!({ "workspace_id": workspace_id }),
    ));
    let run_id = ensured["result"]["run_id"].as_str().unwrap().to_string();
    let added = state.handle(req("agent.add", json!({ "entity_id": run_id })));
    let agent_id = added["result"]["agent"]["id"].as_str().unwrap().to_string();
    Standing {
        state,
        state_root,
        run_id,
        agent_id,
        _home: home,
        _tmp: tmp,
    }
}

impl Standing {
    /// The four things every test here works with, with the tempdirs left on
    /// the struct: destructuring would drop them, and the state root would go
    /// out from under the roster mid-test.
    pub(super) fn parts(&mut self) -> (&mut AppState, std::path::PathBuf, String, String) {
        (
            &mut self.state,
            self.state_root.clone(),
            self.run_id.clone(),
            self.agent_id.clone(),
        )
    }

    /// The same, for the one test that needs the shared handle the daemon
    /// itself holds. The tempdir guards come back with it so the caller can
    /// keep the state root alive past this call.
    fn shared(
        self,
    ) -> (
        Arc<Mutex<AppState>>,
        std::path::PathBuf,
        String,
        String,
        (tempfile::TempDir, tempfile::TempDir),
    ) {
        (
            self.state.shared(),
            self.state_root,
            self.run_id,
            self.agent_id,
            (self._home, self._tmp),
        )
    }
}

/// Put the agent where a roll would find it, without a harness to do it for us.
fn set_state(
    state: &mut AppState,
    run_id: &str,
    agent_id: &str,
    live: AgentLifecycle,
    working: bool,
) {
    let run = state.runs.get_mut(run_id).expect("the run");
    let agent = run
        .agents
        .iter_mut()
        .find(|agent| agent.id == agent_id)
        .expect("the agent");
    agent.state = live;
    agent.working_since = working.then(|| "2026-09-20T03:00:00Z".to_string());
    agent.resume_session_id = Some("sess-before-the-roll".to_string());
}

/// What Build itself said on this conversation, if anything. The mark is the
/// point of the field: a reader looks for Build's voice, not for a body.
fn build_said(state: &mut AppState, run_id: &str, agent_id: &str) -> Option<Value> {
    super::project_agent::items(state, run_id, agent_id)
        .into_iter()
        .find(|item| item["data"]["from_build"] == json!(true))
        .map(|item| item["data"].clone())
}

/// Shutdown: an agent that was mid-turn is written down, with what a respawn
/// needs beside it.
#[test]
fn an_agent_that_was_working_is_recorded_at_shutdown_and_brought_back_at_boot() {
    let mut standing = standing();
    let (state, state_root, run_id, agent_id) = standing.parts();
    set_state(state, &run_id, &agent_id, AgentLifecycle::Live, true);

    // Shutdown.
    let roster = state.resume_roster("0.2.0");
    assert_eq!(roster.agents.len(), 1, "{roster:?}");
    let recorded = &roster.agents[0];
    assert_eq!(recorded.agent_id, agent_id);
    assert_eq!(recorded.entity_id, run_id);
    assert!(recorded.was_working, "it was mid-turn: {recorded:?}");
    assert_eq!(
        recorded.resume_session_id.as_deref(),
        Some("sess-before-the-roll"),
        "the respawn resumes BY NAME, so the name is what is persisted"
    );
    roster.save(&state_root).unwrap();
    assert_eq!(
        ResumeRoster::take(&state_root).as_ref(),
        Some(&roster),
        "and it survives the roll on disk"
    );
}

/// Boot: that agent comes back, with a notice from Build on its conversation
/// and a turn queued to carry it — the same queue every other verb speaks to an
/// agent with, so the spawn resumes by session id exactly as `agent.start` does.
#[test]
fn a_recorded_agent_is_brought_back_at_boot_with_a_notice_from_build() {
    let mut standing = standing();
    let (state, _state_root, run_id, agent_id) = standing.parts();
    set_state(state, &run_id, &agent_id, AgentLifecycle::Live, true);
    let roster = state.resume_roster("0.2.0");

    // The binary at boot is deliberately not the one that went down.
    let resumed = state.resume_recorded_agents(&roster, "0.3.0");
    assert_eq!(resumed, vec![agent_id.clone()]);

    let notice = build_said(state, &run_id, &agent_id).expect("Build said something");
    let body = notice["body"].as_str().unwrap();
    assert!(
        body.contains("bridge 0.3.0"),
        "the NEW binary is named: {body}"
    );
    assert!(
        body.contains(&roster.recorded_at),
        "when it went down: {body}"
    );
    assert!(body.contains("cut short"), "{body}");
    assert!(body.contains("from Build, not from the user"), "{body}");
    assert!(
        !body.contains("comment_task"),
        "a boot that renamed nothing says nothing about names: {body}"
    );
    assert_eq!(
        notice.get("from_agent"),
        None,
        "Build is not an agent and must not borrow an agent's identity"
    );

    assert_eq!(state.delivery_queue.queued_len(), 1, "one turn was queued");
    let queued = state.delivery_queue.queued_nth(0).unwrap();
    assert_eq!(queued.owner, run_id);
    assert_eq!(queued.agent_id, agent_id);
    let delivered =
        state.cold_prompt_with_catch_up(&queued.owner, &queued.agent_id, &queued.said().cold);
    assert!(
        delivered.contains("Build restarted at"),
        "a cold session reads the notice out of the packet composed at delivery: {delivered}"
    );
}

/// An agent nobody was talking to was waiting before the roll and can wait
/// after one. Resurrecting it would spend a model on a conversation with
/// nothing in it.
#[test]
fn an_idle_agent_is_not_recorded_and_not_woken() {
    let mut standing = standing();
    let (state, state_root, run_id, agent_id) = standing.parts();
    set_state(state, &run_id, &agent_id, AgentLifecycle::Idle, false);

    let roster = state.resume_roster("0.2.0");
    assert!(roster.agents.is_empty(), "{roster:?}");

    // An empty roster writes no file at all, so the boot has nothing to read
    // and nothing to do.
    roster.save(&state_root).unwrap();
    assert!(ResumeRoster::take(&state_root).is_none());
    assert_eq!(
        state.resume_recorded_agents(&roster, "0.3.0"),
        Vec::<String>::new()
    );
    assert!(state.delivery_queue.queued_is_empty());
    assert_eq!(
        build_said(state, &run_id, &agent_id),
        None,
        "nothing was said to an agent that was not working"
    );
}

/// Live but not mid-turn still counts. The human had a session open and would
/// come back to an empty rail otherwise — and the notice says the softer thing,
/// because nothing was cut off mid-sentence.
#[test]
fn a_live_agent_between_turns_is_brought_back_without_being_told_it_was_cut_short() {
    let mut standing = standing();
    let (state, _state_root, run_id, agent_id) = standing.parts();
    set_state(state, &run_id, &agent_id, AgentLifecycle::Live, false);

    let roster = state.resume_roster("0.2.0");
    assert_eq!(roster.agents.len(), 1, "{roster:?}");
    assert!(!roster.agents[0].was_working);

    state.resume_recorded_agents(&roster, "0.3.0");
    let notice = build_said(state, &run_id, &agent_id).expect("Build said something");
    let body = notice["body"].as_str().unwrap();
    assert!(!body.contains("cut short"), "nothing was cut short: {body}");
    assert!(body.contains("Read your conversation"), "{body}");
}

/// The opt-out: a deliberate shutdown does not resurrect anybody.
///
/// Both ends are checked, because a roll script can only reach one of them.
/// The marker is consumed either way, so opting out of tonight's roll does not
/// silently opt out of every roll after it.
#[test]
fn the_per_roll_opt_out_is_honoured_at_both_ends() {
    let mut standing = standing();
    {
        let (state, _, run_id, agent_id) = standing.parts();
        set_state(state, &run_id, &agent_id, AgentLifecycle::Live, true);
    }
    let (shared, state_root, run_id, agent_id, _kept) = standing.shared();
    let roster = shared.lock().unwrap().resume_roster("0.2.0");
    assert_eq!(roster.agents.len(), 1);
    roster.save(&state_root).unwrap();

    std::fs::write(state_root.join(OPT_OUT_FILE), "").unwrap();
    let resumed = AppState::resume_after_restart(&shared, &state_root, "0.3.0");
    assert!(resumed.is_empty(), "nobody was brought back: {resumed:?}");
    {
        let mut app = shared.lock().unwrap();
        assert!(
            app.delivery_queue.queued_is_empty(),
            "and nothing was queued"
        );
        assert_eq!(
            build_said(&mut app, &run_id, &agent_id),
            None,
            "and nothing was said"
        );
    }

    // One roll only: the marker and the roster are both gone, so the boot after
    // this one behaves normally again.
    assert!(!state_root.join(OPT_OUT_FILE).exists());
    assert!(!ResumeRoster::path(&state_root).exists());
}

/// A roster line whose agent was deleted while the daemon was down costs that
/// line and nothing else — not the boot, and not the agents beside it.
#[test]
fn a_stale_roster_line_is_skipped_and_the_rest_still_come_back() {
    let mut standing = standing();
    let (state, _state_root, run_id, agent_id) = standing.parts();
    set_state(state, &run_id, &agent_id, AgentLifecycle::Live, true);
    let mut roster = state.resume_roster("0.2.0");
    roster.agents.insert(
        0,
        crate::resume::ResumingAgent {
            entity_id: "run-that-is-gone".to_string(),
            agent_id: "agent-that-is-gone".to_string(),
            conversation_id: "agent-that-is-gone".to_string(),
            resume_session_id: None,
            was_working: true,
        },
    );

    let resumed = state.resume_recorded_agents(&roster, "0.3.0");
    assert_eq!(resumed, vec![agent_id], "the live one still came back");
    assert_eq!(state.delivery_queue.queued_len(), 1);
}

/// The live roster's file, parsed, or `None` when there is none.
fn live_roster_on_disk(state_root: &std::path::Path) -> Option<ResumeRoster> {
    let raw = std::fs::read_to_string(state_root.join(LIVE_ROSTER_FILE)).ok()?;
    Some(serde_json::from_str(&raw).expect("the live roster parses"))
}

/// Which agents the live roster's file names, sorted. Sorted because the
/// file is in roster order, and two agents added in the same instant can
/// hold ids whose order is not their roster order: these tests are about who
/// is on it, not in what order.
fn live_roster_ids(state_root: &std::path::Path) -> Vec<String> {
    let mut ids: Vec<String> = live_roster_on_disk(state_root)
        .map(|roster| {
            roster
                .agents
                .into_iter()
                .map(|agent| agent.agent_id)
                .collect()
        })
        .unwrap_or_default();
    ids.sort();
    ids
}

/// A turn starting puts the agent in the live roster and a turn stopping
/// takes it out — through the same record the pumps write, with nothing
/// written by the caller.
#[test]
fn a_turn_starting_and_stopping_rewrites_the_live_roster() {
    let mut standing = standing();
    let (state, state_root, run_id, agent_id) = standing.parts();
    let live = LiveRoster::start(&state_root, "0.2.2");
    state.live_roster = Some(live.clone());

    state.start_agent_working(&run_id, &agent_id, "2026-09-25T02:00:00Z");
    live.settle();
    let written = live_roster_on_disk(&state_root).expect("the turn start was written");
    assert_eq!(written.agents.len(), 1, "{written:?}");
    assert_eq!(written.agents[0].agent_id, agent_id);
    assert_eq!(written.agents[0].entity_id, run_id);
    assert!(written.agents[0].was_working);

    state.record_agent_working_since(&run_id, &agent_id, None);
    live.settle();
    assert_eq!(
        live_roster_on_disk(&state_root),
        None,
        "nobody is working, so there is nothing a crash would lose"
    );
}

/// Retiring a working agent takes it out of the live roster: a crash after
/// the retirement must not bring back an agent the human removed.
#[test]
fn a_retired_agent_leaves_the_live_roster() {
    let mut standing = standing();
    let (state, state_root, run_id, agent_id) = standing.parts();
    let added = state.handle(req("agent.add", json!({ "entity_id": run_id })));
    let second = added["result"]["agent"]["id"].as_str().unwrap().to_string();
    let live = LiveRoster::start(&state_root, "0.2.2");
    state.live_roster = Some(live.clone());

    state.start_agent_working(&run_id, &agent_id, "2026-09-25T02:00:00Z");
    state.start_agent_working(&run_id, &second, "2026-09-25T02:00:00Z");
    live.settle();
    let mut both = vec![agent_id.clone(), second.clone()];
    both.sort();
    assert_eq!(live_roster_ids(&state_root), both);

    let removed = state.handle(req(
        "agent.remove",
        json!({ "entity_id": run_id, "agent_id": second }),
    ));
    assert_eq!(removed["ok"], true, "{removed:?}");
    live.settle();
    assert_eq!(live_roster_ids(&state_root), vec![agent_id]);
}

/// The whole of a hard kill, end to end: an agent is mid-turn, the daemon
/// dies without its shutdown running (the writer is never told to finish),
/// and the next boot promotes the live roster and brings the agent back with
/// Build's notice on its conversation.
#[test]
fn an_agent_working_at_a_hard_kill_is_brought_back_at_the_next_boot() {
    let mut standing = standing();
    {
        let (state, state_root, run_id, agent_id) = standing.parts();
        let live = LiveRoster::start(&state_root, "0.2.2");
        state.live_roster = Some(live.clone());
        set_state(state, &run_id, &agent_id, AgentLifecycle::Idle, false);
        state.start_agent_working(&run_id, &agent_id, "2026-09-25T02:00:00Z");
        live.settle();
        // SIGKILL: no `finish`, no roster proper.
        state.live_roster = None;
        assert!(!ResumeRoster::path(&state_root).exists());
    }
    let (shared, state_root, run_id, agent_id, _kept) = standing.shared();

    assert!(crate::resume::promote_live_roster(&state_root));
    let resumed = AppState::resume_after_restart(&shared, &state_root, "0.2.3");
    assert_eq!(resumed, vec![agent_id.clone()]);
    // The turn itself was queued and handed on by the boot's own drain; the
    // notice it carries is what stays behind to read.
    let mut app = shared.lock().unwrap();
    let notice = build_said(&mut app, &run_id, &agent_id).expect("Build said something");
    assert!(
        notice["body"].as_str().unwrap().contains("cut short"),
        "{notice}"
    );
}

/// A workspace deleted from disk while the daemon was down is not resumed
/// into: the harness would start in a directory that is not there.
#[test]
fn an_agent_whose_workspace_is_gone_is_not_brought_back() {
    let mut standing = standing();
    let checkout = {
        let (state, state_root, run_id, agent_id) = standing.parts();
        set_state(state, &run_id, &agent_id, AgentLifecycle::Live, true);
        state.resume_roster("0.2.2").save(&state_root).unwrap();
        state.entity_agent_root(&run_id).unwrap()
    };
    assert!(checkout.is_dir(), "{}", checkout.display());
    std::fs::remove_dir_all(&checkout).unwrap();
    let (shared, state_root, run_id, agent_id, _kept) = standing.shared();

    let resumed = AppState::resume_after_restart(&shared, &state_root, "0.2.3");
    assert!(resumed.is_empty(), "{resumed:?}");
    let mut app = shared.lock().unwrap();
    assert!(app.delivery_queue.queued_is_empty(), "nothing was queued");
    assert_eq!(build_said(&mut app, &run_id, &agent_id), None);
    assert!(
        !ResumeRoster::path(&state_root).exists(),
        "and the roster is consumed all the same"
    );
}

/// What reaches disk is addresses. A conversation carrying a secret — as the
/// human's message and as a path the agent read — contributes ids to the
/// roster and not one word of what was said.
#[test]
fn the_live_roster_holds_ids_and_never_what_the_conversation_said() {
    const SECRET: &str = "roster-canary-the-human-said-this-in-confidence";
    let mut standing = standing();
    let (state, state_root, run_id, agent_id) = standing.parts();
    let posted = state.handle(req(
        "thread.post",
        json!({
            "entity_id": run_id,
            "agent_id": agent_id,
            "body": format!("deploy with {SECRET} and tell nobody"),
        }),
    ));
    assert_eq!(posted["ok"], true, "{posted:?}");
    let live = LiveRoster::start(&state_root, "0.2.2");
    state.live_roster = Some(live.clone());
    state.start_agent_working(&run_id, &agent_id, "2026-09-25T02:00:00Z");
    live.settle();

    let raw = std::fs::read_to_string(state_root.join(LIVE_ROSTER_FILE)).unwrap();
    assert!(!raw.contains(SECRET), "{raw}");
    assert!(!raw.contains("tell nobody"), "{raw}");
    let written: Value = serde_json::from_str(&raw).unwrap();
    let agent = &written["agents"][0];
    for (key, value) in agent.as_object().unwrap() {
        match key.as_str() {
            "entity_id" => assert_eq!(value, &json!(run_id)),
            "agent_id" => assert_eq!(value, &json!(agent_id)),
            "conversation_id" | "resume_session_id" => {
                assert!(value.is_string() || value.is_null(), "{key}: {value}")
            }
            "was_working" => assert_eq!(value, &json!(true)),
            other => panic!("the roster grew a field it must not carry: {other}"),
        }
    }

    // And the roster proper the clean shutdown writes is the same record.
    live.finish(true).unwrap();
    let clean = std::fs::read_to_string(ResumeRoster::path(&state_root)).unwrap();
    assert!(!clean.contains(SECRET), "{clean}");
    assert!(!state_root.join(LIVE_ROSTER_FILE).exists());
}

/// A transaction checks out more than one entity — `run.stage_send_notes`
/// takes the run and its plan, and finishes the plan first. An entity
/// settling while another is checked out must not drop the checked-out one's
/// agents from the live roster, not even for the one write the writer can
/// land between the two finishes.
#[test]
fn an_entity_checked_out_by_a_transaction_keeps_its_agents_in_the_live_roster() {
    let mut standing = standing();
    let (state, state_root, run_id, agent_id) = standing.parts();
    let project_id = state.project_of(&run_id).unwrap();
    let other_workspace = workspace(state, &project_id, "the other work");
    let ensured = state.handle(req(
        "workspace.ensure_conversation",
        json!({ "workspace_id": other_workspace }),
    ));
    let other_run = ensured["result"]["run_id"].as_str().unwrap().to_string();
    let added = state.handle(req("agent.add", json!({ "entity_id": other_run })));
    let other_agent = added["result"]["agent"]["id"].as_str().unwrap().to_string();
    let live = LiveRoster::start(&state_root, "0.2.2");
    state.live_roster = Some(live.clone());
    state.start_agent_working(&run_id, &agent_id, "2026-09-25T02:00:00Z");
    live.settle();
    assert_eq!(live_roster_ids(&state_root), vec![agent_id.clone()]);

    // The transaction: the run is out of the map while another entity settles.
    let active = state.take_run(&run_id).unwrap();
    state.start_agent_working(&other_run, &other_agent, "2026-09-25T02:01:00Z");
    live.settle();
    let mut both = vec![agent_id.clone(), other_agent.clone()];
    both.sort();
    assert_eq!(
        live_roster_ids(&state_root),
        both,
        "the checked-out run's agent is still there"
    );

    state.finish_run_mutation(run_id.clone(), active).unwrap();
    live.settle();
    assert_eq!(live_roster_ids(&state_root), both);
}

/// A state root holding a store exactly as a v9 bridge left it, so the boot
/// that opens it is the one that runs the task rename (#190).
fn rooted_on_a_v9_store(state_root: &std::path::Path) -> AppState {
    let dir = state_root.join("tasks");
    std::fs::create_dir_all(&dir).unwrap();
    let conn = rusqlite::Connection::open(dir.join("build.db")).unwrap();
    conn.execute_batch(include_str!("../../store/tests/schema_v9.sql"))
        .unwrap();
    conn.execute_batch("INSERT INTO meta (key, value) VALUES ('schema_version', '9')")
        .unwrap();
    drop(conn);
    rooted(state_root).with_task_store(dir).unwrap()
}

/// An agent brought back by the boot that ran the task rename (#190)
/// remembers tools that are gone: its transcript is full of their old names.
/// The notice that wakes it says what they are called now.
#[test]
fn an_agent_resumed_by_the_boot_that_ran_the_task_rename_is_told_the_new_tool_names() {
    let mut standing = standing_on(rooted_on_a_v9_store);
    let (state, _state_root, run_id, agent_id) = standing.parts();
    set_state(state, &run_id, &agent_id, AgentLifecycle::Live, true);
    let roster = state.resume_roster("0.2.0");

    assert_eq!(
        state.resume_recorded_agents(&roster, "0.3.0"),
        vec![agent_id.clone()]
    );

    let notice = build_said(state, &run_id, &agent_id).expect("Build said something");
    let body = notice["body"].as_str().unwrap();
    assert!(
        body.contains("Build's `*_issue` tools are now `*_task` (e.g. comment_issue → comment_task); old issue-… ids still work."),
        "{body}"
    );
}

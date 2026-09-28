use super::*;

// ---- mute: an entry told to stop asking ---------------------------------

/// The feed row for one entity, whatever kind of work item it folded into.
pub(in crate::app::tests) fn work_item_row_for(state: &mut AppState, entity_id: &str) -> Value {
    let rows = work_item_rows(state);
    rows.iter()
        .find(|row| row["run_id"] == json!(entity_id))
        .or_else(|| rows.iter().find(|row| row["task_id"] == json!(entity_id)))
        .unwrap_or_else(|| panic!("{entity_id} has a row on the feed: {rows:?}"))
        .clone()
}

/// Mute is a switch on the badge, not on the work: the entry keeps its place
/// in the inbox with live status, and unmuting shows exactly what was
/// waiting — the read cursor never moved.
#[test]
#[allow(clippy::cognitive_complexity)] // ratchet: muting_an_entry_silences_its_badge_and_unmuting_brings_it_back is at 27, threshold 15 — bring it under, then remove
fn muting_an_entry_silences_its_badge_and_unmuting_brings_it_back() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (task_id, run_id) = planned_run_in_review(&mut state, "mute me");
    push_to_task_conversation(&mut state, &task_id, |thread| {
        thread.post_agent("which name did you want?", None, now_rfc3339());
    });

    let loud = board_entry(&mut state, &run_id);
    assert_eq!(loud["muted"], false, "an entry asks until told not to");
    assert_eq!(loud["unread"], true, "{loud:?}");
    let waiting = loud["unread_count"].as_u64().unwrap();
    assert!(waiting >= 1, "{loud:?}");

    let silenced = state.handle(req(
        "entity.mute",
        json!({ "entity_id": run_id, "muted": true }),
    ));
    assert_eq!(silenced["ok"], true, "{silenced:?}");
    assert_eq!(silenced["result"]["entity_id"], run_id, "{silenced:?}");
    assert_eq!(silenced["result"]["muted"], true, "{silenced:?}");

    let quiet = board_entry(&mut state, &run_id);
    assert_eq!(quiet["muted"], true, "{quiet:?}");
    assert_eq!(quiet["unread"], false, "{quiet:?}");
    assert_eq!(quiet["unread_count"], 0, "{quiet:?}");
    assert!(quiet["unread_reason"].is_null(), "{quiet:?}");
    assert_eq!(quiet["needs_attention"], false, "{quiet:?}");
    // Status is not a badge: the entry still says where the work got to.
    assert_eq!(quiet["state"], loud["state"], "{quiet:?}");
    assert_eq!(quiet["thread"], loud["thread"], "{quiet:?}");

    // The feed row it renders from says the same, and stays live with it.
    let row = work_item_row_for(&mut state, &run_id);
    assert_eq!(row["muted"], true, "{row:?}");
    assert_eq!(row["unread"], false, "{row:?}");
    assert_eq!(row["unread_count"], 0, "{row:?}");
    assert!(row["unread_reason"].is_null(), "{row:?}");
    assert_eq!(row["state"], loud["state"], "{row:?}");
    assert!(row["stat"]["insertions"].is_u64(), "{row:?}");

    // Its agent's bubble goes quiet with it — the entry's badge is the union
    // of theirs, and one that still counted would contradict the other.
    for bubble in row["agents"].as_array().unwrap() {
        assert_eq!(bubble["unread_count"], 0, "{bubble:?}");
    }

    let restored = state.handle(req(
        "entity.mute",
        json!({ "entity_id": run_id, "muted": false }),
    ));
    assert_eq!(restored["result"]["muted"], false, "{restored:?}");
    let loud_again = board_entry(&mut state, &run_id);
    assert_eq!(loud_again["muted"], false, "{loud_again:?}");
    assert_eq!(loud_again["unread"], true, "{loud_again:?}");
    assert_eq!(loud_again["unread_count"], waiting, "{loud_again:?}");
    assert_eq!(
        loud_again["unread_reason"], "agent_message",
        "{loud_again:?}"
    );
}

/// Muted means the phone stays dark. Nothing is spent on the silence, so the
/// first piece of news after unmuting pushes rather than sitting out a
/// debounce window it never entered.
#[test]
fn a_muted_entry_pushes_nothing_and_burns_no_debounce_window() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (_, run_id) = planned_run_in_review(&mut state, "quiet push");

    state.handle(req(
        "entity.mute",
        json!({ "entity_id": run_id, "muted": true }),
    ));
    assert!(
        !state.agent_news_pushes(&run_id, Some("done")),
        "a muted entry pushes nothing"
    );

    state.handle(req(
        "entity.mute",
        json!({ "entity_id": run_id, "muted": false }),
    ));
    assert!(state.agent_news_pushes(&run_id, Some("done")));
}

#[test]
fn entity_mute_refuses_what_it_cannot_silence() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (_, run_id) = planned_run_in_review(&mut state, "bad mute");

    let unknown = state.handle(req(
        "entity.mute",
        json!({ "entity_id": "run-nowhere", "muted": true }),
    ));
    assert_eq!(unknown["ok"], false, "{unknown:?}");

    let unsaid = state.handle(req("entity.mute", json!({ "entity_id": run_id })));
    assert_eq!(unsaid["ok"], false, "{unsaid:?}");
    let entry = board_entry(&mut state, &run_id);
    assert_eq!(entry["muted"], false, "a refused call changes nothing");
}

// ---- dismiss: a row cleared until the work speaks again -----------------

/// The whole feature: a row the human clears leaves the inbox, stays gone
/// while only tools run, and comes back by itself the
/// moment anybody sends a message. Nothing un-dismisses it, because
/// nothing has to.
#[test]
#[allow(clippy::cognitive_complexity)] // ratchet: dismissing_a_row_clears_it_until_the_work_speaks_again is at 17, threshold 15 — bring it under, then remove
fn dismissing_a_row_clears_it_until_the_work_speaks_again() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (task_id, run_id) = planned_run_in_review(&mut state, "clear me");
    push_to_task_conversation(&mut state, &task_id, |thread| {
        thread.post_agent("which name did you want?", None, now_rfc3339());
    });
    let seen = state.handle(req("entity.seen", json!({ "entity_id": run_id })));
    assert_eq!(seen["ok"], true, "{seen:?}");

    let row = work_item_row_for(&mut state, &run_id);
    assert_eq!(row["unread"], false, "{row:?}");
    assert_eq!(
        row["dismissed"], false,
        "a row nobody cleared is on the list"
    );

    let cleared = state.handle(req("entity.dismiss", json!({ "entity_id": run_id })));
    assert_eq!(cleared["ok"], true, "{cleared:?}");
    assert_eq!(cleared["result"]["entity_id"], run_id, "{cleared:?}");
    assert_eq!(cleared["result"]["dismissed"], true, "{cleared:?}");

    let row = work_item_row_for(&mut state, &run_id);
    assert_eq!(row["dismissed"], true, "{row:?}");
    // The entry the detail surfaces read says the same, and dismissing said
    // nothing about anything else: the row is live, loud, and unarchived.
    let entry = board_entry(&mut state, &run_id);
    assert_eq!(entry["dismissed"], true, "{entry:?}");
    assert_eq!(entry["muted"], false, "{entry:?}");
    assert_eq!(entry["unread"], false, "{entry:?}");
    assert_eq!(entry["state"], "review", "{entry:?}");

    // Tool activity leaves it cleared.
    push_to_task_conversation(&mut state, &task_id, |thread| {
        thread.push_event(
            crate::thread::ThreadEventKind::Committed,
            None,
            None,
            None,
            now_rfc3339(),
        );
    });
    let row = work_item_row_for(&mut state, &run_id);
    assert_eq!(row["dismissed"], true, "tools do not speak: {row:?}");

    push_to_task_conversation(&mut state, &task_id, |thread| {
        thread.post_agent_progress("still going", None, now_rfc3339());
    });
    let row = work_item_row_for(&mut state, &run_id);
    assert_eq!(row["dismissed"], false, "progress is a message: {row:?}");

    // The agent handing its turn back is, and the row is in the list again
    // with nobody having to un-dismiss it.
    push_to_task_conversation(&mut state, &task_id, |thread| {
        thread.post_agent("done — take a look", None, now_rfc3339());
    });
    let row = work_item_row_for(&mut state, &run_id);
    assert_eq!(row["dismissed"], false, "{row:?}");
    assert_eq!(row["unread"], true, "{row:?}");

    // Reading what it said is not clearing it away again: the row stays
    // until the human says so a second time.
    state.handle(req("entity.seen", json!({ "entity_id": run_id })));
    let row = work_item_row_for(&mut state, &run_id);
    assert_eq!(row["unread"], false, "{row:?}");
    assert_eq!(
        row["dismissed"], false,
        "reading is not dismissing: {row:?}"
    );
}

/// Write into one agent's own conversation — the second voice on a branch,
/// which numbers its items from 1 in a sequence space of its own.
fn push_to_agent_conversation(
    state: &mut AppState,
    run_id: &str,
    agent_id: &str,
    write: impl FnOnce(&mut crate::thread::Thread),
) {
    let run = state.runs.get_mut(run_id).expect("the run exists");
    let agent = run
        .agents
        .by_id_mut(agent_id)
        .expect("the agent is on the roster");
    write(&mut agent.thread);
}

/// The regression: a row is cleared against every agent on it, not just the
/// first. Thread sequences are per-agent, so one scalar drawn off agent one
/// says nothing about agent two — and a row cleared before agent two ever
/// spoke used to vanish again the moment the human read what it said.
#[test]
fn clearing_a_row_draws_a_line_under_every_agent_on_it() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (task_id, run_id) = planned_run_in_review(&mut state, "two voices");
    push_to_task_conversation(&mut state, &task_id, |thread| {
        thread.post_agent("which name did you want?", None, now_rfc3339());
    });
    state.handle(req("entity.seen", json!({ "entity_id": run_id })));
    let cleared = state.handle(req("entity.dismiss", json!({ "entity_id": run_id })));
    assert_eq!(cleared["ok"], true, "{cleared:?}");
    let row = work_item_row_for(&mut state, &run_id);
    assert_eq!(row["dismissed"], true, "{row:?}");

    // A second agent joins the branch and asks for something of its own.
    let added = state.handle(req("agent.add", json!({ "entity_id": run_id })));
    assert_eq!(added["ok"], true, "{added:?}");
    let second_agent = added["result"]["agent"]["id"].as_str().unwrap().to_string();
    push_to_agent_conversation(&mut state, &run_id, &second_agent, |thread| {
        thread.post_agent("and this one — which way?", None, now_rfc3339());
    });
    let row = work_item_row_for(&mut state, &run_id);
    assert_eq!(row["unread"], true, "{row:?}");
    assert_eq!(row["dismissed"], false, "unread beats dismissed: {row:?}");
    state.handle(req(
        "agent.remove",
        json!({ "entity_id": run_id, "agent_id": second_agent }),
    ));
    let row = work_item_row_for(&mut state, &run_id);
    assert_eq!(
        row["dismissed"], false,
        "removing the speaker cannot restore an invalidated clear: {row:?}"
    );
    let added = state.handle(req("agent.add", json!({ "entity_id": run_id })));
    let second_agent = added["result"]["agent"]["id"].as_str().unwrap().to_string();

    // Reading what the second agent said is not clearing the row away:
    // nothing the human did draws a line under a conversation they never
    // dismissed, and the first agent's line cannot speak for it.
    state.handle(req("entity.seen", json!({ "entity_id": run_id })));
    let row = work_item_row_for(&mut state, &run_id);
    assert_eq!(row["unread"], false, "{row:?}");
    assert_eq!(
        row["dismissed"], false,
        "the row stays on the list until the human clears it again: {row:?}"
    );

    // Clearing it again draws a line under BOTH agents, and now it sticks.
    let cleared = state.handle(req("entity.dismiss", json!({ "entity_id": run_id })));
    assert_eq!(cleared["ok"], true, "{cleared:?}");
    let row = work_item_row_for(&mut state, &run_id);
    assert_eq!(row["dismissed"], true, "{row:?}");

    // And the second agent speaking again brings the row back on its own,
    // exactly as the first agent's would.
    push_to_agent_conversation(&mut state, &run_id, &second_agent, |thread| {
        thread.post_agent("still waiting on you", None, now_rfc3339());
    });
    let row = work_item_row_for(&mut state, &run_id);
    assert_eq!(row["unread"], true, "{row:?}");
    assert_eq!(row["dismissed"], false, "{row:?}");
}

#[test]
fn silent_agent_roster_changes_preserve_a_clear() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (_, run_id) = planned_run_in_review(&mut state, "silent roster");
    state.handle(req("entity.dismiss", json!({ "entity_id": run_id })));

    let added = state.handle(req("agent.add", json!({ "entity_id": run_id })));
    let agent_id = added["result"]["agent"]["id"].as_str().unwrap().to_string();
    assert_eq!(work_item_row_for(&mut state, &run_id)["dismissed"], true);
    state.handle(req(
        "agent.remove",
        json!({ "entity_id": run_id, "agent_id": agent_id }),
    ));
    assert_eq!(work_item_row_for(&mut state, &run_id)["dismissed"], true);
}

/// A row cleared before dismissal was per-agent carries one scalar and no
/// map. It belongs to the agent that inherited the entity's conversation,
/// and that row must still be cleared after this ships.
#[test]
fn an_old_style_dismissal_still_clears_a_single_agent_row() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (task_id, run_id) = planned_run_in_review(&mut state, "written before agents");
    push_to_task_conversation(&mut state, &task_id, |thread| {
        thread.post_agent("which name did you want?", None, now_rfc3339());
    });
    state.handle(req("entity.seen", json!({ "entity_id": run_id })));

    // What the old code wrote: the end of the entity's conversation, in one
    // number, with no agent named.
    let line = primary_thread(&state.runs[&run_id].agents).last_sequence();
    state
        .board
        .attention_mut()
        .set_legacy_dismissed_through(&run_id, line);
    let row = work_item_row_for(&mut state, &run_id);
    assert_eq!(row["dismissed"], true, "{row:?}");

    // And it comes back the same way it always did.
    push_to_task_conversation(&mut state, &task_id, |thread| {
        thread.post_agent("done — take a look", None, now_rfc3339());
    });
    let row = work_item_row_for(&mut state, &run_id);
    assert_eq!(row["dismissed"], false, "{row:?}");
}

/// Clearing draws its line after existing unread messages; those messages
/// do not immediately revive it.
#[test]
fn old_unread_does_not_beat_a_newer_dismissal() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (task_id, run_id) = planned_run_in_review(&mut state, "still asking");
    push_to_task_conversation(&mut state, &task_id, |thread| {
        thread.post_agent("which name did you want?", None, now_rfc3339());
    });

    let cleared = state.handle(req("entity.dismiss", json!({ "entity_id": run_id })));
    assert_eq!(cleared["ok"], true, "{cleared:?}");

    let row = work_item_row_for(&mut state, &run_id);
    assert_eq!(row["unread"], true, "{row:?}");
    assert_eq!(row["dismissed"], true, "{row:?}");
    assert_eq!(
        row["unread_reason"], "agent_message",
        "the question is still waiting, and still says so: {row:?}"
    );
}

/// Two different things the human can do to one row, and neither is the
/// other: mute silences a row that stays, dismiss takes a row out that
/// still shouts when it comes back.
#[test]
fn mute_and_dismiss_are_independent() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (task_id, run_id) = planned_run_in_review(&mut state, "quiet and gone");
    push_to_task_conversation(&mut state, &task_id, |thread| {
        thread.post_agent("which name did you want?", None, now_rfc3339());
    });

    state.handle(req(
        "entity.mute",
        json!({ "entity_id": run_id, "muted": true }),
    ));
    let row = work_item_row_for(&mut state, &run_id);
    assert_eq!(row["muted"], true, "{row:?}");
    assert_eq!(row["dismissed"], false, "muting does not clear a row away");

    let cleared = state.handle(req("entity.dismiss", json!({ "entity_id": run_id })));
    assert_eq!(cleared["ok"], true, "{cleared:?}");
    let row = work_item_row_for(&mut state, &run_id);
    assert_eq!(row["muted"], true, "dismissing does not unsilence: {row:?}");
    assert_eq!(row["dismissed"], true, "{row:?}");

    // Unmuting restores the unread badge but does not itself send a message.
    state.handle(req(
        "entity.mute",
        json!({ "entity_id": run_id, "muted": false }),
    ));
    let row = work_item_row_for(&mut state, &run_id);
    assert_eq!(row["muted"], false, "{row:?}");
    assert_eq!(row["unread"], true, "{row:?}");
    assert_eq!(row["dismissed"], true, "{row:?}");
}

/// A hand-off is machine-to-machine traffic: one agent's words arriving in
/// another agent's conversation, wearing the sender that wrote them. It never
/// crosses the line the human drew — a project agent staffing a workspace
/// must not put back on the list a row the human cleared — and the human or
/// the agent itself speaking still does.
#[test]
fn an_agents_hand_off_never_brings_back_a_cleared_row() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (task_id, run_id) = planned_run_in_review(&mut state, "handed over");
    push_to_task_conversation(&mut state, &task_id, |thread| {
        thread.post_agent("which name did you want?", None, now_rfc3339());
    });
    state.handle(req("entity.seen", json!({ "entity_id": run_id })));
    let cleared = state.handle(req("entity.dismiss", json!({ "entity_id": run_id })));
    assert_eq!(cleared["ok"], true, "{cleared:?}");

    push_to_task_conversation(&mut state, &task_id, |thread| {
        thread.post_user_from_agent(
            "take the retry path next",
            crate::thread::AgentIdentity::new("project-1".to_string()),
            now_rfc3339(),
        );
    });

    let row = work_item_row_for(&mut state, &run_id);
    assert_eq!(
        row["dismissed"], true,
        "one agent handing work to another is not the row speaking: {row:?}"
    );

    // And the dismissal is still there to be crossed: the human speaking
    // brings the row back the way it always did.
    push_to_task_conversation(&mut state, &task_id, |thread| {
        thread.post_user("actually, hold on", None, now_rfc3339());
    });
    let row = work_item_row_for(&mut state, &run_id);
    assert_eq!(row["dismissed"], false, "{row:?}");
}

/// The same line, across a restart. A conversation is loaded as its newest
/// 200 items, so a session's worth of hand-offs buries what the human and
/// the agent said under the tail — and the line has to come out of the store
/// knowing which of those messages were hand-offs. Otherwise the newest
/// message there is is always a machine's, and the row the human cleared
/// comes back at every boot.
#[test]
fn the_line_a_hand_off_does_not_cross_survives_a_restart() {
    let (dir, repo) = init_repo();
    let bury = |state: &mut AppState, run_id: &str, agent_id: &str| {
        state
            .edit_agent_conversation(run_id, agent_id, |thread, _| {
                for index in 0..crate::store::RESIDENT_CONVERSATION_TAIL + 40 {
                    thread.post_user_from_agent(
                        format!("step {index}"),
                        crate::thread::AgentIdentity::new("project-1".to_string()),
                        now_rfc3339(),
                    );
                }
                Ok(())
            })
            .expect("the hand-offs are written");
    };

    let (run_id, agent_id) = {
        let mut state = qa_state(&repo, dir.path());
        let run_id = adopted_run(&mut state, &repo, dir.path(), "handed-over");
        let agent_id = primary_agent_id(&state, &run_id);
        state
            .edit_agent_conversation(&run_id, &agent_id, |thread, _| {
                thread.post_agent("which name did you want?", None, now_rfc3339());
                Ok(())
            })
            .expect("the question is written");
        state.handle(req("entity.seen", json!({ "entity_id": run_id.clone() })));
        let cleared = state.handle(req(
            "entity.dismiss",
            json!({ "entity_id": run_id.clone() }),
        ));
        assert_eq!(cleared["ok"], true, "{cleared:?}");
        bury(&mut state, &run_id, &agent_id);
        let row = work_item_row_for(&mut state, &run_id);
        assert_eq!(row["dismissed"], true, "{row:?}");
        (run_id, agent_id)
    };

    let mut rebooted = qa_state(&repo, dir.path());
    let row = work_item_row_for(&mut rebooted, &run_id);
    assert_eq!(
        row["dismissed"], true,
        "a tail of hand-offs is not the row speaking: {row:?}"
    );

    // And the agent itself speaking still crosses the line, however deeply
    // the traffic after it buries what was said. Nothing reads the board
    // between the two, so the answer has to come back out of the store.
    rebooted
        .edit_agent_conversation(&run_id, &agent_id, |thread, _| {
            thread.post_agent("the retry path is ready — take a look", None, now_rfc3339());
            Ok(())
        })
        .expect("the answer is written");
    bury(&mut rebooted, &run_id, &agent_id);
    drop(rebooted);

    let mut rebooted = qa_state(&repo, dir.path());
    let row = work_item_row_for(&mut rebooted, &run_id);
    assert_eq!(
        row["dismissed"], false,
        "the agent asked for the human and the row is back on the list: {row:?}"
    );
}

#[test]
fn entity_dismiss_refuses_what_it_cannot_clear() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (_, run_id) = planned_run_in_review(&mut state, "bad dismiss");

    let unknown = state.handle(req("entity.dismiss", json!({ "entity_id": "run-nowhere" })));
    assert_eq!(unknown["ok"], false, "{unknown:?}");

    // A real id that names no entry in the inbox: a project is where the
    // work lives, not a row that can be cleared out of the way.
    let project_id = state.project_at(0).id.clone();
    let not_an_entry = state.handle(req("entity.dismiss", json!({ "entity_id": project_id })));
    assert_eq!(not_an_entry["ok"], false, "{not_an_entry:?}");

    let unsaid = state.handle(req("entity.dismiss", json!({})));
    assert_eq!(unsaid["ok"], false, "{unsaid:?}");

    let entry = board_entry(&mut state, &run_id);
    assert_eq!(
        entry["dismissed"], false,
        "a refused call changes nothing: {entry:?}"
    );
}

// ---- dismiss: the rows no entity stands behind --------------------------

/// One commit in a checkout, which is how a row with no conversation says
/// something new.
pub(in crate::app::tests) fn commit_in(checkout: &std::path::Path, message: &str) {
    std::fs::write(checkout.join("worked.txt"), message).unwrap();
    git_in(checkout, &["add", "."]);
    git_in(checkout, &["commit", "-m", message]);
}

/// Adopting a bare worktree is what brings its row onto the feed at all —
/// releasing it hands the worktree back to the human, and the row goes
/// with it, the same as it never having been adopted.
#[test]
fn adopting_a_bare_worktree_brings_its_row_and_releasing_it_takes_it_away() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.project_at(0).id.clone();
    add_external_worktree(&repo, dir.path(), "loose", "loose");
    let worktree_id = state
        .scan_external_worktrees_now(&project_id)
        .unwrap()
        .into_iter()
        .find(|w| w.branch.as_deref() == Some("loose"))
        .expect("the external worktree is discoverable")
        .id;
    assert!(
        work_item_rows(&mut state)
            .iter()
            .all(|row| row["branch"] != json!("loose")),
        "not on the feed until adopted"
    );

    let adopted = state.handle(req(
        "run.adopt",
        json!({ "project_id": project_id, "worktree_id": worktree_id }),
    ));
    assert_eq!(adopted["ok"], true, "{adopted:?}");
    let row = branch_row(&mut state, "loose");
    assert_eq!(row["run_id"], run_id_of(&adopted), "{row:?}");
    assert_eq!(row["dismissed"], false, "{row:?}");

    let released = state.handle(req("run.release", json!({ "run_id": run_id_of(&adopted) })));
    assert_eq!(released["ok"], true, "{released:?}");
    assert!(
        work_item_rows(&mut state)
            .iter()
            .all(|row| row["branch"] != json!("loose")),
        "released back to a bare worktree — off the feed again: {:?}",
        work_item_rows(&mut state)
    );
}

/// A checkout Build never cut has no entity, no conversation and nothing to
/// file away, so commits cannot revive the row once it is cleared. It is
/// reachable by name (`branch.get`) rather than on the inbox, which lists
/// work started in Build.
#[test]
fn clearing_a_bare_checkouts_row_holds_across_its_own_commits() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.project_at(0).id.clone();
    let loose = add_external_worktree(&repo, dir.path(), "loose", "loose");
    state.scan_external_worktrees_now(&project_id).unwrap();

    assert_eq!(
        bare_row(&mut state, &project_id, "loose")["dismissed"],
        false
    );

    let cleared = state.handle(req(
        "entity.dismiss",
        json!({ "project_id": project_id.clone(), "branch": "loose" }),
    ));
    assert_eq!(cleared["ok"], true, "{cleared:?}");
    assert_eq!(cleared["result"]["branch"], "loose", "{cleared:?}");
    assert_eq!(cleared["result"]["dismissed"], true, "{cleared:?}");
    assert_eq!(
        bare_row(&mut state, &project_id, "loose")["dismissed"],
        true
    );

    commit_in(&loose, "landed");
    state.board.diff_mut().clear_external_scan(&project_id);
    assert_eq!(
        bare_row(&mut state, &project_id, "loose")["dismissed"],
        true,
        "a checkout with no conversation cannot speak its row back"
    );
}

/// Clearing one row clears one row. Two projects on the same branch name are
/// two rows.
#[test]
fn a_row_dismissal_names_exactly_one_row() {
    let (dir, repo) = init_repo();
    let other = init_repo_named(dir.path(), "other");
    let mut state = qa_state(&repo, dir.path());
    let added = state.handle(req(
        "project.add",
        json!({ "path": other.to_str().unwrap() }),
    ));
    assert_eq!(added["ok"], true, "{added:?}");
    let project_id = state.project_at(0).id.clone();
    let other_id = added["result"]["project_id"].as_str().unwrap().to_string();
    add_external_worktree(&repo, dir.path(), "loose", "loose");
    add_external_worktree(&other, dir.path(), "other-loose", "loose");
    for id in [&project_id, &other_id] {
        state.scan_external_worktrees_now(id).unwrap();
    }

    state.handle(req(
        "entity.dismiss",
        json!({ "project_id": project_id.clone(), "branch": "loose" }),
    ));

    assert_eq!(
        bare_row(&mut state, &project_id, "loose")["dismissed"],
        true
    );
    assert_eq!(
        bare_row(&mut state, &other_id, "loose")["dismissed"],
        false,
        "the neighbour's row shares a branch name and nothing else"
    );
}

/// A row dismissal survives a restart: the record is the only place it
/// lives, and the row it names has no conversation to rebuild it from.
#[test]
fn a_bare_checkouts_row_dismissal_survives_a_restart() {
    let (dir, repo) = init_repo();
    add_external_worktree(&repo, dir.path(), "loose", "loose");
    let project_id = {
        let mut state = qa_state(&repo, dir.path());
        let project_id = state.project_at(0).id.clone();
        state.scan_external_worktrees_now(&project_id).unwrap();
        let cleared = state.handle(req(
            "entity.dismiss",
            json!({ "project_id": project_id.clone(), "branch": "loose" }),
        ));
        assert_eq!(cleared["ok"], true, "{cleared:?}");
        project_id
    };

    let mut reloaded = qa_state(&repo, dir.path());
    reloaded.scan_external_worktrees_now(&project_id).unwrap();
    assert_eq!(
        bare_row(&mut reloaded, &project_id, "loose")["dismissed"],
        true
    );
}

#[test]
fn a_row_dismissal_refuses_what_it_cannot_clear() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.project_at(0).id.clone();
    add_external_worktree(&repo, dir.path(), "loose", "loose");
    state.scan_external_worktrees_now(&project_id).unwrap();

    let unknown = state.handle(req(
        "entity.dismiss",
        json!({ "project_id": "proj-nowhere", "branch": "loose" }),
    ));
    assert_eq!(unknown["ok"], false, "{unknown:?}");

    // A project named with no branch names no row: the project itself is
    // where work is cut from, not a row.
    let unsaid = state.handle(req(
        "entity.dismiss",
        json!({ "project_id": project_id.clone() }),
    ));
    assert_eq!(unsaid["ok"], false, "{unsaid:?}");

    // Nor does a branch no checkout of this project is on.
    let nowhere = state.handle(req(
        "entity.dismiss",
        json!({ "project_id": project_id.clone(), "branch": "never-checked-out" }),
    ));
    assert_eq!(nowhere["ok"], false, "{nowhere:?}");

    assert_eq!(
        bare_row(&mut state, &project_id, "loose")["dismissed"],
        false,
        "a refused call changes nothing"
    );
}

/// The row of a checkout Build never cut, read the one way a client can
/// reach it: by name. The inbox lists work started in Build, so it is not
/// there.
fn bare_row(state: &mut AppState, project_id: &str, branch: &str) -> Value {
    let got = state.handle(req(
        "branch.get",
        json!({ "project_id": project_id, "branch": branch }),
    ));
    assert_eq!(got["ok"], true, "{got:?}");
    got["result"].clone()
}

/// A planned run and its Task share one conversation. One piece of news on
/// it is one notification, so whichever mutation tail runs second must find
/// nothing new — otherwise every done report pushes twice.
#[test]
fn one_piece_of_news_reaches_the_push_funnel_once() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (task_id, run_id) = planned_run_in_review(&mut state, "one push");
    push_to_task_conversation(&mut state, &task_id, |thread| {
        thread.push_event(
            crate::thread::ThreadEventKind::Done,
            Some("Implemented the change".to_string()),
            None,
            None,
            now_rfc3339(),
        );
    });

    let conversation = state.runs[&run_id].agents.sole_thread().clone();
    let news = state.conversation_news(&conversation);
    assert_eq!(news.attention_reason, Some("done"));
    state.push_agent_news(
        &run_id,
        vec![crate::app::board::attention::AgentNews {
            agent_id: state.runs[&run_id].agents.sole().id.clone(),
            news,
            watched: true,
        }],
    );
    assert_eq!(
        state.conversation_news(&conversation).attention_reason,
        None,
        "the second tail finds the news already taken"
    );
}

/// Everything in the store was announced when it happened. A restart
/// re-reads all of it and must announce none of it again.
#[test]
fn a_restart_announces_nothing_it_already_announced() {
    let (dir, repo) = init_repo();
    let run_id = {
        let mut state = qa_state(&repo, dir.path());
        planned_run_in_review(&mut state, "quiet restart").1
    };
    let reloaded = qa_state(&repo, dir.path());
    let conversation = reloaded.runs[&run_id].agents.sole_thread().clone();
    assert_eq!(
        reloaded.conversation_news(&conversation).attention_reason,
        None
    );
}

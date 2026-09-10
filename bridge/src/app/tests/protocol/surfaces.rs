use super::*;

fn a_branch_whose_agent_runs(
    branch: &str,
    session: DictatedSession,
) -> (tempfile::TempDir, AppState, String) {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let run_id = adopted_run(&mut state, &repo, dir.path(), branch);
    let agent_id = primary_agent_id(&state, &run_id);
    let root = state
        .entity_agent_root(&run_id)
        .expect("the branch has a checkout");
    insert_agent_tab(&mut state, &root, &run_id, &agent_id, session);
    (dir, state, run_id)
}

fn agent_digests_within(payload: &Value) -> Vec<Value> {
    match payload {
        Value::Object(fields) => fields
            .iter()
            .flat_map(|(name, value)| match name.as_str() {
                "agents" => value.as_array().cloned().unwrap_or_default(),
                "agent" if value.is_object() => vec![value.clone()],
                _ => agent_digests_within(value),
            })
            .collect(),
        Value::Array(entries) => entries.iter().flat_map(agent_digests_within).collect(),
        _ => Vec::new(),
    }
}

fn assert_no_digest_carries_surfaces(verb: &str, payload: &Value) {
    let digests = agent_digests_within(payload);
    assert!(
        !digests.is_empty(),
        "{verb} answered with no agent digest at all, so it pins nothing: {payload:?}"
    );
    for digest in digests {
        assert!(
            digest.get("surfaces").is_none(),
            "{verb} carried surfaces on a list-shaped answer: {digest:?}"
        );
    }
}

#[test]
fn branch_get_carries_the_open_agents_surfaces() {
    let (dir, mut state, run_id) = a_branch_whose_agent_runs(
        "feature-surfaced",
        DictatedSession::reporting(AgentStatus::Working)
            .showing_surfaces(recorded_workflow_surfaces()),
    );
    let agent_id = primary_agent_id(&state, &run_id);
    let project_id = state.project_at(0).id.clone();

    let answered = state.handle(req(
        "branch.get",
        json!({
            "project_id": project_id,
            "branch": "feature-surfaced",
            "agent_id": agent_id,
            "thread_after_sequence": 0
        }),
    ));

    assert_eq!(answered["ok"], true, "{answered:?}");
    let row = &answered["result"];
    assert_eq!(
        row["run"]["agents"][0]["surfaces"]["workflows"][0]["name"], "readme-analysis",
        "{row:?}"
    );
    assert_eq!(
        row["agents"][0]["surfaces"]["subagents"][0]["id"], SUBAGENT_TASK_ID,
        "the rail reads a branch's agents off the row: {row:?}"
    );

    let repo_path = state.project_at(0).repo_path.clone();
    add_external_worktree(&repo_path, dir.path(), "bare-checkout", "feature-bare");
    state
        .scan_external_worktrees_now(&project_id)
        .expect("the new checkout is discoverable");

    let bare = state.handle(req(
        "branch.get",
        json!({ "project_id": project_id, "branch": "feature-bare" }),
    ));
    assert_eq!(bare["ok"], true, "{bare:?}");
    let bare = &bare["result"];
    assert!(bare["run"].is_null(), "{bare:?}");
    assert_eq!(
        bare["agents"],
        json!([]),
        "a bare checkout has no run view to take agents from, so the rail still reads the candidate row's own array: {bare:?}"
    );
    drop(dir);
}

fn the_only_digest_carrying_surfaces(verb: &str, payload: &Value) -> Value {
    let surfaced: Vec<Value> = agent_digests_within(payload)
        .into_iter()
        .filter_map(|digest| digest.get("surfaces").cloned())
        .collect();
    assert_eq!(
        surfaced.len(),
        1,
        "{verb} answered with {} surfaced digests: {payload:?}",
        surfaced.len()
    );
    surfaced[0].clone()
}

#[test]
fn every_detail_verb_carries_the_open_agents_surfaces() {
    let (dir, mut state, run_id) = a_branch_whose_agent_runs(
        "feature-detailed",
        DictatedSession::reporting(AgentStatus::Working)
            .showing_surfaces(recorded_workflow_surfaces()),
    );
    let issue_id = plan_id_of(&state.handle(req(
        "plan.create",
        json!({ "goal": "an issue with a surfaced agent" }),
    )));
    let started = state.handle(req(
        "thread.post",
        json!({ "entity_id": issue_id, "body": "start" }),
    ));
    assert_eq!(started["ok"], true, "{started:?}");
    let issue_agent = primary_agent_id(&state, &issue_id);
    let issue_root = state
        .entity_agent_root(&issue_id)
        .expect("the issue's agent works in the primary checkout");
    insert_agent_tab(
        &mut state,
        &issue_root,
        &issue_id,
        &issue_agent,
        DictatedSession::reporting(AgentStatus::Working)
            .showing_surfaces(recorded_workflow_surfaces()),
    );

    for (verb, params) in [
        ("run.get", json!({ "run_id": run_id })),
        ("issue.get", json!({ "issue_id": issue_id })),
        ("plan.get", json!({ "plan_id": issue_id })),
    ] {
        let answered = state.handle(req(verb, params));
        assert_eq!(answered["ok"], true, "{verb}: {answered:?}");
        let surfaces = the_only_digest_carrying_surfaces(verb, &answered["result"]);
        assert_eq!(
            surfaces["workflows"][0]["id"], WORKFLOW_TASK_ID,
            "{verb}: {surfaces:?}"
        );
    }
    drop(dir);
}

#[test]
fn an_entity_no_roster_knows_answers_with_no_digests_at_all() {
    let (dir, repo) = init_repo();
    let state = qa_state(&repo, dir.path());

    assert!(state
        .agent_digests("no-such-entity", DigestScope::Detail)
        .is_empty());
    drop(dir);
}

#[test]
fn no_list_shaped_answer_carries_surfaces() {
    let (dir, mut state, run_id) = a_branch_whose_agent_runs(
        "feature-listed",
        DictatedSession::reporting(AgentStatus::Working)
            .showing_surfaces(recorded_workflow_surfaces()),
    );
    let issue_id = plan_id_of(&state.handle(req(
        "plan.create",
        json!({ "goal": "an issue with a surfaced agent" }),
    )));
    let started = state.handle(req(
        "thread.post",
        json!({ "entity_id": issue_id, "body": "start" }),
    ));
    assert_eq!(started["ok"], true, "{started:?}");
    let issue_agent = primary_agent_id(&state, &issue_id);
    let issue_root = state
        .entity_agent_root(&issue_id)
        .expect("the issue's agent works in the primary checkout");
    insert_agent_tab(
        &mut state,
        &issue_root,
        &issue_id,
        &issue_agent,
        DictatedSession::reporting(AgentStatus::Working)
            .showing_surfaces(recorded_workflow_surfaces()),
    );
    state
        .plans
        .get_mut(&issue_id)
        .expect("the issue is on the board")
        .plan
        .archived_at = Some(now_rfc3339());
    let project_id = state.project_at(0).id.clone();

    for (verb, params) in [
        ("board.list", json!({})),
        ("plan.list", json!({})),
        ("issue.list", json!({})),
        ("archive.list", json!({ "project_id": project_id })),
        ("agent.list", json!({ "entity_id": run_id })),
    ] {
        let answered = state.handle(req(verb, params));
        assert_eq!(answered["ok"], true, "{verb}: {answered:?}");
        assert_no_digest_carries_surfaces(verb, &answered["result"]);
    }

    let added = state.handle(req("agent.add", json!({ "entity_id": run_id })));
    assert_eq!(added["ok"], true, "{added:?}");
    assert_no_digest_carries_surfaces("agent.add", &added["result"]);
    let second_agent = added["result"]["agent"]["id"]
        .as_str()
        .expect("the added agent has an id")
        .to_string();

    let removed = state.handle(req(
        "agent.remove",
        json!({ "entity_id": run_id, "agent_id": second_agent }),
    ));
    assert_eq!(removed["ok"], true, "{removed:?}");
    assert_no_digest_carries_surfaces("agent.remove", &removed["result"]);
    drop(dir);
}

#[test]
fn thread_post_answers_with_the_surfaces_the_rail_repaints_from() {
    let (dir, mut state, run_id) = a_branch_whose_agent_runs(
        "feature-posted",
        DictatedSession::reporting(AgentStatus::Working)
            .showing_surfaces(recorded_workflow_surfaces()),
    );

    let posted = state.handle(req(
        "thread.post",
        json!({ "entity_id": run_id, "body": "how is the workflow going?", "thread_limit": 20 }),
    ));

    assert_eq!(posted["ok"], true, "{posted:?}");
    assert_eq!(
        posted["result"]["agents"][0]["surfaces"]["workflows"][0]["id"], WORKFLOW_TASK_ID,
        "{posted:?}"
    );
    drop(dir);
}

#[test]
fn an_agent_whose_session_reports_no_surfaces_carries_no_key() {
    let (dir, mut state, run_id) = a_branch_whose_agent_runs(
        "feature-silent",
        DictatedSession::reporting(AgentStatus::Working),
    );

    let got = state.handle(req(
        "branch.get",
        json!({
            "project_id": state.project_at(0).id.clone(),
            "branch": "feature-silent"
        }),
    ));

    assert_eq!(got["ok"], true, "{got:?}");
    let digest = &got["result"]["run"]["agents"][0];
    assert_eq!(digest["id"], primary_agent_id(&state, &run_id), "{got:?}");
    assert!(
        digest.get("surfaces").is_none(),
        "not even an empty object: {digest:?}"
    );
    drop(dir);
}

#[tokio::test]
async fn a_subagents_call_sequence_names_the_row_of_the_call_that_spawned_it() {
    let (_dir, state, key) = a_run_with_a_dictated_tab(
        "run-spawning",
        DictatedSession::reporting(AgentStatus::Working)
            .showing_surfaces(recorded_workflow_surfaces()),
    );
    let (activity, subscribed) = broadcast::channel(4);
    spawn_activity_pump(&state, key.clone(), Some(subscribed));
    activity
        .send(crate::harness::ActivityReport::own_work(
            crate::harness::AgentActivity::ToolUse {
                call_id: SUBAGENT_SPAWNING_CALL_ID.to_string(),
                summary: "Agent Read README.md and report character count".to_string(),
            },
        ))
        .expect("the pump is listening");
    wait_for(Duration::from_secs(5), || {
        let s = state.lock().unwrap();
        (!tool_call_rows(primary_thread(&s.runs["run-spawning"].agents)).is_empty()).then_some(())
    })
    .await
    .expect("the Agent call mints a row");

    let while_running = state
        .lock()
        .unwrap()
        .handle(req("run.get", json!({ "run_id": "run-spawning" })));
    assert_eq!(
        while_running["result"]["agents"][0]["surfaces"]["subagents"][0]["call_sequence"],
        json!(spawning_call_sequence(&while_running["result"])),
        "{while_running:?}"
    );

    activity
        .send(crate::harness::ActivityReport::own_work(
            crate::harness::AgentActivity::ToolResult {
                call_id: SUBAGENT_SPAWNING_CALL_ID.to_string(),
                outcome: crate::harness::ToolOutcome::Ok,
                summary: "4".to_string(),
            },
        ))
        .expect("the pump is listening");
    wait_for(Duration::from_secs(5), || {
        let s = state.lock().unwrap();
        tool_call_rows(primary_thread(&s.runs["run-spawning"].agents))
            .first()
            .filter(|row| row.outcome.is_some())
            .cloned()
    })
    .await
    .expect("the answer lands on the call's own row");

    let after_answer = state
        .lock()
        .unwrap()
        .handle(req("run.get", json!({ "run_id": "run-spawning" })));
    assert_eq!(
        after_answer["result"]["agents"][0]["surfaces"]["subagents"][0]["call_sequence"],
        json!(spawning_call_sequence(&after_answer["result"])),
        "the pairing outlives the answer it was minted for: {after_answer:?}"
    );
}

fn note_the_board_then_bump_each_revision_once(
    state: &Arc<Mutex<AppState>>,
    revisions: &[SurfaceRevision],
) {
    let state = state.lock().unwrap();
    state.note_board_changed();
    for revision in revisions {
        revision.bump();
    }
}

fn spawning_call_sequence(view: &Value) -> u64 {
    view["thread"]["items"]
        .as_array()
        .expect("the view carries its conversation")
        .iter()
        .find(|item| {
            item["data"]["summary"]
                .as_str()
                .unwrap_or_default()
                .starts_with("Agent ")
        })
        .expect("the Agent call has a row in this payload")["data"]["sequence"]
        .as_u64()
        .expect("a row carries its sequence")
}

#[tokio::test]
async fn a_revision_bump_that_mints_no_row_stales_the_owning_entity() {
    let (dir, repo) = init_repo();
    let (state, _handler, _sender, mut rx, session_key) = greeted_push_session(&repo, dir.path());
    let root = {
        let mut s = state.lock().unwrap();
        insert_run(
            &mut s,
            &repo,
            dir.path(),
            "run-invalidated",
            RunState::Building,
        )
    };
    let revisions: Vec<SurfaceRevision> = (0..5).map(|_| SurfaceRevision::default()).collect();
    let agent_ids = {
        let mut s = state.lock().unwrap();
        let roster = &mut s.runs.get_mut("run-invalidated").unwrap().agents;
        let mut ids = vec![roster.primary().unwrap().id.clone()];
        for _ in 1..revisions.len() {
            ids.push(
                roster
                    .add("run-invalidated", ModelChoice::default(), &now_rfc3339())
                    .id
                    .clone(),
            );
        }
        ids
    };
    let activity_senders: Vec<_> = revisions
        .iter()
        .zip(agent_ids)
        .map(|(revision, agent_id)| {
            let key = {
                let mut s = state.lock().unwrap();
                insert_agent_tab(
                    &mut s,
                    &root,
                    "run-invalidated",
                    &agent_id,
                    DictatedSession::reporting(AgentStatus::Working)
                        .moving_surfaces_on(revision.clone()),
                )
            };
            let (activity, subscribed) = broadcast::channel(4);
            spawn_activity_pump(&state, key, Some(subscribed));
            activity
        })
        .collect();
    settled_pushes(&mut rx, &session_key).await;

    note_the_board_then_bump_each_revision_once(&state, &revisions);

    let events = change_events(&settled_pushes(&mut rx, &session_key).await);
    assert_eq!(
        events
            .iter()
            .filter(|event| **event == json!({ "type": "entity.changed", "id": "run-invalidated" }))
            .count(),
        1,
        "five separately watched bumps inside one window are one stale-detail event: {events:?}"
    );
    let s = state.lock().unwrap();
    assert!(
        activity_rows(primary_thread(&s.runs["run-invalidated"].agents)).is_empty(),
        "and they minted no conversation row on the way"
    );
    drop(s);
    drop(activity_senders);
}

#[tokio::test]
async fn the_pump_ends_with_the_activity_stream_though_the_revision_stays_open() {
    let revision = SurfaceRevision::default();
    let (_dir, state, key) = a_run_with_a_dictated_tab(
        "run-outlived",
        DictatedSession::reporting(AgentStatus::Working).moving_surfaces_on(revision.clone()),
    );
    let before_pump = Arc::strong_count(&state);
    let (activity, subscribed) = broadcast::channel(4);
    spawn_activity_pump(&state, key.clone(), Some(subscribed));
    assert_eq!(
        Arc::strong_count(&state),
        before_pump + 1,
        "the pump holds the state for as long as it runs"
    );

    drop(activity);

    wait_for(Duration::from_secs(5), || {
        (!state
            .lock()
            .unwrap()
            .session_registry
            .test_tab(&key)
            .unwrap()
            .live)
            .then_some(())
    })
    .await
    .expect("the stream closing ends the session with the revision channel still open");
    revision.bump();
    wait_for(Duration::from_secs(5), || {
        (Arc::strong_count(&state) == before_pump).then_some(())
    })
    .await
    .expect("and the task is gone rather than parked on a channel nobody will close");
}

#[tokio::test]
async fn a_revision_channel_that_closes_leaves_the_pump_reading_activity() {
    let (revision_sender, watched) = tokio::sync::watch::channel(0u64);
    let (_dir, state, key) = a_run_with_a_dictated_tab(
        "run-unwatchable",
        DictatedSession::reporting(AgentStatus::Working)
            .watching_a_revision_the_caller_can_close(watched),
    );
    let before_pump = Arc::strong_count(&state);
    let (activity, subscribed) = broadcast::channel(4);
    spawn_activity_pump(&state, key.clone(), Some(subscribed));

    drop(revision_sender);
    activity
        .send(crate::harness::ActivityReport::own_work(
            crate::harness::AgentActivity::Narration {
                summary: "still reading".to_string(),
            },
        ))
        .expect("the pump is listening");

    wait_for(Duration::from_secs(5), || {
        let s = state.lock().unwrap();
        let rows = activity_rows(primary_thread(&s.runs["run-unwatchable"].agents));
        (!rows.is_empty()).then_some(())
    })
    .await
    .expect("a revision channel nobody can watch drops to activity-only, it does not end the pump");
    assert_eq!(
        Arc::strong_count(&state),
        before_pump + 1,
        "and the pump is still holding the state it reads into"
    );
    drop(activity);
}

#[tokio::test]
async fn an_io_with_no_revision_channel_pumps_exactly_as_it_did() {
    let (_dir, state, key) = a_run_with_a_dictated_tab(
        "run-unrevised",
        DictatedSession::reporting(AgentStatus::Working),
    );
    assert!(
        state
            .lock()
            .unwrap()
            .session_registry
            .test_tab(&key)
            .unwrap()
            .session
            .surfaces_changed()
            .is_none(),
        "this session protocol says nothing about surfaces"
    );

    let (activity, subscribed) = broadcast::channel(4);
    spawn_activity_pump(&state, key.clone(), Some(subscribed));
    activity
        .send(crate::harness::ActivityReport::own_work(
            crate::harness::AgentActivity::Narration {
                summary: "dropped the index".to_string(),
            },
        ))
        .expect("the pump is listening");
    wait_for(Duration::from_secs(5), || {
        let s = state.lock().unwrap();
        let rows = activity_rows(primary_thread(&s.runs["run-unrevised"].agents));
        (!rows.is_empty()).then_some(rows)
    })
    .await
    .expect("the report still reaches the conversation");

    drop(activity);
    wait_for(Duration::from_secs(5), || {
        (!state
            .lock()
            .unwrap()
            .session_registry
            .test_tab(&key)
            .unwrap()
            .live)
            .then_some(())
    })
    .await
    .expect("and the stream closing still ends the session");
}

/// Step 12's live claim, end to end on the real wire: a real haiku turn
/// runs one tool, and the daemon's whole path — reader, pump, conversation
/// — yields ONE row, minted at the call and completed in place when the
/// real answer arrives: `updated_sequence` moved, the answer a suffix line,
/// and no standalone `tool_result` row minted anywhere in the turn.
///
/// Ignored by default for the same reason as the `real_adk` legs in
/// `harness::adk`, and run by the same hand:
///
/// ```text
/// cargo test --lib real_adk -- --ignored --nocapture
/// ```
#[tokio::test]
#[ignore = "spawns the real claude binary; needs auth + network + a model turn"]
async fn real_adk_tool_call_completes_its_own_row() {
    use crate::harness::Harness;

    let (dir, repo) = init_repo();
    let (state, handler) = shared_state_and_handler(&repo, dir.path());
    let root = insert_run_without_agent(&state, &repo, dir.path().join("side"), "run-live-call");
    crate::harness::adk::AdkHarness.prepare_workspace(&root);
    let mcp = dir.path().join("mcp.json");
    std::fs::write(
        &mcp,
        serde_json::to_vec_pretty(&json!({ "mcpServers": {} })).unwrap(),
    )
    .unwrap();
    let spec = HarnessSpec::new("claude")
        .arg("-p")
        .arg("--input-format")
        .arg("stream-json")
        .arg("--output-format")
        .arg("stream-json")
        .arg("--verbose")
        .arg("--mcp-config")
        .arg(mcp.to_string_lossy())
        .arg("--strict-mcp-config")
        .arg("--dangerously-skip-permissions")
        .arg("--model")
        .arg("haiku");
    run_on_a_headless_provider(&state, &repo, "run-live-call", spec);

    let posted = call(
        &handler,
        "thread.post",
        json!({ "entity_id": "run-live-call", "body": concat!(
            "You are being driven by an automated test. Do exactly this and ",
            "nothing else, then stop. Use the Bash tool exactly ONCE, to run ",
            "exactly: echo pear\n",
            "Then reply with the single word done and stop.",
        ) }),
    );
    assert_eq!(posted["ok"], true, "{posted:?}");

    // The whole turn: rows exist and every one of them has closed. The
    // drain at the result closes anything unanswered, so an outcome still
    // absent past this wait would be a pairing that never landed.
    let rows = wait_for(Duration::from_secs(240), || {
        let s = state.lock().unwrap();
        let rows = tool_call_rows(primary_thread(&s.runs["run-live-call"].agents));
        (!rows.is_empty() && rows.iter().all(|row| row.outcome.is_some())).then_some(rows)
    })
    .await
    .expect("the live call closes on its own row");
    eprintln!("[verdict] tool-call rows: {rows:?}");

    assert_eq!(rows.len(), 1, "one call was asked for, one row: {rows:?}");
    let row = &rows[0];
    assert_eq!(row.outcome, Some(crate::thread::ToolCallOutcome::Ok));
    assert!(
        row.updated_sequence > row.sequence,
        "minted at the call and completed in place, so the bump moved: {row:?}"
    );
    let summary = row.summary.as_deref().unwrap_or_default();
    assert!(
        summary.contains("\n→ "),
        "the real answer landed as the suffix line: {summary:?}"
    );
    let reported = activity_of(primary_thread(
        &state.lock().unwrap().runs["run-live-call"].agents,
    ));
    assert!(
        !reported
            .iter()
            .any(|(kind, _)| *kind == crate::thread::ThreadEventKind::ToolResult),
        "and no standalone tool_result row was minted anywhere: {reported:?}"
    );
}

use super::*;

// ---- adopt / release / delete --------------------------------------------

/// Adoption is git and records: it takes ownership of a checkout, and it
/// speaks to nobody. The branch arrives on the board with no agents at all
/// — its chat tab is the new-agent view — and every surface answers for it
/// without reaching for an agent that is not there.
#[test]
fn run_adopt_mints_no_agent_and_every_surface_still_answers() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.project_at(0).id.clone();
    add_external_worktree(&repo, dir.path(), "feature-agentless", "feature-agentless");
    let worktree_id = state
        .scan_external_worktrees_now(&project_id)
        .unwrap()
        .into_iter()
        .find(|w| w.branch.as_deref() == Some("feature-agentless"))
        .expect("the external worktree is discoverable")
        .id;

    let adopted = state.handle(req(
        "run.adopt",
        json!({ "project_id": project_id, "worktree_id": worktree_id }),
    ));
    assert_eq!(adopted["ok"], true, "{adopted:?}");
    let run_id = run_id_of(&adopted);
    assert!(
        state.runs[&run_id].agents.is_empty(),
        "adoption speaks to nobody, so it creates nobody"
    );

    let got = state.handle(req(
        "branch.get",
        json!({ "project_id": project_id, "branch": "feature-agentless" }),
    ));
    assert_eq!(got["ok"], true, "{got:?}");
    assert_eq!(
        got["result"]["agents"].as_array().unwrap().len(),
        0,
        "{got:?}"
    );
    let listed = state.handle(req("agent.list", json!({ "entity_id": run_id })));
    assert_eq!(listed["result"]["agents"].as_array().unwrap().len(), 0);
    // The board reads a row off it, and the idle sweep walks past it,
    // rather than either one reaching for an agent that is not there —
    // even while the record says the branch is working, which is the arm
    // that used to read the roster's first agent unconditionally.
    let board = state.handle(req("board.list", json!({})));
    assert_eq!(board["ok"], true, "{board:?}");
    state.runs.get_mut(&run_id).unwrap().run.state = RunState::Building;
    assert!(
        state.mark_idle_tasks(QUIET_THRESHOLD).is_empty(),
        "nothing that does not exist can have gone idle"
    );
}

/// A post to an agentless branch is what creates the agent that hears it,
/// on the account's default harness, and that agent is the primary.
#[test]
fn a_post_to_an_agentless_branch_creates_the_agent_that_hears_it() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    state.handle(req(
        "settings.set",
        json!({ "default_harness": "claude_adk" }),
    ));
    let run_id = adopted_run(&mut state, &repo, dir.path(), "feature-first-word");
    // Back to the state adoption leaves: the fixture's agent goes away.
    let planted = primary_agent_id(&state, &run_id);
    state.handle(req(
        "agent.remove",
        json!({ "entity_id": run_id, "agent_id": planted }),
    ));
    assert!(state.runs[&run_id].agents.is_empty());

    let posted = state.handle(req(
        "thread.post",
        json!({ "entity_id": run_id, "body": "have a look at this" }),
    ));
    assert_eq!(posted["ok"], true, "{posted:?}");
    let roster = &state.runs[&run_id].agents;
    assert_eq!(roster.len(), 1);
    let minted = roster.primary().expect("the post created one");
    assert_eq!(minted.choice.provider, AgentProvider::ClaudeAdk);
    assert_eq!(minted.ordinal, 1);
    assert!(
        state
            .delivery_queue
            .queued()
            .any(|turn| turn.owner == run_id && turn.agent_id == minted.id),
        "and the turn is addressed to it"
    );
}

/// The new-agent view's flagship send: a branch with no agents gets its
/// first one on the harness the human picked out of the cards. That harness
/// is the branch's from then on — the entity was adopted on the account's
/// default, and nothing may respawn this agent on it.
#[tokio::test]
async fn adding_the_first_agent_on_a_named_harness_moves_the_branch_onto_it() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_qa_state_and_handler(&repo, dir.path());
    let project_id = state.lock().unwrap().project_at(0).id.clone();
    add_external_worktree(&repo, dir.path(), "first-agent", "first-agent");
    let worktree_id = external_id(&mut state.lock().unwrap(), &project_id, Some("first-agent"));
    let adopted = call(
        &handler,
        "run.adopt",
        json!({ "project_id": project_id, "worktree_id": worktree_id }),
    );
    let run_id = run_id_of(&adopted);

    let added = call(
        &handler,
        "agent.add",
        json!({ "entity_id": run_id, "provider": "claude" }),
    );
    assert_eq!(added["ok"], true, "{added:?}");
    let agent_id = added["result"]["agent"]["id"].as_str().unwrap().to_string();
    assert_eq!(
        state.lock().unwrap().runs[&run_id].model_choice.provider,
        AgentProvider::Claude,
        "the branch had nobody, so its first agent's harness is the branch's"
    );

    // The TUI pane's Resume: no provider named, because the agent is
    // locked to one. It must come back on the harness it was created on.
    let started = call(
        &handler,
        "agent.start",
        json!({ "id": run_id, "agent_id": agent_id }),
    );
    assert_eq!(started["ok"], true, "{started:?}");
    wait_for_deliveries(&state).await;
    let root = AppState::canonical_root(&state.lock().unwrap().runs[&run_id].worktree.path);
    assert_eq!(
        spawned_provider(&state, &root, &agent_id),
        AgentProvider::Claude,
        "a bare start respawns the harness the agent is locked to"
    );
}

/// A branch running two harnesses: the entity carries one choice and the
/// agents carry their own. A start that names the second agent respawns
/// THAT agent's harness — spending the branch's would hand its conversation
/// to a different provider.
#[tokio::test]
async fn a_start_respawns_the_named_agents_harness_on_a_mixed_branch() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_qa_state_and_handler(&repo, dir.path());
    let project_id = state.lock().unwrap().project_at(0).id.clone();
    add_external_worktree(&repo, dir.path(), "mixed-branch", "mixed-branch");
    let worktree_id = external_id(
        &mut state.lock().unwrap(),
        &project_id,
        Some("mixed-branch"),
    );
    let adopted = call(
        &handler,
        "run.adopt",
        json!({ "project_id": project_id, "worktree_id": worktree_id }),
    );
    let run_id = run_id_of(&adopted);
    call(&handler, "agent.add", json!({ "entity_id": run_id }));
    let added = call(
        &handler,
        "agent.add",
        json!({ "entity_id": run_id, "provider": "codex" }),
    );
    let second = added["result"]["agent"]["id"].as_str().unwrap().to_string();

    let started = call(
        &handler,
        "agent.start",
        json!({ "id": run_id, "agent_id": second }),
    );
    assert_eq!(started["ok"], true, "{started:?}");
    wait_for_deliveries(&state).await;
    let root = AppState::canonical_root(&state.lock().unwrap().runs[&run_id].worktree.path);
    assert_eq!(
        spawned_provider(&state, &root, &second),
        AgentProvider::Codex
    );
    assert_eq!(
        state.lock().unwrap().runs[&run_id].model_choice.provider,
        AgentProvider::ClaudeAdk,
        "and the branch's own choice — the primary's harness — did not move"
    );
}

/// The same rule for a turn Build queues rather than a start the human
/// pressed — for the agent a verb NAMES, and for the primary a verb that
/// names none reaches. Either way the harness is the agent's own; what the
/// branch is set to is only the primary's, and here not even that.
#[test]
fn a_queued_turn_spends_the_agents_own_harness() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (_issue_id, run_id) = planned_run_in_review(&mut state, "review on two harnesses");
    let branch_choice = state.runs[&run_id].model_choice.clone();
    let planted = primary_agent_id(&state, &run_id);
    let added = state.handle(req(
        "agent.add",
        json!({ "entity_id": run_id, "provider": "codex" }),
    ));
    let codex_agent = added["result"]["agent"]["id"].as_str().unwrap().to_string();

    // Addressed to the agent whose bubble the reviewer had open.
    let addressed = state.handle(req(
        "run.request_changes",
        json!({
            "run_id": run_id,
            "agent_id": codex_agent,
            "messages": [{ "body": "tighten the parser", "anchor": null }]
        }),
    ));
    assert_eq!(addressed["ok"], true, "{addressed:?}");
    let queued = state
        .delivery_queue
        .queued()
        .find(|turn| turn.agent_id == codex_agent)
        .expect("the turn is addressed to the agent the comments named");
    assert_eq!(
        queued.model_choice.provider,
        AgentProvider::Codex,
        "the branch's harness is not this agent's, and the turn is this agent's"
    );
    assert_eq!(
        state.runs[&run_id].model_choice.provider, branch_choice.provider,
        "and nothing moved the branch's own choice"
    );

    // And with the first agent gone, the codex agent IS the primary — the
    // agent every verb that names none now reaches.
    state.delivery_queue.clear_queued();
    let removed = state.handle(req(
        "agent.remove",
        json!({ "entity_id": run_id, "agent_id": planted }),
    ));
    assert_eq!(removed["ok"], true, "{removed:?}");
    // Off the review gate, where a freeform message is what the branch
    // hears next.
    state.runs.get_mut(&run_id).unwrap().run.state = RunState::Building;
    let messaged = state.handle(req(
        "run.message",
        json!({ "run_id": run_id, "message": "pick this back up" }),
    ));
    assert_eq!(messaged["ok"], true, "{messaged:?}");
    let queued = state
        .delivery_queue
        .queued()
        .find(|turn| turn.agent_id == codex_agent)
        .expect("the turn is addressed to the agent that is left");
    assert_eq!(queued.model_choice.provider, AgentProvider::Codex);
}

/// A dispatch is the system about to speak, so it mints its agent where
/// the choice is in hand — on the provider the dispatch named, not the
/// account's default.
#[test]
fn a_dispatched_branch_mints_its_agent_on_the_dispatchs_own_choice() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.project_at(0).id.clone();
    let dispatched = state.handle(req(
        "branch.dispatch",
        json!({
            "project_id": project_id,
            "instruction": "add the export",
            "provider": "codex",
        }),
    ));
    assert_eq!(dispatched["ok"], true, "{dispatched:?}");
    let run_id = dispatched["result"]["run_id"].as_str().unwrap().to_string();
    let roster = &state.runs[&run_id].agents;
    assert_eq!(roster.len(), 1, "one agent, the one being spoken to");
    let agent = roster.primary().unwrap();
    assert_eq!(agent.choice.provider, AgentProvider::Codex);
    assert_eq!(
        dispatched["result"]["agent_id"].as_str(),
        Some(agent.id.as_str())
    );
    assert!(
        agent.thread.items.iter().any(
            |item| matches!(item, crate::thread::ThreadItem::Message(message)
                if message.body == "add the export")
        ),
        "the instruction is on its conversation: {:?}",
        agent.thread.items
    );
}

#[test]
fn run_adopt_release_and_delete() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.project_at(0).id.clone();
    let _ext_path = add_external_worktree(&repo, dir.path(), "feature-x", "feature-x");
    // Resolve the scanner-minted worktree id (match by branch; the scanner
    // canonicalizes paths, which differ from the raw join on macOS).
    let worktree_id = state
        .scan_external_worktrees_now(&project_id)
        .unwrap()
        .into_iter()
        .find(|w| w.branch.as_deref() == Some("feature-x"))
        .expect("the external worktree is discoverable")
        .id;
    let adopted = state.handle(req(
        "run.adopt",
        json!({ "project_id": project_id, "worktree_id": worktree_id }),
    ));
    assert_eq!(adopted["result"]["state"], "review", "{adopted:?}");
    assert_eq!(adopted["result"]["adopted"], true);
    let run_id = run_id_of(&adopted);
    // Build's agent in that worktree reports `done` for THIS run.
    let root = AppState::canonical_root(&state.runs[&run_id].worktree.path);
    let (tab, _rx) = Tab::spawn_agent(
        run_id.clone(),
        crate::agent::derived_agent_id(&run_id),
        test_agent_session_request(
            AgentProvider::default(),
            warm_tui_spec(),
            root.clone(),
            terminal_size(120, 40),
        ),
    )
    .unwrap();
    let agent_pid = agent_pid(&tab).expect("a live agent");
    state
        .session_registry
        .test_insert_tab(derived_agent_key(&root, &run_id), tab);

    // Release drops the record, keeps the files.
    let released = state.handle(req("run.release", json!({ "run_id": run_id })));
    assert_eq!(released["ok"], true, "{released:?}");
    assert!(!state.runs.contains_key(&run_id));
    assert!(
        root.exists(),
        "un-adopting must never touch the user's files"
    );
    // …and takes Build's agent with it: an agent whose owner is gone would
    // report `done` into the unknown-entity log forever.
    assert!(
        !state
            .session_registry
            .contains(&derived_agent_key(&root, &run_id)),
        "releasing a run closes the agent it owned"
    );
    assert!(process_reaped(agent_pid), "the agent is killed AND reaped");
}

/// A branch view has two surfaces that can each mutate first — the agent
/// rail and the Changes review — so two adoptions of the same checkout can
/// arrive back to back. The second must land on the run the first minted.
#[test]
fn run_adopt_external_worktree_is_idempotent() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.project_at(0).id.clone();
    let _ext_path = add_external_worktree(&repo, dir.path(), "feature-x", "feature-x");
    let worktree_id = state
        .scan_external_worktrees_now(&project_id)
        .unwrap()
        .into_iter()
        .find(|w| w.branch.as_deref() == Some("feature-x"))
        .expect("the external worktree is discoverable")
        .id;

    let adopted = state.handle(req(
        "run.adopt",
        json!({ "project_id": project_id, "worktree_id": worktree_id }),
    ));
    assert_eq!(adopted["ok"], true, "{adopted:?}");
    let run_id = run_id_of(&adopted);

    let again = state.handle(req(
        "run.adopt",
        json!({ "project_id": project_id, "worktree_id": worktree_id }),
    ));
    assert_eq!(again["ok"], true, "{again:?}");
    assert_eq!(
        run_id_of(&again),
        run_id,
        "a second adoption answers with the run that already owns the checkout"
    );
    assert_eq!(
        state.runs.len(),
        1,
        "a second adoption must not mint a second owner"
    );
}

/// Adoption is a whole-repository scan, a checkpoint commit and a scaffold
/// — the git this whole split exists to keep off the app mutex.
#[test]
fn run_adopt_reads_git_with_the_state_lock_free() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let project_id = app.project_at(0).id.clone();
    add_external_worktree(&repo, dir.path(), "feature-slow", "feature-slow");
    let worktree_id = external_id(&mut app, &project_id, Some("feature-slow"));
    let (gate, gate_handle) = OffLockGate::new();
    app.off_lock_gate = Some(gate);
    let state = app.shared();

    let adopted = frame_on_a_thread(
        &state,
        "s-adopt",
        "run.adopt",
        json!({ "project_id": project_id, "worktree_id": worktree_id }),
    );
    gate_handle.wait_for_arrival();
    assert!(
        state.try_lock().is_ok(),
        "the adoption is holding the app mutex through its git"
    );
    let board = frame_on_a_thread(&state, "s-board", "board.list", json!({}))
        .recv_timeout(Duration::from_secs(10))
        .expect("the board answers while a checkout is being adopted");
    assert_eq!(board["ok"], true, "{board:?}");

    gate_handle.release();
    let adopted = adopted
        .recv_timeout(Duration::from_secs(30))
        .expect("the adoption answers once its git is done");
    assert_eq!(adopted["ok"], true, "{adopted:?}");
    assert_eq!(adopted["result"]["state"], "review", "{adopted:?}");
}

/// The Agent tab on an adopted checkout: `run.adopt` mints no agent, so
/// `agent.start` is what creates one. It runs in the checkout, keyed there
/// like every other checkout's agent, and a second start addresses it
/// rather than opening another.
#[tokio::test]
async fn agent_start_opens_an_adopted_checkouts_agent() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_qa_state_and_handler(&repo, dir.path());
    let project_id = state.lock().unwrap().project_at(0).id.clone();
    let root = add_external_worktree(&repo, dir.path(), "agent-here", "agent-here");
    let worktree_id = external_id(&mut state.lock().unwrap(), &project_id, Some("agent-here"));
    let adopted = call(
        &handler,
        "run.adopt",
        json!({ "project_id": project_id, "worktree_id": worktree_id }),
    );
    assert_eq!(adopted["ok"], true, "{adopted:?}");
    let run_id = run_id_of(&adopted);

    // Adoption minted no agent, so the start is what creates one — on the
    // account's default harness, since nobody named another.
    let started = call(&handler, "agent.start", json!({ "id": run_id }));
    assert_eq!(started["ok"], true, "{started:?}");
    assert_eq!(
        started["result"]["term_id"],
        agent_tab_id(started["result"]["agent_id"].as_str().unwrap()),
        "the reply reserves the tab id the agent's own identity mints: {started:?}"
    );
    wait_for_deliveries(&state).await;
    let root = AppState::canonical_root(&root);
    {
        let s = state.lock().unwrap();
        let roster = s.entity_agents(&run_id).expect("the adopted run");
        assert_eq!(roster.len(), 1, "the start created exactly one agent");
        assert_eq!(
            roster.primary().unwrap().choice.provider,
            AgentProvider::ClaudeAdk,
            "on the account's default harness"
        );
        assert!(
            s.session_registry
                .contains(&primary_agent_key(&s, &root, &run_id)),
            "the checkout's agent is keyed on the checkout root"
        );
    }
    let again = call(&handler, "agent.start", json!({ "id": run_id }));
    assert_eq!(again["ok"], true, "{again:?}");
    assert_eq!(
        again["result"]["term_id"], started["result"]["term_id"],
        "a second start addresses the agent the first one opened: {again:?}"
    );
}

/// The reply is built after the git, by the epilogue, so it carries what
/// only the checkout on disk could say — the branch the scan found it on.
#[test]
fn run_adopt_answers_from_its_epilogue_with_the_runs_own_view() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.project_at(0).id.clone();
    add_external_worktree(&repo, dir.path(), "feature-y", "feature-y");
    let worktree_id = state
        .scan_external_worktrees_now(&project_id)
        .unwrap()
        .into_iter()
        .find(|checkout| checkout.branch.as_deref() == Some("feature-y"))
        .expect("the external worktree is discoverable")
        .id;

    let adopted = state.handle(req(
        "run.adopt",
        json!({ "project_id": project_id, "worktree_id": worktree_id }),
    ));
    assert_eq!(adopted["ok"], true, "{adopted:?}");
    assert_eq!(adopted["result"]["branch"], "feature-y", "{adopted:?}");
    assert_eq!(adopted["result"]["adopted"], true, "{adopted:?}");
    let run_id = run_id_of(&adopted);
    let fetched = state.handle(req("run.get", json!({ "run_id": run_id })));
    assert_eq!(
        fetched["result"]["worktree_path"], adopted["result"]["worktree_path"],
        "the deferred reply is the run's own view: {fetched:?}"
    );
}

/// A card is adoptable from every reload and every second browser. While
/// one adoption's git runs the checkout has no run yet, so the second asker
/// is told the adoption is running and handed no id — not the id of a run
/// no verb would accept, and not a refusal, since it has nothing to
/// correct. Its next ask converges on the one owner the first minted.
#[test]
fn two_adopts_of_one_checkout_converge_on_one_run() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let project_id = app.project_at(0).id.clone();
    add_external_worktree(&repo, dir.path(), "feature-contended", "feature-contended");
    let worktree_id = external_id(&mut app, &project_id, Some("feature-contended"));
    let (gate, gate_handle) = OffLockGate::new();
    app.off_lock_gate = Some(gate);
    let state = app.shared();

    let first = frame_on_a_thread(
        &state,
        "s-one",
        "run.adopt",
        json!({ "project_id": project_id, "worktree_id": worktree_id }),
    );
    gate_handle.wait_for_arrival();
    let second = frame_on_a_thread(
        &state,
        "s-two",
        "run.adopt",
        json!({ "project_id": project_id, "worktree_id": worktree_id.clone() }),
    )
    .recv_timeout(Duration::from_secs(10))
    .expect("a second browser is answered while the first adoption runs");
    assert_eq!(second["ok"], true, "{second:?}");
    assert_eq!(
        second["result"]["adopting"], true,
        "the second asker is told the adoption is running: {second:?}"
    );
    assert!(
        second["result"]["run_id"].is_null(),
        "the second asker was handed a run that does not exist yet: {second:?}"
    );

    gate_handle.release();
    let first = first
        .recv_timeout(Duration::from_secs(30))
        .expect("the first adoption answers once its git is done");
    assert_eq!(first["ok"], true, "{first:?}");

    let retried = frame_on_a_thread(
        &state,
        "s-two",
        "run.adopt",
        json!({ "project_id": project_id, "worktree_id": worktree_id }),
    )
    .recv_timeout(Duration::from_secs(30))
    .expect("the retry is answered");
    assert_eq!(
        retried["result"]["run_id"], first["result"]["run_id"],
        "both askers name one run: {retried:?} {first:?}"
    );
    // And the id it names is one every other verb accepts: a reply that
    // promises a run must promise a durable one.
    let converged = run_id_of(&retried);
    let fetched = frame_on_a_thread(&state, "s-two", "run.get", json!({ "run_id": converged }))
        .recv_timeout(Duration::from_secs(10))
        .expect("run.get is answered");
    assert_eq!(
        fetched["ok"], true,
        "the id the second asker converged on resolves: {fetched:?}"
    );
    assert_eq!(
        state.lock().unwrap().runs.len(),
        1,
        "the checkout has one owner"
    );
}

/// Construction is the validation: a checkout that cannot be adopted is
/// refused before anything is written into it, so the work standing in it
/// is left exactly as it was found.
#[test]
fn run_adopt_refuses_a_detached_head_before_it_writes_a_checkpoint() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.project_at(0).id.clone();
    let path = add_external_worktree(&repo, dir.path(), "loose", "loose");
    assert!(Command::new("git")
        .args(["-C", path.to_str().unwrap(), "checkout", "--detach"])
        .status()
        .unwrap()
        .success());
    std::fs::write(path.join("scratch.txt"), "unsaved\n").unwrap();
    let worktree_id = state
        .scan_external_worktrees_now(&project_id)
        .unwrap()
        .into_iter()
        .find(|checkout| checkout.path == crate::worktree::canonical_root(&path))
        .expect("the detached checkout is discoverable")
        .id;

    let adopted = state.handle(req(
        "run.adopt",
        json!({ "project_id": project_id, "worktree_id": worktree_id }),
    ));
    assert_eq!(adopted["ok"], false, "{adopted:?}");
    let pending = Command::new("git")
        .args(["-C", path.to_str().unwrap(), "status", "--porcelain"])
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&pending.stdout).contains("scratch.txt"),
        "the refusal wrote no checkpoint commit"
    );
    assert!(state.runs.is_empty(), "no run was minted");
}

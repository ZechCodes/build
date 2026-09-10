use super::*;

// ==== branch.dispatch: one call from a sentence to an agent working =======

/// The instruction the router hands a branch, and the message the agent it
/// dispatched is holding when it wakes up.
fn dispatched_instruction(state: &AppState, run_id: &str, agent_id: &str) -> String {
    let agent = state.runs[run_id]
        .agents
        .by_id(agent_id)
        .unwrap_or_else(|| panic!("{agent_id} is on {run_id}'s roster"));
    match agent
        .thread
        .items
        .first()
        .unwrap_or_else(|| panic!("{agent_id} was dispatched with a first message"))
    {
        crate::thread::ThreadItem::Message(message) => {
            assert_eq!(message.role, crate::thread::MessageRole::User);
            message.body.clone()
        }
        other => panic!("the first item is the instruction, not {other:?}"),
    }
}

/// Nothing but the instruction: no branch named, no checkout to reuse. One
/// call cuts the branch, takes ownership of it, puts an agent on it, hands
/// that agent the words, and tells the harness there is something to read.
#[test]
#[allow(clippy::cognitive_complexity)] // ratchet: branch_dispatch_cuts_a_branch_and_puts_an_agent_to_work_on_it is at 19, threshold 15 — bring it under, then remove
fn branch_dispatch_cuts_a_branch_and_puts_an_agent_to_work_on_it() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.project_at(0).id.clone();

    let dispatched = state.handle(req(
        "branch.dispatch",
        json!({
            "project_id": project_id,
            "instruction": "Add a health endpoint",
        }),
    ));

    assert_eq!(dispatched["ok"], true, "{dispatched:?}");
    let result = &dispatched["result"];
    assert_eq!(result["project_id"], json!(project_id));
    let branch = result["branch"].as_str().unwrap().to_string();
    let run_id = result["run_id"].as_str().unwrap().to_string();
    let agent_id = result["agent_id"].as_str().unwrap().to_string();
    assert_eq!(branch, "build/add-a-health-endpoint", "{result:?}");
    assert!(agent_id.starts_with("agent-"), "{result:?}");

    let active = state.runs.get(&run_id).expect("the branch has a run");
    assert!(active.worktree.path.is_dir(), "{:?}", active.worktree.path);
    assert_eq!(active.worktree.branch(), branch);
    assert!(active.adopted, "the checkout it cut is one Build owns");
    assert_eq!(
        active.agents.len(),
        1,
        "a dispatched branch opens with exactly one agent, and no empty bubble beside it"
    );
    assert_eq!(active.agents.primary().unwrap().id, agent_id);
    assert_eq!(
        dispatched_instruction(&state, &run_id, &agent_id),
        "Add a health endpoint"
    );

    // The harness is told, through the same queue every other verb speaks
    // to an agent with: cold spawns it with the instruction in its catch-up
    // packet, warm is the read-your-messages nudge `thread.post` writes.
    assert_eq!(state.delivery_queue.queued_len(), 1, "one turn was queued");
    let queued = &state.delivery_queue.queued_nth(0).unwrap();
    assert_eq!(queued.owner, run_id);
    assert_eq!(queued.agent_id, agent_id);
    assert_eq!(queued.root, AppState::canonical_root(&active.worktree.path));
    let delivered =
        state.cold_prompt_with_catch_up(&queued.owner, &queued.agent_id, &queued.said().cold);
    assert!(
        delivered.contains("Add a health endpoint"),
        "a cold agent reads the instruction out of the packet composed at delivery: {delivered}"
    );
    assert_eq!(queued.said().warm, NEW_THREAD_MESSAGES_PROMPT);

    // …and the work is on the feed as one branch row.
    let board = state.handle(req("board.list", json!({})));
    let row = board["result"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["run_id"] == json!(run_id.clone()))
        .unwrap_or_else(|| panic!("the dispatched branch is on the feed: {board:?}"));
    assert_eq!(row["kind"], "branch", "{row:?}");
    assert_eq!(row["branch"], json!(branch), "{row:?}");
}

/// A branch the caller named is a name, not a description of one. It is cut
/// exactly as given — a `build/` prefix is the caller's, not something to
/// add a second time — and only words with no branch name in them get
/// slugified into Build's namespace.
#[test]
fn branch_dispatch_cuts_a_named_branch_exactly_as_it_was_given() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.project_at(0).id.clone();

    let dispatch = |state: &mut AppState, branch: &str| {
        state.handle(req(
            "branch.dispatch",
            json!({
                "project_id": project_id,
                "branch": branch,
                "instruction": "Add a CSV export",
            }),
        ))
    };

    // Prefixed: the namespace is already there, so nothing adds it again.
    let prefixed = dispatch(&mut state, "build/csv-export");
    assert_eq!(prefixed["ok"], true, "{prefixed:?}");
    assert_eq!(
        prefixed["result"]["branch"], "build/csv-export",
        "{prefixed:?}"
    );

    // Plain: a name with no namespace is left with none.
    let plain = dispatch(&mut state, "hotfix-login");
    assert_eq!(plain["ok"], true, "{plain:?}");
    assert_eq!(plain["result"]["branch"], "hotfix-login", "{plain:?}");

    // Not a name at all: the words name the branch, in Build's namespace.
    let described = dispatch(&mut state, "Add a CSV export, please");
    assert_eq!(described["ok"], true, "{described:?}");
    assert_eq!(
        described["result"]["branch"], "build/add-a-csv-export-please",
        "{described:?}"
    );

    for branch in [
        "build/csv-export",
        "hotfix-login",
        "build/add-a-csv-export-please",
    ] {
        assert!(
            local_branch_exists(&repo, branch).unwrap(),
            "{branch} was cut"
        );
    }
    assert!(
        !local_branch_exists(&repo, "build/build-csv-export").unwrap(),
        "a named branch is never slugified into a second namespace"
    );
}

/// A dispatch that names a branch already in flight joins that checkout —
/// and still gets its own agent, because an instruction is never dropped
/// into a conversation someone else is having.
#[test]
fn branch_dispatch_onto_an_existing_branch_reuses_it_and_adds_an_agent() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.project_at(0).id.clone();
    let run_id = adopted_run(&mut state, &repo, dir.path(), "feature-in-flight");
    let primary_agent = primary_agent_id(&state, &run_id);
    let worktree = state.runs[&run_id].worktree.path.clone();

    let dispatched = state.handle(req(
        "branch.dispatch",
        json!({
            "project_id": project_id,
            "branch": "feature-in-flight",
            "instruction": "Also cover the empty case",
            "provider": "codex",
        }),
    ));

    assert_eq!(dispatched["ok"], true, "{dispatched:?}");
    let result = &dispatched["result"];
    assert_eq!(result["run_id"], json!(run_id.clone()), "{result:?}");
    assert_eq!(result["branch"], "feature-in-flight", "{result:?}");
    let agent_id = result["agent_id"].as_str().unwrap().to_string();
    assert_ne!(
        agent_id, primary_agent,
        "a dispatch never joins a conversation"
    );

    let active = state.runs.get(&run_id).expect("the run is still here");
    assert_eq!(active.worktree.path, worktree, "the worktree was reused");
    assert_eq!(active.agents.len(), 2);
    assert_eq!(active.agents.by_id(&agent_id).unwrap().ordinal, 2);
    assert_eq!(
        active.agents.by_id(&agent_id).unwrap().choice.provider,
        AgentProvider::Codex,
        "the dispatch chose what its agent runs on"
    );
    assert!(
        primary_thread(&active.agents).items.is_empty(),
        "nothing landed in the agent that was already there"
    );
    assert_eq!(
        dispatched_instruction(&state, &run_id, &agent_id),
        "Also cover the empty case"
    );
    assert_eq!(state.delivery_queue.queued_len(), 1);
    let queued = &state.delivery_queue.queued_nth(0).unwrap();
    assert_eq!(queued.agent_id, agent_id);
    assert_eq!(
        queued.model_choice.provider,
        AgentProvider::Codex,
        "the harness it spawns is the one this agent was dispatched on"
    );
    let delivered =
        state.cold_prompt_with_catch_up(&queued.owner, &queued.agent_id, &queued.said().cold);
    assert!(
        delivered.contains("Also cover the empty case"),
        "{delivered}"
    );
}

/// A branch it cut and could not finish setting up leaves nothing behind:
/// no directory, no branch ref, no run, no agent, nothing queued.
#[test]
fn branch_dispatch_releases_the_branch_it_minted_when_a_step_fails() {
    let (dir, repo) = init_repo();
    for step in [BranchDispatchStep::Adopt, BranchDispatchStep::Own] {
        let mut state = qa_state(&repo, dir.path());
        let project_id = state.project_at(0).id.clone();
        // A board that has been looked at once: the checkout the dispatch
        // cuts joins that list, and the rollback has to take it back out.
        state.scan_external_worktrees_now(&project_id).unwrap();
        state.dispatch_fault = Some(step);

        let failed = state.handle(req(
            "branch.dispatch",
            json!({ "project_id": project_id, "instruction": "Add a health endpoint" }),
        ));

        assert_eq!(failed["ok"], false, "{step:?} -> {failed:?}");
        assert!(state.runs.is_empty(), "{step:?} left a run behind");
        assert!(
            state.pending_rows.is_empty(),
            "{step:?} left its reservation on the board"
        );
        assert!(
            state.delivery_queue.queued_is_empty(),
            "{step:?} left a turn queued"
        );
        assert!(
            !dir.path().join("wt").join("add-a-health-endpoint").exists(),
            "{step:?} left the worktree it minted on disk"
        );
        assert!(
            state
                .external_scan_of(&project_id)
                .is_some_and(|cache| cache.worktrees.is_empty()),
            "{step:?} left a checkout on the board that is not on disk: {:?}",
            state.external_scan_of(&project_id).map(|c| &c.worktrees)
        );
        state.dispatch_fault = None;
        assert!(
            state
                .scan_external_worktrees_now(&project_id)
                .unwrap()
                .is_empty(),
            "{step:?} left a checkout the scan can still see"
        );
        assert!(
            !local_branch_exists(&repo, "build/add-a-health-endpoint").unwrap(),
            "{step:?} left the branch ref it cut"
        );
    }
}

/// The other half of the failure: the git ran, and the writes it was
/// supposed to lead to did not. What it made is real, so the checkout stays
/// on the board as the unowned card it is — and nothing pretends a run
/// exists.
#[test]
fn a_dispatch_that_fails_after_its_git_leaves_the_checkout_on_the_board() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.project_at(0).id.clone();
    // A board that has been looked at once, so the amendment has a list to
    // put the checkout back into.
    state.scan_external_worktrees_now(&project_id).unwrap();
    state.dispatch_fault = Some(BranchDispatchStep::Open);

    let failed = state.handle(req(
        "branch.dispatch",
        json!({ "project_id": project_id, "instruction": "Add a health endpoint" }),
    ));

    assert_eq!(failed["ok"], false, "{failed:?}");
    assert!(state.runs.is_empty(), "the failed apply opened a run");
    assert!(
        state.pending_rows.is_empty(),
        "the failed apply left its row on the board"
    );
    assert!(
        state.delivery_queue.queued_is_empty(),
        "the failed apply queued a turn for an agent that does not exist"
    );
    let checkout = crate::worktree::canonical_root(
        &dir.path()
            .join("wt")
            .join(&project_id)
            .join("add-a-health-endpoint"),
    );
    assert!(checkout.is_dir(), "the git that succeeded was undone");
    assert!(
        state
            .external_scan_of(&project_id)
            .is_some_and(|cache| cache.worktrees.iter().any(|w| w.path == checkout)),
        "the checkout it cut is invisible until the next full rescan: {:?}",
        state.external_scan_of(&project_id).map(|c| &c.worktrees)
    );
    // And it is adoptable from that card: nothing about it is half-owned.
    state.dispatch_fault = None;
    let worktree_id = crate::worktree::external_worktree_id(&checkout);
    let adopted = state.handle(req(
        "run.adopt",
        json!({ "project_id": project_id, "worktree_id": worktree_id }),
    ));
    assert_eq!(adopted["ok"], true, "{adopted:?}");
}

/// A turn is not deliverable until the mutation that queued it is durable.
/// A dispatch that fails after handing its agent the instruction hands that
/// instruction to nobody — on the arm that opens a run, on the arm that
/// joins one, and through the drain the MCP control socket runs.
#[test]
fn a_dispatch_that_fails_after_queuing_its_turn_delivers_nothing() {
    fn queued_owners(state: &AppState) -> Vec<String> {
        state
            .delivery_queue
            .queued()
            .map(|turn| turn.owner.clone())
            .collect()
    }
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.project_at(0).id.clone();
    let (capture_id, _) = captured(&mut state, "finish the toast");
    state.delivery_queue.clear_queued();
    state.dispatch_fault = Some(BranchDispatchStep::Settle);

    let opened = state
        .router_action(
            &capture_id,
            BridgeAction::DispatchBranch {
                project_id: project_id.clone(),
                branch: None,
                instruction: "finish the toast".to_string(),
                rationale: None,
            },
        )
        .unwrap_err();
    assert!(opened.contains("Settle"), "{opened}");
    assert!(
        state.delivery_queue.queued_is_empty(),
        "the router's dispatch left a turn queued for a run the store never got: {:?}",
        queued_owners(&state)
    );
    assert!(
        state.pending_rows.is_empty(),
        "the failed apply left its row on the board"
    );

    // The other arm: a branch Build already runs, which touches no git at
    // all and so answers without a deferral.
    state.dispatch_fault = None;
    adopted_run(&mut state, &repo, dir.path(), "feature-running");
    state.delivery_queue.clear_queued();
    state.dispatch_fault = Some(BranchDispatchStep::Settle);

    let joined = state.handle(req(
        "branch.dispatch",
        json!({
            "project_id": project_id,
            "branch": "feature-running",
            "instruction": "one more thing",
        }),
    ));

    assert_eq!(joined["ok"], false, "{joined:?}");
    assert!(
        state.delivery_queue.queued_is_empty(),
        "joining a run left a turn queued for a dispatch that failed: {:?}",
        queued_owners(&state)
    );

    // And the same arm reached over the socket, which answers a router tool
    // without a deferral to apply and so has to drop the turn itself.
    state.dispatch_fault = None;
    adopted_run(&mut state, &repo, dir.path(), "feature-elsewhere");
    let (second_capture, _) = captured(&mut state, "and this too");
    state.delivery_queue.clear_queued();
    state.dispatch_fault = Some(BranchDispatchStep::Settle);

    let routed = state
        .router_action(
            &second_capture,
            BridgeAction::DispatchBranch {
                project_id,
                branch: Some("feature-elsewhere".to_string()),
                instruction: "one more thing".to_string(),
                rationale: None,
            },
        )
        .unwrap_err();
    assert!(routed.contains("Settle"), "{routed}");
    assert!(
        state.delivery_queue.queued_is_empty(),
        "the router's join left a turn queued for a dispatch that failed: {:?}",
        queued_owners(&state)
    );
}

/// A named branch that existed before the call is checked out, not cut. So
/// when the dispatch then fails, cleanup takes the directory it added and
/// leaves the branch: those commits are somebody's work, and nobody asked
/// Build to delete them.
#[test]
fn branch_dispatch_cleanup_keeps_a_branch_it_only_checked_out() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.project_at(0).id.clone();
    {
        let opened = git2::Repository::open(&repo).unwrap();
        let head = opened.head().unwrap().peel_to_commit().unwrap();
        opened
            .branch("build/started-by-hand", &head, false)
            .unwrap();
    }
    state.dispatch_fault = Some(BranchDispatchStep::Adopt);

    let failed = state.handle(req(
        "branch.dispatch",
        json!({
            "project_id": project_id,
            "branch": "build/started-by-hand",
            "instruction": "pick this up",
        }),
    ));

    assert_eq!(failed["ok"], false, "{failed:?}");
    assert!(
        local_branch_exists(&repo, "build/started-by-hand").unwrap(),
        "a branch the dispatch only checked out is not its to delete"
    );
    assert!(
        !dir.path().join("wt").join("started-by-hand").exists(),
        "the checkout it added is gone"
    );
    assert!(state.runs.is_empty());
}

/// A checkout that was there before the call is handed back exactly as it
/// was found — files, branch and all. Cleanup undoes what the dispatch
/// created; it never destroys what it merely adopted.
#[test]
fn branch_dispatch_cleanup_never_destroys_a_checkout_it_only_found() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.project_at(0).id.clone();

    // A bare checkout Build does not own yet: the dispatch adopts it, so
    // cleanup un-adopts and leaves every file alone.
    let by_hand = add_external_worktree(&repo, dir.path(), "by-hand", "feature-by-hand");
    std::fs::write(by_hand.join("mine.txt"), "not Build's to delete\n").unwrap();
    state.dispatch_fault = Some(BranchDispatchStep::Own);

    let failed = state.handle(req(
        "branch.dispatch",
        json!({
            "project_id": project_id,
            "branch": "feature-by-hand",
            "instruction": "pick this up",
        }),
    ));

    assert_eq!(failed["ok"], false, "{failed:?}");
    assert!(by_hand.is_dir(), "the checkout was destroyed");
    assert!(by_hand.join("mine.txt").is_file(), "its work was destroyed");
    assert!(state.runs.is_empty(), "the adoption was undone");
    assert!(state.delivery_queue.queued_is_empty());

    // A branch Build already runs: cleanup leaves the run and its checkout
    // standing, with the agent roster it had before the call.
    state.dispatch_fault = Some(BranchDispatchStep::Post);
    let run_id = adopted_run(&mut state, &repo, dir.path(), "feature-running");
    let worktree = state.runs[&run_id].worktree.path.clone();
    let agents_before = state.runs[&run_id].agents.len();

    let failed = state.handle(req(
        "branch.dispatch",
        json!({
            "project_id": project_id,
            "branch": "feature-running",
            "instruction": "one more thing",
        }),
    ));

    assert_eq!(failed["ok"], false, "{failed:?}");
    assert!(worktree.is_dir(), "the running checkout was destroyed");
    assert!(state.runs.contains_key(&run_id), "the run was released");
    assert_eq!(state.runs[&run_id].agents.len(), agents_before);
    assert!(state.delivery_queue.queued_is_empty());
}

/// A branch the repository's own checkout is on is one git refuses to
/// check out a second time, and its refusal names nothing the caller can
/// act on. The dispatch answers with the checkout that holds the branch,
/// as `worktree.create {branch}` does — one resolution for both.
#[test]
fn branch_dispatch_names_the_primary_checkout_holding_its_branch() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.project_at(0).id.clone();

    let refused = state.handle(req(
        "branch.dispatch",
        json!({
            "project_id": project_id,
            "branch": "main",
            "instruction": "work on main itself",
        }),
    ));

    assert_eq!(refused["ok"], false, "{refused:?}");
    let message = refused["error"].as_str().unwrap();
    assert!(message.contains("primary checkout"), "{message}");
    assert!(state.runs.is_empty(), "nothing was created");
    assert!(state.external_worktrees(&project_id).worktrees.is_empty());
}

/// Refusals come before anything is created: an unknown project and an
/// empty instruction both leave the repo untouched.
#[test]
fn branch_dispatch_refuses_an_unknown_project_and_an_empty_instruction() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.project_at(0).id.clone();

    let unknown = state.handle(req(
        "branch.dispatch",
        json!({ "project_id": "proj-nope", "instruction": "do the thing" }),
    ));
    assert_eq!(unknown["ok"], false, "{unknown:?}");
    assert!(
        unknown["error"].as_str().unwrap().contains("proj-nope"),
        "{unknown:?}"
    );

    for instruction in [json!(""), json!("   ")] {
        let empty = state.handle(req(
            "branch.dispatch",
            json!({ "project_id": project_id, "instruction": instruction }),
        ));
        assert_eq!(empty["ok"], false, "{instruction:?} -> {empty:?}");
    }

    let unrunnable = state.handle(req(
        "branch.dispatch",
        json!({
            "project_id": project_id,
            "instruction": "do the thing",
            "provider": "hal9000",
        }),
    ));
    assert_eq!(unrunnable["ok"], false, "{unrunnable:?}");

    assert!(state.runs.is_empty(), "nothing was created");
    assert!(state
        .scan_external_worktrees_now(&project_id)
        .unwrap()
        .is_empty());
}

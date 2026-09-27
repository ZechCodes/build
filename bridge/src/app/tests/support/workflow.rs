use super::super::*;

/// A QA state with a durable store — plans keep their canonical docs there
/// and `run.create` reads/writes through it, so every lifecycle test needs
/// one.
pub(in crate::app::tests) fn qa_state(repo: &std::path::Path, dir: &std::path::Path) -> AppState {
    let context = HarnessContext::resolved(dir.join("test-mcp.sock"), dir.to_path_buf())
        .expect("resolve QA harness context");
    AppState::new_configured(repo.to_path_buf(), dir.join("wt"), "main", true, context)
        .with_task_store(dir.join("store"))
        .unwrap()
}

/// A QA daemon behind the shared `Arc` plus its frame handler — the entry
/// point the relay uses, and the only one that delivers the agent turns a
/// verb queues while it holds the state lock.
pub(in crate::app::tests) fn shared_qa_state_and_handler(
    repo: &std::path::Path,
    dir: &std::path::Path,
) -> (Arc<Mutex<AppState>>, FrameHandler) {
    let state = qa_state_timed_by(FrameClock::new(), repo, dir);
    let handler = AppState::handler(Arc::clone(&state));
    (state, handler)
}

/// The same daemon, timed by the clock the test means to read back — the
/// one entry point the MCP control socket has, since it answers with no
/// frame handler behind it.
pub(in crate::app::tests) fn qa_state_timed_by(
    clock: Arc<FrameClock>,
    repo: &std::path::Path,
    dir: &std::path::Path,
) -> Arc<Mutex<AppState>> {
    let mut app = qa_state(repo, dir);
    app.term_shell = "/bin/bash".into();
    app.frame_clock = clock;
    app.shared()
}

/// One RPC over the frame handler.
pub(in crate::app::tests) fn call(handler: &FrameHandler, method: &str, params: Value) -> Value {
    handler.call(SessionSender::detached("qa"), req(method, params))
}

/// The tab key of the agent whose id is DERIVED from its owner — the one
/// the pre-agent migration mints, and the one a task holds. A branch
/// whose agent was added rather than migrated has a minted id instead, and
/// its key is `primary_agent_key`.
pub(in crate::app::tests) fn derived_agent_key(root: &std::path::Path, entity_id: &str) -> TabKey {
    TabKey::agent(root, &crate::agent::derived_agent_id(entity_id))
}

/// The terminal a tab's session offers. Tests are the only place that
/// reaches for one without a client asking: the daemon goes through
/// [`Tab::require_terminal`], which says why when there is none.
pub(in crate::app::tests) fn agent_terminal(tab: &Tab) -> &dyn crate::harness::TerminalView {
    tab.session
        .terminal()
        .expect("a PTY session offers a terminal")
}

/// The OS process behind a tab, asked through the terminal that owns it —
/// a process id is the basement's, and no other kind of session has one to give.
pub(in crate::app::tests) fn agent_pid(tab: &Tab) -> Option<u32> {
    tab.session
        .terminal()
        .and_then(crate::harness::TerminalView::pid)
}

/// [`planned_run_in_review`] plus one supported post over the frame handler.
/// A conversation only gains its session lineage when a turn is delivered
/// COLD (a new agent process), so a test about what the thread carries has to
/// go this way even though the legacy plan/run records are installed directly.
pub(in crate::app::tests) fn planned_run_in_review_delivered(
    state: &Arc<Mutex<AppState>>,
    handler: &FrameHandler,
    goal: &str,
) -> (String, String) {
    let (plan_id, run_id) = {
        let mut state = state.lock().unwrap();
        planned_run_in_review(&mut state, goal)
    };
    // A supported conversation post gives the fixture the live delivery
    // lineage that its handler-only callers are specifically testing.
    let delivered = call(
        handler,
        "thread.post",
        json!({ "entity_id": run_id, "body": "review this implementation" }),
    );
    assert_eq!(delivered["ok"], true, "{delivered:?}");
    (plan_id, run_id)
}

/// The one row a list surface carries for `id`. Every list is a
/// `HashMap`'s values in whatever order this process's hash seed put them,
/// so a test that means one entity has to name it.
pub(in crate::app::tests) fn row_with<'a>(rows: &'a Value, key: &str, id: &str) -> &'a Value {
    rows.as_array()
        .unwrap_or_else(|| panic!("not a list: {rows:?}"))
        .iter()
        .find(|row| row[key] == json!(id))
        .unwrap_or_else(|| panic!("no {key} {id}: {rows:?}"))
}

pub(in crate::app::tests) fn plan_id_of(res: &Value) -> String {
    res.get("result").unwrap_or(res)["plan_id"]
        .as_str()
        .unwrap_or_else(|| panic!("no plan_id: {res:?}"))
        .to_string()
}

pub(in crate::app::tests) fn run_id_of(res: &Value) -> String {
    res.get("result").unwrap_or(res)["run_id"]
        .as_str()
        .unwrap_or_else(|| panic!("no run_id: {res:?}"))
        .to_string()
}

/// A run in review, minted the only way runs are minted now: author a plan
/// with the scripted agent, approve it and both of its stage docs, implement
/// stage one, then dispatch stage two (whose completion opens review).
/// Returns `(plan_id, run_id)`. The goal-only "Quick task" dispatch this
/// replaces is gone — see `run_create_refuses_a_goal_without_a_plan`.
pub(in crate::app::tests) fn planned_run_in_review(
    state: &mut AppState,
    goal: &str,
) -> (String, String) {
    // Task workflow RPCs are intentionally retired, but many non-workflow
    // tests still need the durable shape an old Task and its implementation
    // left behind. Build that shape through the domain seams so those tests do
    // not accidentally keep the retired public surface alive.
    let plan = state
        .plan_create(&json!({ "goal": goal, "dispatch": false }))
        .expect("the legacy plan fixture is filed");
    let plan_id = plan["plan_id"]
        .as_str()
        .expect("the legacy plan fixture has an id")
        .to_string();
    let project_id = state.project_of(&plan_id).expect("the plan has a project");
    let orch = state
        .orch_for(&project_id)
        .expect("the project has an orchestrator")
        .clone();
    let store = state
        .require_store()
        .expect("the fixture has a store")
        .clone();
    let workspace = orch
        .prepare_plan_workspace(&plan_id, &store)
        .expect("the legacy plan workspace is prepared");
    let mut active = state.plans.remove(&plan_id).expect("the plan was filed");
    orch.open_plan_drafting(&mut active, workspace)
        .expect("the legacy plan starts drafting");
    state
        .qa_simulate_plan(&project_id, &mut active)
        .expect("the scripted planner authors the legacy plan");
    state
        .finish_plan_mutation(plan_id.clone(), active)
        .expect("the planned fixture is durable");
    for stage_id in ["first-half", "second-half"] {
        state
            .plan_stage_approve(&json!({ "plan_id": plan_id, "stage_id": stage_id }))
            .expect("the legacy stage is approved");
    }
    state
        .plan_approve(&json!({ "plan_id": plan_id }))
        .expect("the legacy plan is approved");

    let plan = state.plans.get(&plan_id).expect("the plan remains live");
    let task = ImplementableTask::judge(RunSource {
        plan,
        has_active_run: false,
    })
    .expect("the approved plan is implementable");
    let run_id = format!("run-{}", uuid::Uuid::new_v4());
    let prepared = orch
        .prepare_run_checkout(&task, &plan.base_branch, &run_id, state.isolation, &store)
        .expect("the legacy implementation checkout is prepared");
    let (mut run, _turn) = orch
        .open_prepared_run(RunId::new(&run_id), plan, prepared, Default::default())
        .expect("the legacy implementation opens");
    let plan_docs = plan.stages.clone();
    state
        .qa_simulate_stage_build(&project_id, &mut run, &plan_docs)
        .expect("the first legacy stage is built");
    state
        .projects
        .bind_entity(run_id.clone(), project_id.clone());
    state
        .finish_run_mutation(run_id.clone(), run)
        .expect("the legacy run is durable");
    state
        .run_stage_dispatch(&json!({ "run_id": run_id, "stage_id": "second-half" }))
        .expect("the second legacy stage dispatches");
    let mut run = state.runs.remove(&run_id).expect("the run remains live");
    state
        .qa_simulate_stage_build(&project_id, &mut run, &plan_docs)
        .expect("the second legacy stage is built");
    assert_eq!(run.run.state, RunState::Review);
    state
        .finish_run_mutation(run_id.clone(), run)
        .expect("the reviewed legacy run is durable");
    (plan_id, run_id)
}

/// A plan-less run: adopt an external worktree. Adoption is the only
/// remaining source of runs that implement no plan, so it stands in wherever
/// a test just needs a live run with no plan behind it.
pub(in crate::app::tests) fn adopted_run(
    state: &mut AppState,
    repo: &std::path::Path,
    dir: &std::path::Path,
    branch: &str,
) -> String {
    add_external_worktree(repo, dir, branch, branch);
    let project_id = state.project_at(0).id.clone();
    let worktree_id = state
        .scan_external_worktrees_now(&project_id)
        .unwrap()
        .into_iter()
        .find(|w| w.branch.as_deref() == Some(branch))
        .expect("the external worktree is discoverable")
        .id;
    let adopted = state.handle(req(
        "run.adopt",
        json!({ "project_id": project_id, "worktree_id": worktree_id }),
    ));
    assert_eq!(adopted["ok"], true, "{adopted:?}");
    let run_id = run_id_of(&adopted);
    // Adoption is git and records — it speaks to nobody, so it mints no
    // agent. A test about a branch someone is working gives it the one the
    // human's first message would have created.
    let added = state.handle(req("agent.add", json!({ "entity_id": run_id.clone() })));
    assert_eq!(added["ok"], true, "{added:?}");
    run_id
}

/// The id of an entity's primary agent — the one a verb that names none
/// reaches. Whichever kind of entity the id names, and whether its agent
/// was minted or derived from the owner.
pub(in crate::app::tests) fn primary_agent_id(state: &AppState, entity_id: &str) -> String {
    state
        .entity_agents(entity_id)
        .expect("the entity is on the board")
        .primary()
        .expect("and holds an agent")
        .id
        .clone()
}

/// That agent's tab key.
pub(in crate::app::tests) fn primary_agent_key(
    state: &AppState,
    root: &std::path::Path,
    entity_id: &str,
) -> TabKey {
    TabKey::agent(root, &primary_agent_id(state, entity_id))
}

/// The harness an agent's session was actually opened on. The tab records
/// what it spawned, so this is what a start really spent — and unlike the
/// attach's answer it holds for a session with no terminal.
pub(in crate::app::tests) fn spawned_provider(
    state: &Arc<Mutex<AppState>>,
    root: &std::path::Path,
    agent_id: &str,
) -> AgentProvider {
    match state
        .lock()
        .unwrap()
        .session_registry
        .test_tab(&TabKey::agent(root, agent_id))
        .unwrap()
        .role
    {
        TabRole::Agent { provider, .. } => provider,
        TabRole::Shell => panic!("{agent_id} opened a shell, not an agent session"),
    }
}

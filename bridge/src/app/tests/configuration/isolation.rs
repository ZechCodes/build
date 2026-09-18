use super::*;

/// Which isolation new checkouts get is an account setting with a
/// per-project override, and both outlive the process that chose them. A
/// project that inherits writes nothing: the absent key is what inheriting
/// looks like on disk.
#[test]
fn the_account_isolation_and_a_project_override_survive_a_reload() {
    let tmp = tempfile::tempdir().unwrap();
    let (_dir, repo) = init_repo();
    let inheriting = crate::git_fixture::init_repo_named(tmp.path(), "inheriting");
    let cfg = tmp.path().join("config.json");
    let load = |cfg: &std::path::Path| {
        AppState::new(
            repo.clone(),
            tmp.path().join("wt"),
            "main",
            true,
            "/tmp/test-mcp.sock",
        )
        .with_config(cfg)
        .unwrap()
    };
    {
        let mut state = load(&cfg);
        state.add_project(inheriting.clone(), "main".to_string());
        state.isolation = Isolation::Rift;
        state.project_at_mut(0).isolation = Some(Isolation::Worktree);
        state.persist();
    }

    let written: Value = serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    assert_eq!(written["isolation"], "rift", "{written:?}");
    assert_eq!(
        written["projects"][0]["isolation"], "worktree",
        "{written:?}"
    );
    assert!(
        written["projects"][1].get("isolation").is_none(),
        "a project that inherits the account setting persists no override: {written:?}"
    );

    let reloaded = load(&cfg);
    assert_eq!(reloaded.isolation, Isolation::Rift);
    assert_eq!(reloaded.project_at(0).isolation, Some(Isolation::Worktree));
    assert_eq!(
        reloaded.project_at(1).isolation,
        None,
        "an absent key loads as inheriting, not as a choice"
    );
}

/// A config naming an isolation this bridge has never heard of is a config
/// from a newer bridge, not a reason to fail boot: the word is logged and
/// read as absent, exactly as an unknown `default_harness` is.
#[test]
fn an_unknown_persisted_isolation_loads_as_the_default() {
    let tmp = tempfile::tempdir().unwrap();
    let (_dir, repo) = init_repo();
    let cfg = tmp.path().join("config.json");
    std::fs::write(
        &cfg,
        json!({
            "isolation": "telepathy",
            "projects": [ {
                "path": repo.display().to_string(),
                "base_branch": "main",
                "isolation": "telekinesis",
            } ],
        })
        .to_string(),
    )
    .unwrap();

    let state = AppState::new(
        repo,
        tmp.path().join("wt"),
        "main",
        true,
        "/tmp/test-mcp.sock",
    )
    .with_config(&cfg)
    .unwrap();
    assert_eq!(state.isolation, Isolation::Worktree);
    assert_eq!(state.project_at(0).isolation, None);
}

/// The project's own answer is the one asked first: an override of the
/// linked worktree beats an account default of cloning, and no volume has
/// to be consulted to honour it.
#[test]
fn a_project_override_beats_the_account_isolation() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    state.isolation = Isolation::Rift;
    state.project_at_mut(0).isolation = Some(Isolation::Worktree);

    let project_id = state.project_at(0).id.clone();
    let resolved = state.resolved_isolation(&project_id);
    assert_eq!(resolved.isolation, Isolation::Worktree);
    assert_eq!(resolved.downgrade, None);
}

/// An override the other way is kept the same way: the account asks for a
/// linked worktree, this project asks to be cloned, and a volume that can
/// clone answers with the project's choice and nothing to announce.
#[test]
fn a_project_asking_to_be_cloned_is_cloned_where_the_volume_can() {
    let (dir, repo) = init_repo();
    if !crate::isolation::probe::rift_or_skip(dir.path()) {
        return;
    }
    let mut state = qa_state(&repo, dir.path());
    state.project_at_mut(0).isolation = Some(Isolation::Rift);

    let project_id = state.project_at(0).id.clone();
    let resolved = state.resolved_isolation(&project_id);
    assert_eq!(resolved.isolation, Isolation::Rift);
    assert_eq!(resolved.downgrade, None);
}

/// A volume that cannot clone does not fail the create: the request is
/// downgraded to the isolation every volume can make, and the resolver
/// hands back the sentence that says why, for whoever announces it.
#[test]
fn a_clone_no_volume_can_make_is_downgraded_with_its_reason() {
    let (dir, repo) = init_repo();
    let linked = dir.path().join("linked");
    crate::git_fixture::git_in(
        &repo,
        &["worktree", "add", "-b", "feature", linked.to_str().unwrap()],
    );
    let mut state = qa_state(&repo, dir.path());
    state.isolation = Isolation::Rift;
    let project_id = state.add_project(linked, "feature".to_string());

    let resolved = state.resolved_isolation(&project_id);
    assert_eq!(resolved.isolation, Isolation::Worktree);
    assert!(
        resolved
            .downgrade
            .unwrap_or_default()
            .contains("linked worktree"),
        "the downgrade carries the probe's own sentence"
    );
}

/// A checkout that is itself a linked worktree can never be cloned, on any
/// filesystem: the one project shape that makes this machine's answer
/// deterministic wherever the suite runs.
fn state_on_an_unclonable_project(dir: &std::path::Path, repo: &std::path::Path) -> AppState {
    let linked = dir.join("linked");
    crate::git_fixture::git_in(
        repo,
        &["worktree", "add", "-b", "feature", linked.to_str().unwrap()],
    );
    qa_state(&linked, dir)
}

/// The account settings carry the isolation new checkouts get and what this
/// machine can actually make, so one read tells a control both what is
/// chosen and whether the other choice is even offerable.
#[test]
fn settings_get_reports_the_account_isolation_and_what_this_volume_can_make() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());

    let settings = state.handle(req("settings.get", json!({})));
    assert_eq!(settings["ok"], true, "{settings:?}");
    let result = &settings["result"];
    assert_eq!(result["isolation"], "worktree", "{result:?}");
    let available = &result["isolation_available"];
    assert!(available["rift"].is_boolean(), "{result:?}");
    assert_eq!(
        available["reason"].is_null(),
        available["rift"] == true,
        "a locked clone carries its sentence and an available one carries none: {result:?}"
    );
}

/// Whether a volume can clone is a project's question, so a bridge with no
/// project has no volume to ask — and says exactly that rather than
/// reporting a machine limit it never tested.
#[test]
fn with_no_project_registered_the_clone_answer_names_the_missing_project() {
    let mut state = AppState::new_unrooted("/tmp/no-such-wt", "main", true, "/tmp/test.sock");

    let settings = state.handle(req("settings.get", json!({})));
    assert_eq!(
        settings["result"]["isolation_available"],
        json!({ "rift": false, "reason": "no project registered yet" }),
        "{settings:?}"
    );
}

/// Choosing to clone is kept where the volume can clone, and the answer the
/// setter returns is the settings themselves — the control repaints from
/// what the bridge holds, never from what it asked for.
#[test]
fn the_account_can_choose_cloning_where_the_volume_clones() {
    let (dir, repo) = init_repo();
    if !crate::isolation::probe::rift_or_skip(dir.path()) {
        return;
    }
    let mut state = qa_state(&repo, dir.path());

    let saved = state.handle(req("settings.set", json!({ "isolation": "rift" })));
    assert_eq!(saved["ok"], true, "{saved:?}");
    assert_eq!(saved["result"]["isolation"], "rift", "{saved:?}");
    assert_eq!(
        state.handle(req("settings.get", json!({})))["result"]["isolation"],
        "rift"
    );
}

/// A clone this machine cannot make is refused with the volume's own reason
/// before anything is stored, so the setting a client is shown afterwards is
/// the one that was already there.
#[test]
fn a_clone_this_machine_cannot_make_is_refused_and_changes_nothing() {
    let (dir, repo) = init_repo();
    let mut state = state_on_an_unclonable_project(dir.path(), &repo);

    let refused = state.handle(req("settings.set", json!({ "isolation": "rift" })));
    assert_eq!(refused["ok"], false, "{refused:?}");
    let sentence = refused["error"].as_str().unwrap().to_string();
    assert!(
        sentence.starts_with("Rift isolation is unavailable: ")
            && sentence.contains("linked worktree")
            && sentence.ends_with("; locked to worktrees"),
        "{sentence}"
    );

    let settings = state.handle(req("settings.get", json!({})));
    assert_eq!(settings["result"]["isolation"], "worktree", "{settings:?}");
    assert_eq!(
        settings["result"]["isolation_available"]["rift"], false,
        "{settings:?}"
    );
    assert_eq!(
        settings["result"]["isolation_available"]["reason"]
            .as_str()
            .unwrap(),
        sentence
            .trim_start_matches("Rift isolation is unavailable: ")
            .trim_end_matches("; locked to worktrees"),
        "the refusal quotes the reason the same read reports: {settings:?}"
    );
}

/// An isolation only a newer bridge knows is refused by name, and naming
/// the isolation alone is something to set: the emptiness check counts it
/// like every other field.
#[test]
fn an_unknown_isolation_is_refused_and_a_known_one_is_not_an_empty_set() {
    let (dir, repo) = init_repo();
    let mut state = state_on_an_unclonable_project(dir.path(), &repo);

    let refused = state.handle(req("settings.set", json!({ "isolation": "telepathy" })));
    assert_eq!(refused["ok"], false, "{refused:?}");
    assert_eq!(
        refused["error"].as_str().unwrap(),
        "unknown isolation \"telepathy\" (expected \"worktree\" or \"rift\")"
    );

    let saved = state.handle(req("settings.set", json!({ "isolation": "worktree" })));
    assert_eq!(saved["ok"], true, "{saved:?}");
    assert_eq!(saved["result"]["isolation"], "worktree", "{saved:?}");
}

/// A project row carries the whole isolation picture a control paints from:
/// what this project chose (nothing, while it inherits), what the account
/// chose, what its next checkout will actually be, and what its volume can
/// make — so no client composes any of it.
#[test]
fn a_project_row_carries_its_own_isolation_the_accounts_and_the_effective_one() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());

    let listed = state.handle(req("project.list", json!({})));
    let row = &listed["result"]["projects"][0];
    assert!(
        row["isolation"].is_null(),
        "a project that inherits names none of its own: {row:?}"
    );
    assert_eq!(row["isolation_default"], "worktree", "{row:?}");
    assert_eq!(row["isolation_effective"], "worktree", "{row:?}");
    let available = &row["isolation_available"];
    assert!(available["rift"].is_boolean(), "{row:?}");
    assert_eq!(
        available["reason"].is_null(),
        available["rift"] == true,
        "a locked clone carries its sentence and an available one carries none: {row:?}"
    );
}

/// A project's override is stored, answered as the row the control repaints
/// from, and written where a reload will find it; naming no isolation at all
/// clears it back to inheriting and unwrites it.
#[test]
fn a_project_override_is_stored_and_a_null_clears_it() {
    let tmp = tempfile::tempdir().unwrap();
    let (dir, repo) = init_repo();
    let cfg = tmp.path().join("config.json");
    let mut state = qa_state(&repo, dir.path()).with_config(&cfg).unwrap();
    let project_id = state.project_at(0).id.clone();
    let persisted_override = |cfg: &std::path::Path| -> Value {
        let written: Value = serde_json::from_str(&std::fs::read_to_string(cfg).unwrap()).unwrap();
        written["projects"][0]["isolation"].clone()
    };

    let saved = state.handle(req(
        "project.set_isolation",
        json!({ "project_id": project_id, "isolation": "worktree" }),
    ));
    assert_eq!(saved["ok"], true, "{saved:?}");
    assert_eq!(saved["result"]["isolation"], "worktree", "{saved:?}");
    assert_eq!(state.project_at(0).isolation, Some(Isolation::Worktree));
    assert_eq!(persisted_override(&cfg), json!("worktree"));

    let cleared = state.handle(req(
        "project.set_isolation",
        json!({ "project_id": project_id, "isolation": null }),
    ));
    assert_eq!(cleared["ok"], true, "{cleared:?}");
    assert!(
        cleared["result"]["isolation"].is_null(),
        "a cleared override inherits again: {cleared:?}"
    );
    assert_eq!(state.project_at(0).isolation, None);
    assert_eq!(persisted_override(&cfg), Value::Null);
}

/// A project on a volume that cannot clone is refused the choice in the
/// volume's own words, keeps the setting it had, and reports a next checkout
/// of the isolation every volume can make even while the account asks for
/// the other one.
#[test]
fn a_project_cannot_choose_a_clone_its_volume_cannot_make() {
    let (dir, repo) = init_repo();
    let mut state = state_on_an_unclonable_project(dir.path(), &repo);
    state.isolation = Isolation::Rift;
    let project_id = state.project_at(0).id.clone();

    let refused = state.handle(req(
        "project.set_isolation",
        json!({ "project_id": project_id, "isolation": "rift" }),
    ));
    assert_eq!(refused["ok"], false, "{refused:?}");
    let sentence = refused["error"].as_str().unwrap().to_string();
    assert!(
        sentence.starts_with("Rift isolation is unavailable: ")
            && sentence.contains("linked worktree")
            && sentence.ends_with("; locked to worktrees"),
        "{sentence}"
    );
    assert_eq!(
        state.project_at(0).isolation,
        None,
        "a refused choice stores nothing"
    );

    let listed = state.handle(req("project.list", json!({})));
    let row = &listed["result"]["projects"][0];
    assert!(row["isolation"].is_null(), "{row:?}");
    assert_eq!(row["isolation_default"], "rift", "{row:?}");
    assert_eq!(
        row["isolation_effective"], "worktree",
        "a locked volume makes the checkout every volume can: {row:?}"
    );
    assert_eq!(row["isolation_available"]["rift"], false, "{row:?}");
    assert!(
        row["isolation_available"]["reason"]
            .as_str()
            .unwrap()
            .contains("linked worktree"),
        "{row:?}"
    );
}

/// Where the volume clones, a project may ask for it while the account has
/// not: the row answers the override, the account's untouched default, and
/// the clone as the effective choice.
#[test]
fn a_project_may_choose_cloning_where_its_volume_clones() {
    let (dir, repo) = init_repo();
    if !crate::isolation::probe::rift_or_skip(dir.path()) {
        return;
    }
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.project_at(0).id.clone();

    let saved = state.handle(req(
        "project.set_isolation",
        json!({ "project_id": project_id, "isolation": "rift" }),
    ));
    assert_eq!(saved["ok"], true, "{saved:?}");
    let row = &saved["result"];
    assert_eq!(row["isolation"], "rift", "{row:?}");
    assert_eq!(row["isolation_default"], "worktree", "{row:?}");
    assert_eq!(row["isolation_effective"], "rift", "{row:?}");
    assert_eq!(state.project_at(0).isolation, Some(Isolation::Rift));
}

/// The setter fails fast on both ways of naming nothing: a project this
/// bridge does not hold, and a call that names no isolation at all — silence
/// is not the same as the explicit null that clears an override.
#[test]
fn project_set_isolation_refuses_an_unknown_project_and_an_unnamed_choice() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());

    let unknown = state.handle(req(
        "project.set_isolation",
        json!({ "project_id": "proj-nowhere", "isolation": null }),
    ));
    assert_eq!(unknown["ok"], false, "{unknown:?}");
    assert_eq!(
        unknown["error"].as_str().unwrap(),
        "unknown project: proj-nowhere"
    );

    let project_id = state.project_at(0).id.clone();
    let unnamed = state.handle(req(
        "project.set_isolation",
        json!({ "project_id": project_id }),
    ));
    assert_eq!(unnamed["ok"], false, "{unnamed:?}");
    assert_eq!(
        unnamed["error"].as_str().unwrap(),
        "missing required param: isolation"
    );
}

/// Every summary the conversation of `run_id` carries, whoever wrote it:
/// an Issue's implementation talks on the Issue's thread and a branch's
/// agent talks on its own, and a fallback note is legible on either.
fn conversation_summaries(state: &AppState, run_id: &str) -> Vec<String> {
    let thread = match state.runs[run_id]
        .run
        .plan_id
        .as_ref()
        .and_then(|issue_id| state.plans.get(&issue_id.0))
    {
        Some(issue) => issue.agents.sole_thread(),
        None => {
            &state.runs[run_id]
                .agents
                .primary()
                .expect("the run has an agent to talk to")
                .thread
        }
    };
    thread
        .items
        .iter()
        .filter_map(|item| match item {
            crate::thread::ThreadItem::Event(event) => event.summary.clone(),
            crate::thread::ThreadItem::Message(_) => None,
        })
        .collect()
}

/// The environment can change under a setting that was accepted when it was
/// true. A create never fails for it: the checkout is made the way every
/// volume can, and the conversation says so in the volume's own words, so
/// nobody is left wondering why the clone they chose is a worktree.
#[test]
fn a_create_that_cannot_clone_falls_back_and_says_so_on_the_conversation() {
    let (dir, repo) = init_repo();
    let mut state = state_on_an_unclonable_project(dir.path(), &repo);
    state.isolation = Isolation::Rift;
    let project_id = state.project_at(0).id.clone();

    let dispatched = state.handle(req(
        "branch.dispatch",
        json!({
            "project_id": project_id,
            "instruction": "Add a health endpoint",
        }),
    ));
    assert_eq!(dispatched["ok"], true, "{dispatched:?}");
    let run_id = dispatched["result"]["run_id"].as_str().unwrap().to_string();

    let checkout = state.runs[&run_id].worktree.path.clone();
    assert_eq!(
        Isolation::of(&checkout),
        Some(Isolation::Worktree),
        "the create fell back to the isolation this volume can make"
    );
    let note = conversation_summaries(&state, &run_id)
        .into_iter()
        .find(|summary| summary.starts_with("Created a git worktree: "))
        .unwrap_or_else(|| {
            panic!(
                "the fallback is on the conversation: {:?}",
                conversation_summaries(&state, &run_id)
            )
        });
    assert!(
        note.starts_with("Created a git worktree: Rift isolation is unavailable here — ")
            && note.contains("linked worktree"),
        "the note carries the volume's own sentence: {note}"
    );
}

/// A bare worktree has no run, no agent and no conversation, so the only
/// place the fallback can reach the human who asked for it is the answer to
/// the ask — the same sentence the log and every thread carry.
#[test]
fn a_bare_worktree_that_cannot_be_cloned_says_so_in_its_answer() {
    let (dir, repo) = init_repo();
    let mut state = state_on_an_unclonable_project(dir.path(), &repo);
    state.isolation = Isolation::Rift;
    let project_id = state.project_at(0).id.clone();

    let created = state.handle(req(
        "worktree.create",
        json!({ "project_id": project_id, "name": "somewhere to work" }),
    ));

    assert_eq!(created["ok"], true, "{created:?}");
    let result = &created["result"];
    assert_eq!(result["isolation"], "worktree", "{result:?}");
    let note = result["isolation_note"].as_str().unwrap_or_default();
    assert!(
        note.starts_with("Created a git worktree: Rift isolation is unavailable here — ")
            && note.contains("linked worktree"),
        "the answer carries the volume's own sentence: {result:?}"
    );
    let path = std::path::PathBuf::from(result["path"].as_str().unwrap_or_default());
    assert_eq!(
        Isolation::of(&path),
        Some(Isolation::Worktree),
        "and the checkout is the one this volume can make: {path:?}"
    );
}

/// The row that stands where a checkout will be says how that checkout is
/// being made, and what it says is the resolver's answer rather than the
/// account's ask: a board watching a create appear reads the same fact off
/// the row that it will read off the card.
#[test]
fn a_creating_row_carries_the_isolation_the_checkout_is_being_made_as() {
    let (dir, repo) = init_repo();
    let mut app = state_on_an_unclonable_project(dir.path(), &repo);
    app.isolation = Isolation::Rift;
    let project_id = app.project_at(0).id.clone();
    let (gate, gate_handle) = OffLockGate::new();
    app.off_lock_gate = Some(gate);
    let state = app.shared();

    let created = frame_on_a_thread(
        &state,
        "s-create",
        "worktree.create",
        json!({ "project_id": project_id, "name": "Scratch Space" }),
    );
    gate_handle.wait_for_arrival();

    let board = frame_on_a_thread(&state, "s-board", "board.list", json!({}))
        .recv_timeout(Duration::from_secs(10))
        .expect("the board answers while the checkout is being cut");
    let pending = pending_on_the_board(&board);
    assert_eq!(
        pending[0]["isolation"],
        json!("worktree"),
        "the row says what this volume can make, not what the account asked for: {pending:?}"
    );

    gate_handle.release();
    let created = created
        .recv_timeout(Duration::from_secs(30))
        .expect("the create answers once its git is done");
    assert_eq!(created["ok"], true, "{created:?}");
}

/// And on a volume that clones, the same create is a clone from the row
/// onwards: what the board is told while the git runs is what the checkout
/// turns out to be, and a clone that was made announces no fallback.
#[test]
fn a_create_under_cloning_stands_as_a_clone_and_settles_as_one() {
    let (dir, repo) = init_repo();
    if !crate::isolation::probe::rift_or_skip(dir.path()) {
        return;
    }
    let mut app = qa_state(&repo, dir.path());
    app.isolation = Isolation::Rift;
    let project_id = app.project_at(0).id.clone();
    let (gate, gate_handle) = OffLockGate::new();
    app.off_lock_gate = Some(gate);
    let state = app.shared();

    let created = frame_on_a_thread(
        &state,
        "s-create",
        "worktree.create",
        json!({ "project_id": project_id, "name": "Scratch Space" }),
    );
    gate_handle.wait_for_arrival();

    let board = frame_on_a_thread(&state, "s-board", "board.list", json!({}))
        .recv_timeout(Duration::from_secs(10))
        .expect("the board answers while the clone is being made");
    let pending = pending_on_the_board(&board);
    assert_eq!(
        pending[0]["isolation"],
        json!("rift"),
        "the row says the checkout being made is a clone: {pending:?}"
    );

    gate_handle.release();
    let created = created
        .recv_timeout(Duration::from_secs(30))
        .expect("the create answers once its git is done");
    assert_eq!(created["ok"], true, "{created:?}");
    let result = &created["result"];
    assert_eq!(result["isolation"], "rift", "{result:?}");
    assert!(
        result["isolation_note"].is_null(),
        "a clone that was made announces no fallback: {result:?}"
    );
    let path = std::path::PathBuf::from(result["path"].as_str().unwrap_or_default());
    assert_eq!(
        Isolation::of(&path),
        Some(Isolation::Rift),
        "and what stands on disk is the clone: {path:?}"
    );
}

/// The whole feature, end to end, on a volume that clones: the account
/// chooses cloning, the run that follows lives in a clone of the project
/// rather than a linked worktree, every surface that reads its work still
/// reads it, and Finish retains the checkout when no remote is configured.
#[test]
fn a_run_dispatched_under_cloning_lives_in_a_clone_and_finish_refuses_it() {
    let (dir, repo) = init_repo();
    if !crate::isolation::probe::rift_or_skip(dir.path()) {
        return;
    }
    let mut state = qa_state(&repo, dir.path());
    let chosen = state.handle(req("settings.set", json!({ "isolation": "rift" })));
    assert_eq!(chosen["ok"], true, "{chosen:?}");

    let (_, run_id) = planned_run_in_review(&mut state, "clone the project to work in it");
    let checkout = state.runs[&run_id].worktree.path.clone();
    assert_eq!(
        Isolation::of(&checkout),
        Some(Isolation::Rift),
        "the chosen isolation is what the run got: {checkout:?}"
    );
    assert!(
        conversation_summaries(&state, &run_id)
            .iter()
            .all(|summary| !summary.starts_with("Created a git worktree: ")),
        "a clone that was made announces no fallback"
    );

    let diff = state.handle(req("run.diff", json!({ "run_id": run_id })));
    let files: Vec<String> = diff["result"]["files"]
        .as_array()
        .unwrap_or_else(|| panic!("the clone's work is reviewable: {diff:?}"))
        .iter()
        .map(|file| file["path"].as_str().unwrap().to_string())
        .collect();
    assert!(
        files.contains(&"result-first-half.txt".to_string())
            && files.contains(&"result-second-half.txt".to_string()),
        "{files:?}"
    );

    let board = state.handle(req("board.list", json!({})));
    let row = board["result"]["runs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["run_id"] == json!(run_id.clone()))
        .unwrap_or_else(|| panic!("the run is on the feed: {board:?}"));
    assert_eq!(
        row["stat"]["branch"], row["branch"],
        "the feed reads the clone's own branch: {row:?}"
    );
    assert!(
        row["stat"]["files_changed"].as_u64().unwrap() >= 2,
        "the feed counts the clone's work: {row:?}"
    );

    // Done removes what it finishes, so it is not offered until the work is
    // somewhere else. No remote is configured here, so it never is.
    let finished = state.handle(req(
        "run.finish",
        json!({ "run_id": run_id, "action": "merge" }),
    ));
    assert_eq!(finished["ok"], false, "{finished:?}");
    assert!(
        finished["error"]
            .as_str()
            .unwrap()
            .contains("no remote has"),
        "{finished:?}"
    );
    assert!(
        checkout.join("result-first-half.txt").exists()
            && checkout.join("result-second-half.txt").exists(),
        "a refused Done retains the clone and its work"
    );
}

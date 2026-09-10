use super::*;

// ---- Review prioritization (triage) ----

/// The hunk ids of a run's current diff, and the revision hash that names
/// that diff — what a triage report has to speak in.
fn diff_vocabulary(state: &AppState, run_id: &str) -> (Vec<String>, String) {
    let project_id = state.project_of(run_id).expect("the run has a project");
    let patch = state
        .orch_for(&project_id)
        .expect("the project has an orchestrator")
        .run_diff(&state.runs[run_id])
        .expect("the worktree is readable")
        .patch()
        .to_string();
    (crate::diff::hunk_ids(&patch), sha256_hex(patch.as_bytes()))
}

fn done_triage(based_on: &str, hunk_ids: &[String]) -> DoneReport {
    DoneReport {
        phase: DonePhase::Triage,
        status: DoneStatus::Completed,
        summary: "the new file carries the risk".into(),
        outputs: DoneOutputs {
            triage: Some(crate::run::TriageReport {
                based_on: based_on.to_string(),
                hunks: hunk_ids
                    .iter()
                    .map(|hunk_id| crate::run::TriageHunk {
                        hunk_id: hunk_id.clone(),
                        level: crate::run::TriageLevel::Critical,
                        rationale: Some("new code path".into()),
                        group: None,
                    })
                    .collect(),
                overrides: Vec::new(),
            }),
            ..DoneOutputs::default()
        },
    }
}

/// A build that changed files is followed by a pass that orders the diff for
/// review — on the same agent, in the same worktree, naming the hunks it is
/// to classify. The run here is adopted and sitting in Review, so the report
/// moves no lifecycle state at all: triage gates nothing, so it does not
/// wait for the state machine's blessing either.
#[test]
fn a_completed_build_queues_a_triage_pass_for_the_worktrees_agent() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let run_id = adopted_run(&mut state, &repo, dir.path(), "triage-me");
    let root = AppState::canonical_root(&state.runs[&run_id].worktree.path);
    std::fs::write(
        state.runs[&run_id].worktree.path.join("crypto.rs"),
        "fn a() {}\n",
    )
    .unwrap();
    state.pending_agent_turns.clear();

    state.on_agent_done(
        &run_id,
        DoneReport {
            phase: DonePhase::Build,
            status: DoneStatus::Completed,
            summary: "added the key derivation".into(),
            outputs: DoneOutputs {
                completion_report: Some(crate::thread::CompletionReport {
                    critical_files: vec!["crypto.rs — key derivation".into()],
                    ..Default::default()
                }),
                ..DoneOutputs::default()
            },
        },
    );

    let queued = state
        .pending_agent_turns
        .last()
        .expect("a finished diff is ordered for review");
    assert_eq!(queued.phase, "triage");
    assert_eq!(queued.owner, run_id);
    assert_eq!(queued.root, root, "triage reads the diff where it lives");
    let (hunk_ids, revision_sha) = diff_vocabulary(&state, &run_id);
    for hunk_id in &hunk_ids {
        assert!(
            queued.said().warm.contains(hunk_id),
            "{hunk_id}: {}",
            queued.said().warm
        );
    }
    assert!(
        queued.said().warm.contains(&revision_sha),
        "{}",
        queued.said().warm
    );
    assert!(
        queued.said().warm.contains("crypto.rs — key derivation"),
        "the completion report seeds the pass: {}",
        queued.said().warm
    );
}

/// The pass lands on the run and ships to the SPA; when the diff moves out
/// from under it, the same pass still ships — labelled stale — and a new one
/// is queued for the revision that replaced it.
#[test]
fn a_triage_ships_with_the_run_and_goes_stale_when_the_diff_moves() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let run_id = adopted_run(&mut state, &repo, dir.path(), "triage-staleness");
    let worktree = state.runs[&run_id].worktree.path.clone();
    std::fs::write(worktree.join("crypto.rs"), "fn a() {}\n").unwrap();
    state.on_agent_done(
        &run_id,
        DoneReport {
            phase: DonePhase::Build,
            status: DoneStatus::Completed,
            summary: "first revision".into(),
            outputs: DoneOutputs::default(),
        },
    );

    let (hunk_ids, first_revision) = diff_vocabulary(&state, &run_id);
    state.on_agent_done(&run_id, done_triage(&first_revision, &hunk_ids));

    let view = state.handle(req("run.get", json!({ "run_id": run_id })));
    let triage = &view["result"]["triage"];
    assert_eq!(triage["based_on"], first_revision, "{view:?}");
    assert_eq!(triage["stale"], false, "{view:?}");
    assert_eq!(triage["hunks"][0]["hunk_id"], hunk_ids[0], "{view:?}");
    assert_eq!(triage["hunks"][0]["level"], "critical", "{view:?}");
    assert_eq!(triage["hunks"][0]["rationale"], "new code path", "{view:?}");

    // The diff moves: a second turn, a second revision.
    std::fs::write(worktree.join("crypto.rs"), "fn a() {}\nfn b() {}\n").unwrap();
    state.pending_agent_turns.clear();
    state.on_agent_done(
        &run_id,
        DoneReport {
            phase: DonePhase::Build,
            status: DoneStatus::Completed,
            summary: "second revision".into(),
            outputs: DoneOutputs::default(),
        },
    );

    let view = state.handle(req("run.get", json!({ "run_id": run_id })));
    let triage = &view["result"]["triage"];
    assert_eq!(
        triage["based_on"], first_revision,
        "the ordering the reviewer was reading is not thrown away: {view:?}"
    );
    assert_eq!(
        triage["stale"], true,
        "and it is labelled as describing the previous revision: {view:?}"
    );
    let (_, second_revision) = diff_vocabulary(&state, &run_id);
    assert_ne!(first_revision, second_revision);
    let queued = state
        .pending_agent_turns
        .last()
        .expect("a new revision is triaged again");
    assert_eq!(queued.phase, "triage");
    assert!(
        queued.said().warm.contains(&second_revision),
        "{}",
        queued.said().warm
    );
}

/// Triage asks the reviewer for nothing, so it must not ring their bell.
/// A pass finishing is status — it updates the review surface and says so
/// quietly, unlike the `done` that produced the diff in the first place.
#[test]
fn a_finished_triage_pass_updates_the_surface_without_asking_for_the_user() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let run_id = adopted_run(&mut state, &repo, dir.path(), "triage-quietly");
    std::fs::write(
        state.runs[&run_id].worktree.path.join("crypto.rs"),
        "fn a() {}\n",
    )
    .unwrap();
    state.on_agent_done(
        &run_id,
        DoneReport {
            phase: DonePhase::Build,
            status: DoneStatus::Completed,
            summary: "built".into(),
            outputs: DoneOutputs::default(),
        },
    );
    let seen = state.handle(req("entity.seen", json!({ "entity_id": run_id })));
    assert_eq!(seen["ok"], true, "{seen:?}");

    let summary_before =
        state.handle(req("run.get", json!({ "run_id": run_id })))["result"]["summary"].clone();
    let (hunk_ids, revision) = diff_vocabulary(&state, &run_id);
    state.on_agent_done(&run_id, done_triage(&revision, &hunk_ids));

    let view = state.handle(req("run.get", json!({ "run_id": run_id })));
    assert_eq!(
        view["result"]["unread"], false,
        "a background pass is not something the user is called to: {view:?}"
    );
    let events = view["result"]["thread"]["items"]
        .as_array()
        .expect("the conversation ships with the run");
    assert!(
        events.iter().any(|item| item["data"]["event"] == "triaged"),
        "the pass is on the record, as status: {events:?}"
    );
    assert_eq!(
        view["result"]["summary"], summary_before,
        "the card says exactly what it said before the pass: {view:?}"
    );
}

/// A revision nobody has changed is not re-triaged: the agent's turn is
/// worth more than a second opinion on the same diff.
#[test]
fn a_report_that_changed_nothing_does_not_ask_for_the_same_triage_twice() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let run_id = adopted_run(&mut state, &repo, dir.path(), "triage-once");
    std::fs::write(
        state.runs[&run_id].worktree.path.join("crypto.rs"),
        "fn a() {}\n",
    )
    .unwrap();
    let build = || DoneReport {
        phase: DonePhase::Build,
        status: DoneStatus::Completed,
        summary: "reported again".into(),
        outputs: DoneOutputs::default(),
    };
    state.on_agent_done(&run_id, build());
    let (hunk_ids, revision) = diff_vocabulary(&state, &run_id);
    state.on_agent_done(&run_id, done_triage(&revision, &hunk_ids));

    state.pending_agent_turns.clear();
    state.on_agent_done(&run_id, build());
    assert!(
        state.pending_agent_turns.is_empty(),
        "an unchanged diff is already ordered: {:?}",
        state
            .pending_agent_turns
            .iter()
            .map(|turn| turn.phase)
            .collect::<Vec<_>>()
    );
}

/// The run's current hunk ids, keyed by the file each belongs to. Every
/// test below writes one hunk per file, and the diff carries scaffolding of
/// its own — so a hunk is asked for by the name a reader would use.
fn hunks_by_path(state: &AppState, run_id: &str) -> HashMap<String, String> {
    let project_id = state.project_of(run_id).expect("the run has a project");
    let patch = state
        .orch_for(&project_id)
        .expect("the project has an orchestrator")
        .run_diff(&state.runs[run_id])
        .expect("the worktree is readable")
        .patch()
        .to_string();
    crate::diff::patch_hunks(&patch)
        .into_iter()
        .map(|hunk| (hunk.path, hunk.hunk_id))
        .collect()
}

/// An adopted run whose diff has been read and ordered by a triage pass,
/// with the pass's hunk ids by file.
fn triaged_run(
    state: &mut AppState,
    repo: &std::path::Path,
    dir: &std::path::Path,
    branch: &str,
    files: &[(&str, &str)],
) -> (String, HashMap<String, String>) {
    let run_id = adopted_run(state, repo, dir, branch);
    let worktree = state.runs[&run_id].worktree.path.clone();
    for (path, contents) in files {
        let path = worktree.join(path);
        std::fs::create_dir_all(path.parent().expect("a file sits in a directory")).unwrap();
        std::fs::write(path, contents).unwrap();
    }
    state.on_agent_done(
        &run_id,
        DoneReport {
            phase: DonePhase::Build,
            status: DoneStatus::Completed,
            summary: "wrote the change".into(),
            outputs: DoneOutputs::default(),
        },
    );
    let (hunk_ids, revision) = diff_vocabulary(state, &run_id);
    state.on_agent_done(&run_id, done_triage(&revision, &hunk_ids));
    let seen = state.handle(req("entity.seen", json!({ "entity_id": run_id })));
    assert_eq!(seen["ok"], true, "{seen:?}");
    let hunks = hunks_by_path(state, &run_id);
    (run_id, hunks)
}

fn review_rules_of(repo: &std::path::Path) -> Value {
    crate::review_rules::read(repo).expect("the rules file parses")
}

/// Disagreeing lands in all three places at once: the run's pass carries
/// the reviewer's level from here on, the agent that wrote the rationale is
/// told its rationale was rejected, and the project keeps the count. And
/// the telling is status — the reviewer has already done what they wanted,
/// so nothing is asked of anyone.
#[test]
fn a_reviewer_who_disagrees_with_a_pass_is_recorded_and_the_agent_is_told() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (run_id, hunks) = triaged_run(
        &mut state,
        &repo,
        dir.path(),
        "triage-override",
        &[("src/crypto.rs", "fn derive() {}\n")],
    );
    let crypto = &hunks["src/crypto.rs"];

    let overridden = state.handle(req(
        "triage.override",
        json!({
            "run_id": run_id,
            "hunk_id": crypto,
            "direction": "surface",
            "note": "key derivation is never boilerplate",
        }),
    ));
    assert_eq!(overridden["ok"], true, "{overridden:?}");
    assert_eq!(overridden["result"]["path"], "src/crypto.rs");
    assert_eq!(overridden["result"]["rule"]["pattern"], "src/*.rs");
    assert_eq!(overridden["result"]["rule"]["count"], 1);

    let view = state.handle(req("run.get", json!({ "run_id": run_id })));
    let overrides = view["result"]["triage"]["overrides"]
        .as_array()
        .expect("the pass carries what the reviewer said");
    assert_eq!(overrides.len(), 1, "{view:?}");
    assert_eq!(overrides[0]["hunk_id"], crypto.as_str());
    assert_eq!(overrides[0]["direction"], "surface");
    assert_eq!(
        overrides[0]["note"], "key derivation is never boilerplate",
        "{view:?}"
    );
    assert!(
        overrides[0]["at"].as_str().is_some_and(|at| !at.is_empty()),
        "{view:?}"
    );

    let told = view["result"]["thread"]["items"]
        .as_array()
        .expect("the conversation ships with the run")
        .iter()
        .find(|item| item["data"]["event"] == "triage_overridden")
        .cloned()
        .unwrap_or_else(|| panic!("the agent is told: {view:?}"));
    let summary = told["data"]["summary"].as_str().expect("it says something");
    assert!(
        summary.contains("src/crypto.rs"),
        "it names the file, not the hash: {summary}"
    );
    assert!(
        summary.contains("key derivation is never boilerplate"),
        "and the reviewer's own words: {summary}"
    );
    assert!(
        summary.contains("new code path"),
        "and the rationale that was rejected, so the agent reading this can \
         see which of its own claims the reviewer did not believe: {summary}"
    );
    assert_eq!(
        view["result"]["unread"], false,
        "the reviewer already did the thing; nothing is asked back: {view:?}"
    );
}

/// The durable half. One disagreement is nearly worthless and the fourth in
/// the same directory is not, so the project-level record is keyed on the
/// pattern the file shares with its neighbours and counted.
#[test]
fn disagreements_accumulate_as_one_counted_rule_in_the_primary_checkout() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (run_id, hunks) = triaged_run(
        &mut state,
        &repo,
        dir.path(),
        "triage-rules",
        &[
            ("src/crypto.rs", "fn derive() {}\n"),
            ("src/session.rs", "fn open() {}\n"),
        ],
    );
    assert_eq!(
        review_rules_of(&repo),
        Value::Null,
        "nothing is written until somebody disagrees"
    );

    for hunk_id in [&hunks["src/crypto.rs"], &hunks["src/session.rs"]] {
        let overridden = state.handle(req(
            "triage.override",
            json!({ "run_id": run_id, "hunk_id": hunk_id, "direction": "surface" }),
        ));
        assert_eq!(overridden["ok"], true, "{overridden:?}");
    }

    let rules = review_rules_of(&repo);
    let rules = rules["rules"].as_array().expect("a list of rules");
    assert_eq!(
        rules.len(),
        1,
        "two files, one pattern, one rule: {rules:?}"
    );
    assert_eq!(rules[0]["pattern"], "src/*.rs");
    assert_eq!(rules[0]["direction"], "surface");
    assert_eq!(rules[0]["count"], 2, "{rules:?}");
}

/// A reviewer toggling the same hunk the same way twice has said one thing,
/// not two. The count is signal about the pattern, and double-counting one
/// click would make it lie.
#[test]
fn saying_the_same_thing_about_the_same_hunk_twice_counts_once() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (run_id, hunks) = triaged_run(
        &mut state,
        &repo,
        dir.path(),
        "triage-repeat",
        &[("src/crypto.rs", "fn derive() {}\n")],
    );
    let crypto = hunks["src/crypto.rs"].clone();
    let same = || json!({ "run_id": run_id, "hunk_id": crypto, "direction": "collapse" });

    assert_eq!(state.handle(req("triage.override", same()))["ok"], true);
    let again = state.handle(req("triage.override", same()));
    assert_eq!(again["ok"], true, "{again:?}");
    assert_eq!(
        again["result"]["rule"]["count"],
        Value::Null,
        "nothing new was said, so nothing was counted: {again:?}"
    );

    let view = state.handle(req("run.get", json!({ "run_id": run_id })));
    assert_eq!(
        view["result"]["triage"]["overrides"]
            .as_array()
            .expect("overrides")
            .len(),
        1,
        "one hunk carries one disagreement: {view:?}"
    );
    assert_eq!(review_rules_of(&repo)["rules"][0]["count"], 1);
}

/// The reviewer disagrees with a reading they were shown. A hunk id no pass
/// classified was never shown to them, and a direction that is neither way
/// is not a disagreement — both are refused before anything is written.
#[test]
fn a_disagreement_with_something_that_was_never_classified_is_refused() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (run_id, hunks) = triaged_run(
        &mut state,
        &repo,
        dir.path(),
        "triage-refuse",
        &[("src/crypto.rs", "fn derive() {}\n")],
    );

    let invented = state.handle(req(
        "triage.override",
        json!({ "run_id": run_id, "hunk_id": "hnotinthispass", "direction": "surface" }),
    ));
    assert_eq!(invented["ok"], false, "{invented:?}");

    let sideways = state.handle(req(
        "triage.override",
        json!({ "run_id": run_id, "hunk_id": hunks["src/crypto.rs"], "direction": "maybe" }),
    ));
    assert_eq!(sideways["ok"], false, "{sideways:?}");

    assert_eq!(
        review_rules_of(&repo),
        Value::Null,
        "a refused call writes nothing"
    );
    let view = state.handle(req("run.get", json!({ "run_id": run_id })));
    assert_eq!(view["result"]["triage"]["overrides"], json!([]), "{view:?}");
}

/// A hunk id is derived from its content, so a hunk the next pass still
/// names is the same hunk — and what the reviewer said about it still
/// holds. One that changed is a different hunk, and the disagreement does
/// not follow it; the project's rules are what outlive the diff.
#[test]
fn a_re_triage_keeps_what_the_reviewer_said_about_the_hunks_it_still_names() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (run_id, hunks) = triaged_run(
        &mut state,
        &repo,
        dir.path(),
        "triage-carry",
        &[
            ("src/crypto.rs", "fn derive() {}\n"),
            ("src/session.rs", "fn open() {}\n"),
        ],
    );
    for hunk_id in [&hunks["src/crypto.rs"], &hunks["src/session.rs"]] {
        let overridden = state.handle(req(
            "triage.override",
            json!({ "run_id": run_id, "hunk_id": hunk_id, "direction": "surface" }),
        ));
        assert_eq!(overridden["ok"], true, "{overridden:?}");
    }

    // One file moves; the other is untouched, so its hunk id survives.
    let worktree = state.runs[&run_id].worktree.path.clone();
    std::fs::write(
        worktree.join("src/session.rs"),
        "fn open() {}\nfn shut() {}\n",
    )
    .unwrap();
    state.on_agent_done(
        &run_id,
        DoneReport {
            phase: DonePhase::Build,
            status: DoneStatus::Completed,
            summary: "moved the diff".into(),
            outputs: DoneOutputs::default(),
        },
    );
    let (next_hunk_ids, next_revision) = diff_vocabulary(&state, &run_id);
    state.on_agent_done(&run_id, done_triage(&next_revision, &next_hunk_ids));

    let moved = hunks_by_path(&state, &run_id);
    assert_eq!(
        moved["src/crypto.rs"], hunks["src/crypto.rs"],
        "the untouched file's hunk is the same hunk"
    );
    assert_ne!(
        moved["src/session.rs"], hunks["src/session.rs"],
        "the edited file's hunk is a different one"
    );

    let view = state.handle(req("run.get", json!({ "run_id": run_id })));
    let carried: Vec<&str> = view["result"]["triage"]["overrides"]
        .as_array()
        .expect("overrides")
        .iter()
        .map(|entry| entry["hunk_id"].as_str().expect("a hunk id"))
        .collect();
    assert_eq!(carried, vec![hunks["src/crypto.rs"].as_str()], "{view:?}");
}

/// An override is the reviewer's word about the reviewer's own reading. A
/// pass claiming to carry one is claiming to have been the human, and is
/// refused whole rather than quietly stripped of the claim.
#[test]
fn a_pass_that_reports_the_reviewers_overrides_for_them_is_refused() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (run_id, hunks) = triaged_run(
        &mut state,
        &repo,
        dir.path(),
        "triage-forgery",
        &[("src/crypto.rs", "fn derive() {}\n")],
    );

    let (hunk_ids, revision) = diff_vocabulary(&state, &run_id);
    let mut forged = done_triage(&revision, &hunk_ids);
    forged
        .outputs
        .triage
        .as_mut()
        .expect("a triage report")
        .overrides = vec![crate::run::TriageOverride {
        hunk_id: hunks["src/crypto.rs"].clone(),
        direction: crate::run::OverrideDirection::Collapse,
        note: Some("the reviewer never said this".into()),
        at: now_rfc3339(),
    }];
    state.on_agent_done(&run_id, forged);

    let view = state.handle(req("run.get", json!({ "run_id": run_id })));
    assert_eq!(
        view["result"]["triage"]["overrides"],
        json!([]),
        "the forged pass never landed: {view:?}"
    );
}

use super::support::*;
use super::*;

#[test]
fn boot_requeues_only_safe_intents_and_marks_claimed_handoffs_uncertain() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let record = plan_record("issue-1");
    let queued = queued_operation("queued", 1);
    let claimed = queued_operation("claimed", 2);
    store
        .accept_thread_post("issue-1", &record.agents, &queued)
        .unwrap();
    store
        .accept_thread_post("issue-1", &record.agents, &claimed)
        .unwrap();
    assert!(store
        .transition_operation(
            "claimed",
            OperationStatus::Queued,
            OperationStatus::Claimed,
            None,
        )
        .unwrap());
    drop(store);

    let reopened = Store::new(dir.path()).unwrap();
    let recovered = reopened.recover_operations().unwrap();
    let ambiguous = recovered
        .iter()
        .find(|receipt| receipt.operation_id == "claimed")
        .unwrap();
    assert_eq!(ambiguous.status, OperationStatus::Uncertain);
    assert_eq!(
        ambiguous.execution_error.as_deref(),
        Some("provider handoff outcome unknown after restart")
    );
    assert_eq!(
        recovered
            .iter()
            .find(|receipt| receipt.operation_id == "queued")
            .unwrap()
            .status,
        OperationStatus::Queued
    );
}

/// Two writers replacing one file at once — two planning workspaces
/// appending to one `.git/info/exclude` — must each stage their own
/// sibling: a shared temp name has the second `create` truncate the first
/// writer's bytes and the second `rename` find nothing to rename.
#[test]
fn two_writers_of_one_file_never_share_a_temp_sibling() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("exclude");
    let contents = [
        "first writer\n".repeat(2048),
        "second writer\n".repeat(2048),
    ];
    let writers: Vec<_> = contents
        .iter()
        .map(|contents| {
            let path = path.clone();
            let contents = contents.clone();
            std::thread::spawn(move || {
                for _ in 0..200 {
                    write_file_atomically(&path, &contents).unwrap();
                }
            })
        })
        .collect();
    for writer in writers {
        writer.join().unwrap();
    }
    let settled = std::fs::read_to_string(&path).unwrap();
    assert!(
        contents.contains(&settled),
        "the file is neither writer's whole text"
    );
    let leftovers: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .filter(|name| name != "exclude")
        .collect();
    assert!(
        leftovers.is_empty(),
        "temp files left beside the target: {leftovers:?}"
    );
}

/// The history a load left in the store is not history this process may
/// throw away. Saving a conversation whose tail is all the daemon read
/// must not read the missing items as items that were taken off it.
#[test]
fn saving_a_tail_leaves_the_history_it_never_read_in_place() {
    let dir = tempfile::tempdir().unwrap();
    let held = RESIDENT_CONVERSATION_TAIL + 60;
    let (store, agent_id) = store_with_conversation(&dir.path().join("tasks"), held);

    let mut reloaded = reload_run(&store, "run-1");
    reloaded.agents[0].thread.post_user("one more", None, NOW);
    store.save_run(&reloaded).expect("the append saves");

    assert_eq!(
        store.thread_item_count(&agent_id).expect("the count reads"),
        held as u64 + 1,
        "the conversation lost the history the daemon never read"
    );
    assert_eq!(
        store
            .thread_page(&agent_id, Some(2), 1)
            .expect("a page reads")
            .first()
            .map(ThreadItem::sequence),
        Some(1),
        "the oldest item is still there"
    );
}

/// The end of the walk. Asking for what precedes the oldest item is the
/// ordinary way a client learns the conversation has no more history, so
/// it answers empty rather than failing or wrapping around.
#[test]
fn paging_before_the_oldest_item_returns_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (store, agent_id) = store_with_conversation(&dir.path().join("tasks"), 10);

    let oldest = store
        .thread_page(&agent_id, None, 10)
        .expect("a page reads")
        .first()
        .map(ThreadItem::sequence)
        .expect("the conversation has an oldest item");
    let page = store
        .thread_page(&agent_id, Some(oldest), 10)
        .expect("a page reads");
    assert!(page.is_empty(), "{:?}", sequences(&page));
}

/// A panic anywhere under the connection lock poisons the mutex, and a
/// store that treats poison as fatal answers every later call with a panic
/// of its own. The daemon would stay up and connected while it could
/// neither read nor write a thing, which is a far worse failure than the
/// one panic that started it.
#[test]
fn a_panic_under_the_connection_lock_does_not_wedge_the_store() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().join("tasks")).expect("store opens");
    store
        .save_run(&run_record("run-1", None, NOW))
        .expect("the run saves");

    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _held = store.connection();
        panic!("a store call panicked while it held the connection");
    }));
    std::panic::set_hook(previous_hook);
    assert!(panicked.is_err(), "the test did not poison the mutex");

    store
        .save_run(&run_record("run-2", None, NOW))
        .expect("a write after the poisoning still lands");
    assert_eq!(
        store
            .load_all_runs()
            .expect("a read after the poisoning still runs")
            .len(),
        2
    );
}

/// The census's index by name. A count over a span answered off the
/// primary key reads every item in the span to test `tool_call` on each —
/// the whole cost of a thousand-call run, paid to print one number. The
/// partial index holds only the calls, so the count is a seek down them
/// and the plan says COVERING: the row itself is never touched.
#[test]
fn the_tool_call_census_reads_through_the_tool_call_index() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().join("tasks")).expect("store opens");
    let connection = store.connection();
    let plan = query_plan(&connection, THREAD_TOOL_CALL_COUNT_SQL);
    assert!(
        plan.iter()
            .any(|step| step.contains("COVERING INDEX thread_items_tool_calls")),
        "the census does not use thread_items_tool_calls: {plan:?}"
    );
}

/// The failure the daemon dies on. Under a KeepAlive supervisor it dies on
/// it once a second, so the message is the only thing standing between the
/// user and a silent restart loop: it has to name the file.
#[test]
fn a_store_that_cannot_be_opened_names_its_path() {
    let dir = tempfile::tempdir().unwrap();
    let occupied = dir.path().join("tasks");
    std::fs::write(&occupied, "not a directory").unwrap();

    let Err(error) = Store::new(&occupied) else {
        panic!("a file where the store directory belongs must fail to open");
    };
    let message = error.to_string();
    assert!(
        message.contains(&occupied.display().to_string()),
        "the failure does not name the store: {message}"
    );
}

/// A run that belongs to an Issue is filed under it by `save_run` alone —
/// there is no second write path for implementations, and the `issue_id`
/// column comes off `plan_id` whichever way the run got here.
#[test]
fn save_run_files_a_run_under_the_issue_its_plan_id_names() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().join("tasks")).expect("store opens");
    store
        .save_issue_plan(&plan_record("plan-1"))
        .expect("the Issue saves");
    store
        .save_run(&run_record("run-1", Some("plan-1"), NOW))
        .expect("the run saves");

    let issues = store.load_all_issues().expect("issues load");
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].implementations.len(), 1);
    assert_eq!(issues[0].implementations[0].id, "run-1");
}

/// Boot reattaches in creation order, so the loaders have to hand records
/// back oldest first — a plan is recovered before the run that reads its
/// record.
#[test]
fn records_load_oldest_first() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().join("tasks")).expect("store opens");
    for (id, created) in [
        ("run-late", "2026-08-21T12:00:00Z"),
        ("run-early", "2026-08-21T08:00:00Z"),
        ("run-middle", "2026-08-21T10:00:00Z"),
    ] {
        store
            .save_run(&run_record(id, None, created))
            .expect("the run saves");
    }
    let order: Vec<String> = store
        .load_all_runs()
        .expect("runs load")
        .into_iter()
        .map(|run| run.id)
        .collect();
    assert_eq!(order, vec!["run-early", "run-middle", "run-late"]);
}

/// The two readings of the counted rule — `ThreadItem::counted()` and the
/// store's `message = 1 OR attention = 1` — held equal over every kind
/// there is, so the hoisted columns cannot drift from the enum they were
/// written off.
#[test]
fn the_hoisted_columns_agree_with_the_counted_rule() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().join("tasks")).expect("store opens");
    let mut record = run_record("run-1", None, NOW);
    for kind in crate::thread::ThreadEventKind::ALL {
        record.agents[0]
            .thread
            .push_event(kind, Some(kind.as_str().to_string()), None, None, NOW);
    }
    record.agents[0].thread.post_user("a question", None, NOW);
    record.agents[0].thread.post_agent("an answer", None, NOW);
    record.agents[0]
        .thread
        .post_agent_progress("still going", None, NOW);
    store.save_run(&record).expect("the conversation saves");

    let counted_in_sql: Vec<u64> = {
        let connection = store.connection();
        let mut statement = connection
            .prepare(
                "SELECT sequence FROM thread_items \
                     WHERE agent_id = ?1 AND (message = 1 OR attention = 1) ORDER BY sequence",
            )
            .expect("the predicate prepares");
        statement
            .query_map([&record.agents[0].id], |row| row.get::<_, i64>(0))
            .expect("the predicate reads")
            .map(|sequence| sequence.expect("a row reads") as u64)
            .collect()
    };
    let counted_in_rust: Vec<u64> = record.agents[0]
        .thread
        .items
        .iter()
        .filter(|item| item.counted())
        .map(ThreadItem::sequence)
        .collect();

    assert!(!counted_in_rust.is_empty(), "the fixture counts nothing");
    assert_eq!(counted_in_sql, counted_in_rust);
}

/// A tool call answered after it was stored is rewritten where it sits, and
/// nothing else is: the `updated_sequence` column has carried in-place
/// mutations since the store landed, and an event bump rides it with no SQL
/// change at all.
///
/// The reload is the other half. `next_sequence` is repaired from the
/// items' newest counter value, so the bump travels with them and the
/// conversation carries on above it rather than spending a value twice.
#[test]
fn an_answered_tool_call_is_rewritten_in_place_and_survives_a_reload() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("tasks");
    let store = Store::new(&root).expect("store opens");
    let mut record = run_record("run-1", None, NOW);
    let thread = &mut record.agents[0].thread;
    let call = thread.push_event(
        crate::thread::ThreadEventKind::ToolUse,
        Some("Read bridge/src/app.rs".to_string()),
        None,
        None,
        NOW,
    );
    thread.push_event(
        crate::thread::ThreadEventKind::Narration,
        Some("dropped the index".to_string()),
        None,
        None,
        NOW,
    );
    store.save_run(&record).expect("the conversation saves");
    let cursor = record.agents[0].thread.last_sequence();
    let agent_id = record.agents[0].id.clone();

    assert!(record.agents[0].thread.resolve_tool_call(
        call,
        crate::thread::ToolCallOutcome::Ok,
        "fn main() {}"
    ));
    store.save_run(&record).expect("the answer saves");

    let delta = store
        .thread_items_after(&agent_id, cursor)
        .expect("the cursor reads");
    assert_eq!(delta.len(), 1, "the answered row and no other: {delta:?}");
    let crate::thread::ThreadItem::Event(event) = &delta[0] else {
        panic!("{delta:?}");
    };
    assert_eq!(event.sequence, call);
    assert_eq!(event.outcome, Some(crate::thread::ToolCallOutcome::Ok));
    assert_eq!(
        event.summary.as_deref(),
        Some("Read bridge/src/app.rs\n→ fn main() {}")
    );
    assert_eq!(
        store
            .thread_items_after(&agent_id, 0)
            .expect("the whole conversation reads")
            .len(),
        2,
        "and the rewrite replaced the row rather than adding one"
    );

    let mut reloaded = store
        .load_all_runs()
        .expect("runs load")
        .into_iter()
        .find(|run| run.id == "run-1")
        .expect("the run is there");
    let thread = &mut reloaded.agents[0].thread;
    assert_eq!(thread.last_sequence(), event.updated_sequence);
    let minted = thread.push_event(
        crate::thread::ThreadEventKind::Narration,
        Some("and carried on".to_string()),
        None,
        None,
        NOW,
    );
    assert!(
        minted > event.updated_sequence,
        "a reload clears the bump, so no counter value is spent twice: {minted}"
    );
}

/// The census a page's activity digests are counted with: exact over the
/// whole run, whatever a page shipped of it, and counted in SQL off the
/// hoisted column rather than by reading the items back.
#[test]
fn rows_and_tool_calls_are_counted_in_sql_over_a_span() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().join("tasks")).expect("store opens");
    let mut record = run_record("run-1", None, NOW);
    record.agents[0].thread.post_user("rename it", None, NOW);
    for index in 0..40 {
        record.agents[0].thread.push_event(
            crate::thread::ThreadEventKind::ToolUse,
            Some(format!("Read file-{index}.rs")),
            None,
            None,
            NOW,
        );
        record.agents[0].thread.push_event(
            crate::thread::ThreadEventKind::Reasoning,
            Some("thinking".to_string()),
            None,
            None,
            NOW,
        );
    }
    store.save_run(&record).expect("the conversation saves");
    let agent_id = record.agents[0].id.clone();
    let last = record.agents[0].thread.last_sequence();

    assert_eq!(
        store
            .run_census(&agent_id, 2, last)
            .expect("the census counts"),
        RunCensus {
            tool_calls: 40,
            rows: 80
        },
        "every tool call of the run, and the thinking between them is rows"
    );
    // Inclusive at both ends, which is what a digest's span means.
    assert_eq!(
        store
            .run_census(&agent_id, 2, 2)
            .expect("the census counts"),
        RunCensus {
            tool_calls: 1,
            rows: 1
        }
    );
    assert_eq!(
        store
            .run_census(&agent_id, 1, 1)
            .expect("the census counts"),
        RunCensus::default(),
        "a message is neither a tool call nor a row of work"
    );
}

/// The attention map is pruned to the entities that still exist, so it
/// tracks the world rather than growing forever.
#[test]
fn saving_attention_prunes_entities_that_no_longer_exist() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().join("tasks")).expect("store opens");
    let mut map = HashMap::new();
    map.insert("run-live".to_string(), Attention::default());
    map.insert("run-gone".to_string(), Attention::default());
    let live: HashSet<String> = ["run-live".to_string()].into_iter().collect();

    store.save_attention(&map, &live).expect("attention saves");
    let loaded = store.load_attention();
    assert!(loaded.contains_key("run-live"));
    assert!(!loaded.contains_key("run-gone"), "a dead id was kept");
}

/// A capture is durable before anything is decided about it: what the user
/// said survives a store that is opened again from scratch, routing and
/// all.
#[test]
fn a_capture_round_trips_with_everything_decided_about_it() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().join("tasks")).expect("store opens");
    let mut capture = Capture::new(
        "capture-1",
        "fix the login redirect",
        "2026-08-13T10:00:00Z",
    );
    capture.state = CaptureState::Routed;
    capture.routing = Some(CaptureRouting {
        project_id: "p1".to_string(),
        kind: CaptureTarget::Issue,
        target_id: "plan-7".to_string(),
        routed_at: "2026-08-13T10:00:05Z".to_string(),
        rationale: Some("no branch names this work".to_string()),
    });
    store.save_capture(&capture).unwrap();

    let reopened = Store::new(dir.path().join("tasks")).expect("store opens");
    assert_eq!(reopened.load_all_captures().unwrap(), vec![capture]);
}

/// Re-saving a capture replaces it in place rather than filing a second
/// copy: one capture, one record, however many times routing touches it.
#[test]
fn saving_a_capture_again_replaces_the_record() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().join("tasks")).expect("store opens");
    let mut capture = Capture::new("capture-1", "ship it", "2026-08-13T10:00:00Z");
    store.save_capture(&capture).unwrap();
    capture.state = CaptureState::Routing;
    store.save_capture(&capture).unwrap();

    let loaded = store.load_all_captures().unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].state, CaptureState::Routing);
}

/// Captures come back oldest first, so the order they were said in is the
/// order they are read in.
#[test]
fn captures_load_in_the_order_they_were_said() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().join("tasks")).expect("store opens");
    for (id, said_at) in [
        ("capture-b", "2026-08-13T10:00:02Z"),
        ("capture-a", "2026-08-13T10:00:01Z"),
        ("capture-c", "2026-08-13T10:00:03Z"),
    ] {
        store
            .save_capture(&Capture::new(id, "something", said_at))
            .unwrap();
    }
    let ids: Vec<String> = store
        .load_all_captures()
        .unwrap()
        .into_iter()
        .map(|capture| capture.id)
        .collect();
    assert_eq!(ids, vec!["capture-a", "capture-b", "capture-c"]);
}

/// A capture the user abandoned is gone, and gone across a reboot: the one
/// deletion this store does, and it happens only when they asked for it.
#[test]
fn a_cancelled_capture_is_forgotten_and_stays_forgotten() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().join("tasks")).expect("store opens");
    store
        .save_capture(&Capture::new(
            "capture-1",
            "ship it",
            "2026-08-13T10:00:00Z",
        ))
        .unwrap();
    store
        .save_capture(&Capture::new(
            "capture-2",
            "and this",
            "2026-08-13T10:00:01Z",
        ))
        .unwrap();

    store.delete_capture("capture-1").unwrap();
    let ids: Vec<String> = store
        .load_all_captures()
        .unwrap()
        .into_iter()
        .map(|capture| capture.id)
        .collect();
    assert_eq!(ids, vec!["capture-2"], "only the one asked for");

    store
        .delete_capture("capture-1")
        .expect("forgetting what is already forgotten is not an error");
}

/// No captures dir means no captures — a first boot is not an error.
#[test]
fn a_store_with_no_captures_yet_loads_none() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().join("tasks")).expect("store opens");
    assert_eq!(store.load_all_captures().unwrap(), Vec::new());
}

/// Naming a stored agent needs its record, never its conversation (#131):
/// the roster reads answer every run and every Issue with the agents the
/// full reads restore — the same members, in the same order — and leave each
/// conversation in the database.
#[test]
fn the_roster_reads_name_every_agent_the_full_reads_do_without_a_conversation() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().join("tasks")).expect("store opens");
    let mut plan = plan_record("plan-1");
    plan.agents[0].name = Some("Planner".into());
    plan.agents[0]
        .thread
        .post_user("plan this".to_string(), None, NOW);
    store.save_issue_plan(&plan).expect("the Issue saves");
    for (id, issue, created) in [
        ("run-late", Some("plan-1"), "2026-08-21T12:00:00Z"),
        ("run-early", None, "2026-08-21T08:00:00Z"),
    ] {
        let mut run = run_record(id, issue, created);
        run.agents[0].name = Some(format!("Builder of {id}"));
        for n in 0..5 {
            run.agents[0]
                .thread
                .post_user(format!("message {n}"), None, created);
        }
        store.save_run(&run).expect("the run saves");
    }

    let runs = store.load_all_runs().expect("runs load");
    let run_rosters = store.load_all_run_rosters().expect("run rosters load");
    let plans = store.load_all_plans().expect("plans load");
    let plan_rosters = store.load_all_plan_rosters().expect("plan rosters load");

    let members = |runs: &[PersistedRun]| -> Vec<_> {
        runs.iter()
            .map(|run| (run.id.clone(), run.worktree_path.clone(), run.members()))
            .collect()
    };
    assert_eq!(members(&run_rosters), members(&runs));
    assert_eq!(
        plan_rosters
            .iter()
            .map(|plan| plan.members())
            .collect::<Vec<_>>(),
        plans.iter().map(|plan| plan.members()).collect::<Vec<_>>()
    );
    assert_eq!(
        run_rosters[1].members()[0].name.as_deref(),
        Some("Builder of run-late")
    );
    assert!(run_rosters
        .iter()
        .flat_map(|run| &run.agents)
        .chain(plan_rosters.iter().flat_map(|plan| &plan.agents))
        .all(|agent| agent.thread.items.is_empty()));
}

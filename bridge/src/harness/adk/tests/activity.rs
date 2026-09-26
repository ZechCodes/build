// Exact test bodies moved from the former inline test module.
use super::*;

#[test]
fn a_started_shell_that_has_named_no_output_file_starts_no_poller() {
    let mut reader = reader_with_a_live_activity_slot();

    read_lines_into(
        &mut reader,
        &fixture_lines_numbered(SHELL_AND_CHECKLIST_FIXTURE, &[SHELL_STARTED_LINE]),
    );

    assert!(
        running_shell_outputs(&reader.state).is_empty(),
        "the start alone names no file to tail"
    );
    assert!(no_poller_is_running(&reader));
}
#[test]
fn the_answer_naming_the_output_file_starts_exactly_one_poller() {
    let mut reader = ReaderEndingItsSessionWhenDropped {
        reader: reader_with_a_live_activity_slot(),
    };
    let launched = fixture_lines_numbered(
        SHELL_AND_CHECKLIST_FIXTURE,
        &[
            SHELL_LAUNCH_CALL_LINE,
            SHELL_STARTED_LINE,
            SHELL_LAUNCH_ANSWER_LINE,
        ],
    );

    read_lines_into(&mut reader, &launched);
    let started = polling_thread_of(&reader);

    read_lines_into(&mut reader, &launched);
    reader.ensure_shell_tail_poller();

    assert_eq!(
        polling_thread_of(&reader),
        started,
        "a running poller is never joined by a second"
    );
}
#[test]
fn a_growing_output_file_reaches_the_snapshot_and_moves_the_revision() {
    let (_directory, output_path) = shell_output_file_holding("");
    let reader = reader_tailing(SHELL_TASK_ID, &output_path);
    let before_it_grew = revision_counter_of(&reader);

    std::fs::write(&output_path, hundred_numbered_lines()).expect("the output file grows");

    assert!(
        becomes_true_within(Duration::from_secs(3), || {
            tail_of_the_shell(&reader, SHELL_TASK_ID).len() == SHELL_TAIL_LINES
        }),
        "the tail never reached the file: {:?}",
        tail_of_the_shell(&reader, SHELL_TASK_ID)
    );
    let tailed = tail_of_the_shell(&reader, SHELL_TASK_ID);
    assert_eq!(tailed.first().map(String::as_str), Some("line 81"));
    assert_eq!(tailed.last().map(String::as_str), Some("line 100"));
    assert!(
        revision_counter_of(&reader) > before_it_grew,
        "the poller said the snapshot moved"
    );
}
#[test]
fn a_tail_that_has_not_changed_moves_the_revision_not_at_all() {
    let (_directory, output_path) = shell_output_file_holding(&hundred_numbered_lines());
    let reader = reader_tailing(SHELL_TASK_ID, &output_path);

    assert!(becomes_true_within(Duration::from_secs(3), || {
        !tail_of_the_shell(&reader, SHELL_TASK_ID).is_empty()
    }));
    stop_shell_polling(&reader);
    let once_the_tail_landed = revision_counter_of(&reader);
    super::super::activity::poll_shell_tails(
        &reader.state,
        &reader.revision,
        running_shell_outputs(&reader.state),
    );

    assert_eq!(
        revision_counter_of(&reader),
        once_the_tail_landed,
        "a file that did not change is not movement"
    );
}
#[test]
fn the_notification_that_closes_the_shell_ends_the_poller() {
    let (_directory, output_path) = shell_output_file_holding(&hundred_numbered_lines());
    let mut reader = reader_tailing(SHELL_TASK_ID, &output_path);
    assert!(!no_poller_is_running(&reader));

    read_lines_into(
        &mut reader,
        &fixture_lines_numbered(SHELL_AND_CHECKLIST_FIXTURE, &[SHELL_NOTIFICATION_LINE]),
    );

    assert!(
        becomes_true_within(Duration::from_secs(2), || no_poller_is_running(&reader)),
        "the poller outlived the last running shell"
    );
}
#[test]
fn a_poller_whose_session_ended_returns_though_the_shell_still_runs() {
    let (_directory, output_path) = shell_output_file_holding(&hundred_numbered_lines());
    let reader = reader_tailing(SHELL_TASK_ID, &output_path);

    reader.activity.lock().unwrap().take();

    assert!(
        becomes_true_within(Duration::from_secs(2), || no_poller_is_running(&reader)),
        "the poller outlived the stdout reader that started it"
    );
    assert!(
        !running_shell_outputs(&reader.state).is_empty(),
        "the ledger still holds the running shell the poller walked away from"
    );
}
#[test]
fn a_deleted_output_file_is_skipped_and_the_next_read_replaces_the_tail() {
    let (_directory, output_path) = shell_output_file_holding(&hundred_numbered_lines());
    let reader = reader_tailing(SHELL_TASK_ID, &output_path);
    assert!(becomes_true_within(Duration::from_secs(3), || {
        !tail_of_the_shell(&reader, SHELL_TASK_ID).is_empty()
    }));
    let last_tail_before_the_delete = tail_of_the_shell(&reader, SHELL_TASK_ID);

    std::fs::remove_file(&output_path).expect("the output file is removed");
    assert!(
        super::super::activity::shells_left_to_tail(
            &reader.state,
            &reader.activity,
            &reader.shell_poller
        )
        .is_some(),
        "a missing file does not close the shell poller"
    );
    stop_shell_polling(&reader);
    super::super::activity::poll_shell_tails(
        &reader.state,
        &reader.revision,
        running_shell_outputs(&reader.state),
    );

    assert_eq!(
        tail_of_the_shell(&reader, SHELL_TASK_ID),
        last_tail_before_the_delete,
        "the last tail stands until a read succeeds"
    );

    std::fs::write(&output_path, "back again\n").expect("the output file returns");
    super::super::activity::poll_shell_tails(
        &reader.state,
        &reader.revision,
        running_shell_outputs(&reader.state),
    );
    assert_eq!(
        tail_of_the_shell(&reader, SHELL_TASK_ID),
        vec!["back again".to_string()],
        "the next successful read replaces the tail"
    );
}
#[test]
fn a_poller_that_walked_away_leaves_the_slot_empty_for_the_next_shell() {
    let (_directory, output_path) = shell_output_file_holding(&hundred_numbered_lines());
    let mut reader = reader_tailing(SHELL_TASK_ID, &output_path);

    read_lines_into(
        &mut reader,
        &fixture_lines_numbered(SHELL_AND_CHECKLIST_FIXTURE, &[SHELL_NOTIFICATION_LINE]),
    );

    assert!(
        becomes_true_within(Duration::from_secs(2), || no_poller_is_running(&reader)),
        "a poller with nothing left to tail clears its own slot before it returns"
    );
}
#[test]
fn a_second_shell_started_after_the_poller_left_starts_a_fresh_one() {
    let (_first_directory, first_path) = shell_output_file_holding(&hundred_numbered_lines());
    let (_second_directory, second_path) = shell_output_file_holding("second shell\n");
    let mut reader = reader_tailing(SHELL_TASK_ID, &first_path);
    let the_poller_that_left = polling_thread_of(&reader);
    read_lines_into(
        &mut reader,
        &fixture_lines_numbered(SHELL_AND_CHECKLIST_FIXTURE, &[SHELL_NOTIFICATION_LINE]),
    );
    assert!(becomes_true_within(Duration::from_secs(2), || {
        no_poller_is_running(&reader)
    }));

    read_lines_into(
        &mut reader,
        &a_background_shell_line_set("s2ndshell", "toolu_second_background_shell", &second_path),
    );

    assert_ne!(polling_thread_of(&reader), the_poller_that_left);
    assert!(!no_poller_is_running(&reader));
    assert!(
        becomes_true_within(Duration::from_secs(3), || {
            tail_of_the_shell(&reader, "s2ndshell") == vec!["second shell".to_string()]
        }),
        "the fresh poller tails the shell that started it"
    );
}
/// `Starting` is not a guess here. The child is forked long before it can
/// take a turn, and the init line is the moment it can — so the status the
/// PTY could never report is exactly what this session protocol reads off the wire.
#[test]
fn status_is_starting_until_the_session_reports_init() {
    let session = open(&stream_json_harness(&[RESULT]));
    assert_eq!(
        session.status(),
        AgentStatus::Starting,
        "a forked child that has said nothing is not waiting for anyone"
    );
    wait_for_status(&session, AgentStatus::Waiting);
    assert_eq!(
        session.session_id().as_deref(),
        Some("sess-adk"),
        "the id a resume is passed comes from the init line"
    );
    session.end();
}
/// `is_error` is the whole of what says a call failed, and the failure
/// travels as the outcome rather than as words in the summary.
#[tokio::test]
async fn a_failed_answer_reports_the_error_outcome_and_its_text() {
    let session = open(&stream_json_harness(&[TOOL_USE, ERROR_TOOL_RESULT, RESULT]));
    let mut activity = session.activity().expect("a reporting session");
    wait_for_status(&session, AgentStatus::Waiting);
    session.send_turn(&Turn::new("read the file")).unwrap();

    assert!(matches!(
        next_activity(&mut activity).await,
        AgentActivity::ToolUse { .. }
    ));
    assert_eq!(
        next_activity(&mut activity).await,
        AgentActivity::ToolResult {
            call_id: "toolu_1".to_string(),
            outcome: ToolOutcome::Error,
            summary: "File does not exist. Note: your current working directory is /work."
                .to_string()
        }
    );
    session.end();
}
/// The activity stream closing is what tells the daemon the session is over
/// — the death rites hang off it the way they hang off a PTY's EOF.
#[tokio::test]
async fn the_activity_stream_closes_when_the_child_does() {
    let session = open(&HarnessSpec::new("sh").arg("-c").arg("exit 0"));
    let mut activity = session.activity().expect("a reporting session");
    let closed = tokio::time::timeout(Duration::from_secs(5), activity.recv()).await;
    assert!(
        matches!(closed, Ok(Err(broadcast::error::RecvError::Closed))),
        "a subscriber must observe the close, not hang: {closed:?}"
    );
    session.end();
}
/// Step 11's live claim: a REAL background task, on the real wire. The
/// recorded fixtures pin what one probe emitted; this leg holds the shipped
/// reader to a fresh child — the task events still arrive on `system`, the
/// reader mints the started and finished rows, and the session reports
/// `Working` with `can_interrupt` false while its turn is closed and the
/// task lives, then `Waiting` once the roster empties.
///
/// Ignored by default for the same reason as the two legs above; run with
/// the same `cargo test --lib real_adk -- --ignored --nocapture`.
#[test]
#[ignore = "spawns the real claude binary; needs auth + network + a model turn"]
fn real_adk_session_reports_a_background_task_and_stays_working() {
    use crate::harness::Harness;

    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path().join("fresh-worktree");
    std::fs::create_dir_all(&workspace).unwrap();
    let mcp = workspace.join("mcp.json");
    std::fs::write(
        &mcp,
        serde_json::to_vec_pretty(&json!({ "mcpServers": {} })).unwrap(),
    )
    .unwrap();
    AdkHarness.prepare_workspace(&workspace);

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

    let (session, mut activity) = AdkSession::spawn(&spec, Some(workspace.clone()), &adk_choice())
        .expect("claude should spawn");
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    {
        let seen = std::sync::Arc::clone(&seen);
        std::thread::spawn(move || {
            while let Ok(event) = activity.blocking_recv() {
                let line = match &event.activity {
                    AgentActivity::Compaction { completed } => format!("compaction: {completed}"),
                    AgentActivity::Reasoning { summary } => format!("reasoning: {summary}"),
                    AgentActivity::ToolUse { summary, .. } => format!("tool_use: {summary}"),
                    AgentActivity::ToolResult {
                        outcome, summary, ..
                    } => format!("tool_result[{outcome:?}]: {summary}"),
                    AgentActivity::Narration { summary } => format!("narration: {summary}"),
                    AgentActivity::TaskUpdate { summary } => format!("task_update: {summary}"),
                };
                eprintln!("[activity] {line}");
                seen.lock().unwrap().push(line);
            }
        });
    }

    let wait_until = |what: &str, deadline: Duration, test: &dyn Fn() -> bool| {
        let started = Instant::now();
        while !test() {
            assert!(started.elapsed() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(100));
        }
    };
    // A task row names its work first and what happened to it after, and a
    // failed one carries the error behind that — so the ending is looked
    // for anywhere in the line, with the `task_update:` guard kept.
    let task_rows = |marker: &str| {
        seen.lock()
            .unwrap()
            .iter()
            .filter(|line| line.starts_with("task_update: ") && line.contains(marker))
            .count()
    };

    // A turn that puts a sleep in the BACKGROUND and answers without
    // waiting on it, so the turn's result closes over live work — the exact
    // shape the headless-looks-idle finding described.
    session
        .send_turn(&Turn::new(concat!(
            "You are being driven by an automated test. Do exactly this and nothing ",
            "else. Use the Bash tool with run_in_background set to true to run ",
            "exactly: sleep 15 && echo woke\n",
            "Do NOT wait for it, do NOT check on it, do NOT use any other tool. ",
            "Immediately after starting it, reply with a two-line haiku and stop.",
        )))
        .unwrap();
    wait_until("init", Duration::from_secs(30), &|| {
        !matches!(session.status(), AgentStatus::Starting)
    });
    wait_until("the started task row", Duration::from_secs(120), &|| {
        task_rows(" — started") == 1
    });

    // The result must close the turn while the task lives. The turn's edge
    // is read through the control tied to it: `can_interrupt` goes false
    // when the turn closes, while the live task holds status at `Working` —
    // the legal pair the digest pins, observed on the real wire.
    wait_until(
        "the turn to close over the live task",
        Duration::from_secs(120),
        &|| !session.can_interrupt(),
    );
    assert_eq!(
        session.status(),
        AgentStatus::Working,
        "the turn is closed and the sleep is not: this session is mid-work, not idle"
    );

    // The sleep ends; the roster empties; the reader closes the task in the
    // timeline and the session finally waits.
    wait_until(
        "the task to finish in the timeline",
        Duration::from_secs(120),
        &|| task_rows(" — finished") + task_rows(" — failed") >= 1,
    );
    wait_until(
        "the session to wait once the roster empties",
        Duration::from_secs(60),
        &|| matches!(session.status(), AgentStatus::Waiting),
    );
    eprintln!(
        "[verdict] task rows: {:?}",
        seen.lock()
            .unwrap()
            .iter()
            .filter(|line| line.starts_with("task_update:"))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        task_rows(" — started"),
        1,
        "one start, one row — however many events described it"
    );
    assert_eq!(
        task_rows(" — finished"),
        1,
        "the task completed, so its ending reads as finished, minted once"
    );
    session.end();
}

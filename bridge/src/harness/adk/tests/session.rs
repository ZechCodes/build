// Exact test bodies moved from the former inline test module.
use super::*;

#[test]
fn only_the_three_bad_endings_count_as_a_failure() {
    for failed in ["failed", "error", "timed_out"] {
        assert!(task_status_failed(failed), "{failed}");
        assert!(task_status_is_terminal(failed), "{failed}");
    }
    for ended in ["completed", "killed", "stopped", "cancelled", "canceled"] {
        assert!(!task_status_failed(ended), "{ended}");
        assert!(task_status_is_terminal(ended), "{ended}");
    }
}
#[test]
fn a_spawned_agents_tool_call_never_writes_the_sessions_surfaces() {
    let (mut reader, mut heard) = reader_and_what_it_reports();

    reader.read_line(&a_spawned_agents_todo_write_line());

    assert!(surfaces_of(&reader).unwrap().checklist.is_empty());
    assert_eq!(
        kinds_and_parents_of(&reports_already_sent(&mut heard)),
        vec![("tool_use", Some(SPAWNED_AGENT_CALL))],
        "and the row still folds under the call that spawned the agent"
    );
}
#[test]
fn a_spawned_agents_open_call_outlives_the_sessions_turn() {
    let (mut reader, mut heard) = reader_and_what_it_reports();

    reader.read_line(&a_spawned_agents_todo_write_line());
    reader.read_line(&json!({ "type": "result", "subtype": "success" }).to_string());

    assert_eq!(
        kinds_and_parents_of(&reports_already_sent(&mut heard)),
        vec![("tool_use", Some(SPAWNED_AGENT_CALL))],
        "the session's turn ending answers none of the spawned agent's calls"
    );
}
#[test]
fn a_session_that_read_only_the_workflow_fixture_writes_workflows_and_capability() {
    let surfaces = surfaces_of(&reader_over_every_line_of(WORKFLOW_FIXTURE))
        .expect("the workflow fixture leaves the session a snapshot");

    let written = surfaces.wire_value(&|_| None);
    let named: Vec<&str> = written
        .as_object()
        .expect("a snapshot writes an object")
        .keys()
        .map(String::as_str)
        .collect();

    assert_eq!(named, vec!["observations", "workflows"], "{written}");
    assert_eq!(written["observations"]["goal"]["support"], "unsupported");
}
#[test]
fn a_session_that_read_no_task_line_still_reports_goal_unsupported() {
    let mut reader = reader_over_a_silent_session();
    for line in fixture_lines(WORKFLOW_FIXTURE) {
        let event: Value = serde_json::from_str(&line).expect("the fixture is protocol");
        let announces = event["type"] == "system" && event["subtype"] == "init";
        if announces || event["type"] == "assistant" {
            reader.read_line(&line);
        }
    }

    let written = surfaces_of(&reader).unwrap().wire_value(&|_| None);
    assert_eq!(written["observations"]["goal"]["support"], "unsupported");
    assert_eq!(revision_counter_of(&reader), 0);
}
#[test]
fn a_turn_that_ended_between_a_create_and_its_answer_leaves_the_ledger_nothing_pending() {
    let mut reader = reader_over_a_silent_session();
    read_lines_into(
        &mut reader,
        &fixture_lines_numbered(SHELL_AND_CHECKLIST_FIXTURE, &[TASK_CREATE_CALL_LINE]),
    );
    assert_eq!(pending_create_count_of(&reader), 1);

    read_lines_into(
        &mut reader,
        &fixture_lines_numbered(SHELL_AND_CHECKLIST_FIXTURE, &[TURN_RESULT_LINE]),
    );

    assert_eq!(pending_create_count_of(&reader), 0);
}
#[test]
fn a_live_session_answers_with_the_snapshot_its_reader_built() {
    let session = open(&stream_json_harness(&[
        TASK_STARTED,
        TASK_NOTIFICATION,
        RESULT,
    ]));
    let mut moved = session
        .surfaces_changed()
        .expect("a session that reports its work watches its snapshot");
    session.send_turn(&Turn::new("run the job")).unwrap();
    wait_for_status(&session, AgentStatus::Waiting);

    let surfaces = session.surfaces().expect("the session read one shell");

    assert_eq!(surfaces.shells.len(), 1, "{surfaces:?}");
    assert_eq!(surfaces.shells[0].state.as_deref(), Some("done"));
    assert!(
        *moved.borrow_and_update() > 0,
        "the reader said the snapshot moved"
    );
    session.end();
}
#[test]
fn a_live_session_that_read_no_task_line_answers_with_capability_metadata() {
    let session = open(&stream_json_harness(&[THINKING, NARRATION, RESULT]));
    session.send_turn(&Turn::new("say something")).unwrap();
    wait_for_status(&session, AgentStatus::Waiting);

    let written = session.surfaces().unwrap().wire_value(&|_| None);
    assert_eq!(written["observations"]["goal"]["support"], "unsupported");
    assert!(
        session.surfaces_changed().is_some(),
        "the channel is offered even before anything moves"
    );
    session.end();
}
/// A conversation the last session NAMED is resumed by that name, and the
/// cwd guess is not passed beside it: `--resume` names the exact
/// conversation and `--continue` names the newest one in the directory, so
/// asking for both is asking for two different conversations.
#[test]
fn a_recorded_session_id_is_resumed_by_name_instead_of_by_the_cwd_guess() {
    let by_name = AdkHarness
        .spec(
            &ModelChoice::default(),
            &SpawnOptions {
                // Both offered, exactly as the daemon offers them: the probe
                // answers for every Build-owned checkout, and the record
                // answers for an agent that has run before.
                continue_session: true,
                resume_session_id: Some("sess-adk".to_string()),
                ..spawn_options()
            },
            &context(),
        )
        .unwrap();
    let args = by_name.args.join(" ");
    assert!(args.contains("--resume sess-adk"), "{args}");
    assert!(
        !args.contains("--continue"),
        "the name wins, and it wins alone: {args}"
    );

    // And nothing recorded leaves the shipped fallback exactly as it was:
    // an agent whose session died before announcing itself must not be a
    // spawn that fails.
    let by_guess = AdkHarness
        .spec(
            &ModelChoice::default(),
            &SpawnOptions {
                continue_session: true,
                resume_session_id: None,
                ..spawn_options()
            },
            &context(),
        )
        .unwrap();
    let args = by_guess.args.join(" ");
    assert!(args.contains("--continue"), "{args}");
    assert!(!args.contains("--resume"), "{args}");
}
#[test]
fn a_reporting_session_matches_its_harness_capability() {
    let session = open(&stream_json_harness(&[RESULT]));
    assert_eq!(session.terminal().is_some(), AdkHarness.has_terminal());
    assert_eq!(session.activity().is_some(), !AdkHarness.has_terminal());
    session.end();
}
#[test]
fn the_init_line_names_the_model_the_session_is_running() {
    let session = open(&stream_json_harness(&[RESULT]));
    assert_eq!(
        session.active_model(),
        None,
        "a child that has said nothing is running nothing Build knows of"
    );
    wait_for_status(&session, AgentStatus::Waiting);
    assert_eq!(
        session.active_model().as_deref(),
        Some("claude-fable-5-1"),
        "the model the button names comes from the init line"
    );
    session.end();
}
/// The turn boundary the whole session protocol exists for: a model reasoning in
/// silence is still working, and only its result line says otherwise.
#[test]
fn a_turn_is_working_until_its_result_line_arrives() {
    // The child cannot answer until the test opens this gate. Its silence is
    // the state under test, independent of scheduler speed.
    let directory = tempfile::tempdir().unwrap();
    let release = directory.path().join("release");
    let session = open(&HarnessSpec::new("sh").arg("-c").arg(format!(
            "printf '%s\\n' '{INIT}'\nwhile IFS= read -r turn; do while [ -d '{}' ] && [ ! -e '{}' ]; do sleep 0.01; done; printf '%s\\n' '{RESULT}'; done\n",
            directory.path().display(), release.display()
        )));
    wait_for_status(&session, AgentStatus::Waiting);

    session
        .send_turn(&Turn::new("drop the index"))
        .expect("the turn is written");
    assert_eq!(
        session.status(),
        AgentStatus::Working,
        "the turn started the moment it was accepted"
    );
    assert_eq!(
        session.status(),
        AgentStatus::Working,
        "a silent model mid-turn is working, not waiting for the human"
    );

    std::fs::write(&release, "go").unwrap();
    wait_for_status(&session, AgentStatus::Waiting);
    session.end();
}
#[tokio::test]
async fn a_silent_result_publishes_the_completed_turn_boundary() {
    let session = open(&HarnessSpec::new("sh").arg("-c").arg(format!(
            "printf '%s\\n' '{INIT}'\nwhile IFS= read -r turn; do sleep 0.1; printf '%s\\n' '{RESULT}'; done\n"
        )));
    let mut changed = session
        .status_changed()
        .expect("a protocol session publishes exact status changes");
    wait_for_status(&session, AgentStatus::Waiting);
    changed.borrow_and_update();

    session
        .send_turn(&Turn::new("work without output"))
        .expect("the turn is written");
    assert_eq!(changed.borrow_and_update().status, AgentStatus::Working);

    let completed = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            changed
                .changed()
                .await
                .expect("the status channel remains open");
            let snapshot = changed.borrow_and_update().clone();
            if snapshot.status == AgentStatus::Waiting {
                break snapshot;
            }
        }
    })
    .await
    .expect("the silent result publishes promptly");
    assert!(
        completed.last_worked_at.is_some(),
        "the cumulative snapshot remembers the completed turn even if later updates coalesce"
    );
    session.end();
}
/// A turn handed over mid-turn is absorbed by the one already running, and
/// the child answers both with a single result. The session has to end that
/// turn on it: a session that kept waiting for a second result would report
/// `Working` forever, and `Working` is what stops the idle sweep from ever
/// explaining an agent that quietly stopped.
#[test]
fn one_result_ends_the_turn_it_answers_however_many_were_handed_over() {
    // Two turns in, one result out, then silence — the shape of a message
    // written while the model was still working.
    let session = open(&HarnessSpec::new("sh").arg("-c").arg(format!(
            "printf '%s\\n' '{INIT}'; read -r first; read -r second; printf '%s\\n' '{RESULT}'; cat >/dev/null"
        )));
    wait_for_status(&session, AgentStatus::Waiting);

    session.send_turn(&Turn::new("drop the index")).unwrap();
    session.send_turn(&Turn::new("and the trigger")).unwrap();
    assert_eq!(session.status(), AgentStatus::Working);

    wait_for_status(&session, AgentStatus::Waiting);
    session.end();
}
/// The turn boundary drains the map. A call the interrupted turn left open
/// is closed as unanswered — no answer ever arrived and none is coming —
/// and the next turn's call pairs into a row of its own, which is what
/// proves the drain emptied the map rather than leaving the id behind.
#[tokio::test]
async fn a_result_closes_every_call_its_turn_left_open() {
    let session = open(&stream_json_harness_turn_by_turn(&[
        &[TOOL_USE, FAILED_RESULT],
        &[SECOND_TOOL_USE, SECOND_TOOL_RESULT, RESULT],
    ]));
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
            outcome: ToolOutcome::Unanswered,
            summary: String::new()
        },
        "the turn ended over the call, and nothing is fabricated about it"
    );

    wait_for_status(&session, AgentStatus::Waiting);
    session.send_turn(&Turn::new("try the other one")).unwrap();
    assert!(matches!(
        next_activity(&mut activity).await,
        AgentActivity::ToolUse { .. }
    ));
    assert_eq!(
        next_activity(&mut activity).await,
        AgentActivity::ToolResult {
            call_id: "toolu_2".to_string(),
            outcome: ToolOutcome::Ok,
            summary: "pub struct Thread".to_string()
        },
        "and the next turn starts against an empty map"
    );
    session.end();
}
/// The row a background task mints when it starts, and the whole reason
/// this step exists: the turn that started the work is over and the work is
/// not, so a session with nothing open is still `Working`.
///
/// The `background_tasks_changed` roster that follows lists the same task,
/// and mints NOTHING — one row per transition, however many events describe
/// it. The narration behind it is the fence that proves so: if the roster
/// had minted, it would be sitting where the narration is.
#[tokio::test]
async fn a_started_task_mints_once_and_keeps_a_turnless_session_working() {
    let session = open(&stream_json_harness(&[
        TASK_STARTED,
        TASK_ROSTER,
        RESULT,
        NARRATION,
    ]));
    let mut activity = session.activity().expect("a reporting session");
    wait_for_status(&session, AgentStatus::Waiting);
    session.send_turn(&Turn::new("run the reindex")).unwrap();

    assert_eq!(
        next_activity(&mut activity).await,
        AgentActivity::TaskUpdate {
            summary: format!("{TASK_DESCRIPTION} — started"),
        }
    );
    assert_eq!(
        next_activity(&mut activity).await,
        AgentActivity::Narration {
            summary: "dropped the index".to_string()
        },
        "the roster listing a task already live moved nothing, so it minted nothing"
    );

    // The narration arrived after the result, so the turn is closed — said
    // through the control the composer reads, which is tied to an open turn
    // and to nothing else.
    assert!(
        !session.can_interrupt(),
        "a background task is not a turn, and the interrupt stops a turn"
    );
    assert_eq!(
        session.status(),
        AgentStatus::Working,
        "the work outlived the turn that started it"
    );
    session.end();
}
/// The roster is the source of truth, in both directions: a task it stops
/// listing is over, whether or not an event ever said so. The timeline never
/// shows work that started and never ended, and the set clears rather than
/// pinning `Working` forever.
#[tokio::test]
async fn a_roster_that_drops_a_task_closes_it_once_and_the_session_waits_again() {
    let session = open(&stream_json_harness(&[
        TASK_STARTED,
        RESULT,
        TASK_ROSTER_EMPTY,
        NARRATION,
    ]));
    let mut activity = session.activity().expect("a reporting session");
    wait_for_status(&session, AgentStatus::Waiting);
    session.send_turn(&Turn::new("run the reindex")).unwrap();

    assert_eq!(
        next_activity(&mut activity).await,
        AgentActivity::TaskUpdate {
            summary: format!("{TASK_DESCRIPTION} — started"),
        }
    );
    assert_eq!(
        next_activity(&mut activity).await,
        AgentActivity::TaskUpdate {
            summary: format!("{TASK_DESCRIPTION} — finished"),
        },
        "a roster that quietly drops a task still closes it in the timeline"
    );
    assert_eq!(
        next_activity(&mut activity).await,
        AgentActivity::Narration {
            summary: "dropped the index".to_string()
        },
        "and closes it exactly once"
    );

    wait_for_status(&session, AgentStatus::Waiting);
    session.end();
}
/// A FOREGROUND Bash command, replayed in the order the live child emitted
/// it: `task_started`, then a `task_notification` carrying a terminal
/// status — and no roster and no `task_updated`, because the child sends
/// neither for one.
///
/// So the notification is the only event that says the work is over, and it
/// has to close the task: a reader that took it for chatter would hold the
/// task for the life of the session and report `Working` over an agent that
/// has been idle for hours — the inverse of the failure this step exists to
/// close. The `Waiting` at the end is that regression's fence.
///
/// Its text mints nothing of its own here because a foreground
/// notification's summary IS the task's description, and a row reading
/// `Sleep for 10 seconds: Sleep for 10 seconds` says nothing the ending row
/// did not.
#[tokio::test]
async fn a_foreground_tasks_notification_closes_it_and_the_session_waits_again() {
    let session = open(&stream_json_harness(&[
        FOREGROUND_TASK_STARTED,
        FOREGROUND_TASK_NOTIFICATION,
        RESULT,
        NARRATION,
    ]));
    let mut activity = session.activity().expect("a reporting session");
    wait_for_status(&session, AgentStatus::Waiting);
    session
        .send_turn(&Turn::new("sleep for ten seconds"))
        .unwrap();

    let mut minted = Vec::new();
    for _ in 0..3 {
        minted.push(next_activity(&mut activity).await);
    }
    assert_eq!(
        minted,
        vec![
            AgentActivity::TaskUpdate {
                summary: format!("{FOREGROUND_TASK_DESCRIPTION} — started"),
            },
            AgentActivity::TaskUpdate {
                summary: format!("{FOREGROUND_TASK_DESCRIPTION} — finished"),
            },
            AgentActivity::Narration {
                summary: "dropped the index".to_string()
            },
        ]
    );

    wait_for_status(&session, AgentStatus::Waiting);
    session.end();
}
/// The epitaph is REPORTED, never scraped: the last error the child said
/// out loud is what explains the crash, because there is no screen to read.
#[test]
fn the_epitaph_is_the_error_the_session_reported() {
    let session = open(&stream_json_harness(&[FAILED_RESULT]));
    wait_for_status(&session, AgentStatus::Waiting);
    session.send_turn(&Turn::new("try it")).unwrap();
    wait_for_status(&session, AgentStatus::Waiting);

    becomes_true_within(Duration::from_secs(5), || session.epitaph().is_some());
    assert_eq!(
        session.epitaph().as_deref(),
        Some("the tool call was refused")
    );
    session.end();
}
/// A child that dies mid-turn never reports a result, so its last words are
/// whatever it managed to say on stderr — the only surface left.
#[test]
fn a_child_that_dies_mid_turn_leaves_what_it_said_on_stderr() {
    let session = open(&HarnessSpec::new("sh").arg("-c").arg(format!(
        "printf '%s\\n' '{INIT}'; read -r turn; echo 'API Error: overloaded_error' >&2; exit 7"
    )));
    wait_for_status(&session, AgentStatus::Waiting);
    session.send_turn(&Turn::new("go")).unwrap();

    assert!(
        session.exited_within(Duration::from_secs(5)),
        "the child died mid-turn and never came back"
    );
    assert_eq!(session.status(), AgentStatus::Ended { code: Some(7) });
    assert_eq!(
        session.epitaph().as_deref(),
        Some("API Error: overloaded_error")
    );
    session.end();
}
/// The reap lag, from the terminal's test: a dying child closes its
/// pipes before its exit status is reapable, so the caller deciding whether
/// a failed write means "crashed" rather than "wedged" waits it out here.
#[test]
fn exited_within_bridges_the_gap_until_the_exit_is_reapable() {
    let session = open(&HarnessSpec::new("sh").arg("-c").arg("sleep 0.15"));
    assert_eq!(
        session.status(),
        AgentStatus::Starting,
        "the child is still running, so nothing has ended"
    );
    assert!(
        session.exited_within(Duration::from_secs(5)),
        "the wait must outlast the lag between the pipes closing and the reap"
    );
    assert_eq!(session.status(), AgentStatus::Ended { code: Some(0) });
}
/// And a harness that keeps running is not declared dead by waiting.
#[test]
fn exited_within_gives_up_on_a_harness_that_keeps_running() {
    let session = open(&stream_json_harness(&[RESULT]));
    assert!(!session.exited_within(Duration::from_millis(50)));
    session.end();
}
/// Ending reaps: killing without collecting the status leaks one zombie per
/// session on a daemon that never restarts.
#[test]
fn ending_a_session_reaps_the_child() {
    let session = open(&stream_json_harness(&[RESULT]));
    wait_for_status(&session, AgentStatus::Waiting);
    session.end();
    assert!(
        matches!(session.status(), AgentStatus::Ended { .. }),
        "the child is gone the moment the session ends"
    );
}
/// `send_turn` returns on the WRITE. The daemon's in-place nudge speaks from
/// under the app-wide state lock, so a session that waited on the model here
/// would stall every RPC, every pump and the idle sweep with it.
#[test]
fn send_turn_returns_on_the_write_even_when_the_child_never_answers() {
    let directory = tempfile::tempdir().unwrap();
    let release = directory.path().join("release");
    let session = open(
        &HarnessSpec::new("sh")
            .arg("-c")
            .arg(format!(
                "printf '%s\\n' '{INIT}'; while IFS= read -r turn; do while [ -d '{}' ] && [ ! -e '{}' ]; do sleep 0.01; done; printf '%s\\n' '{RESULT}'; done",
                directory.path().display(), release.display()
            )),
    );
    wait_for_status(&session, AgentStatus::Waiting);

    session
        .send_turn(&Turn::new("this turn is not answered yet"))
        .expect("the write returns before the child can answer");
    assert_eq!(
        session.status(),
        AgentStatus::Working,
        "an unanswered turn is still a turn in progress"
    );
    std::fs::write(&release, "go").unwrap();
    wait_for_status(&session, AgentStatus::Waiting);
    session.end();
}
/// The capability is the CHILD's answer, not the provider's: the same CLI
/// advertises an interrupt on one version and not on the next, so the
/// question is asked of the `init` line rather than of a version number.
/// And it is offered only while there is a turn to spend it on, because
/// what an interrupt ends is a turn.
///
/// The equivalence between the flag and the call runs one way and is
/// asserted as such: a refusal implies the flag is false, and a false flag
/// with a turn open implies a refusal. Between turns the flag is false and
/// the call is the satisfied no-op — the turn the human meant to stop is
/// already over, which is an answer rather than an error.
#[test]
fn stopping_a_turn_is_offered_exactly_when_the_child_announced_it_and_a_turn_is_open() {
    let announced = open(&stream_json_harness(&[THINKING]));
    assert!(
        !announced.can_interrupt(),
        "a child that has said nothing has announced nothing"
    );
    assert!(
        matches!(announced.interrupt(), Err(HarnessError::Unsupported(_))),
        "and a refusal always means the flag was false"
    );

    wait_for_status(&announced, AgentStatus::Waiting);
    assert!(
        !announced.can_interrupt(),
        "announced, but idle at its prompt: there is no turn to stop"
    );
    assert!(
        announced.interrupt().is_ok(),
        "and the press that lands there is satisfied, not refused"
    );

    // A turn the child never answers, so it is still open to be stopped.
    announced.send_turn(&Turn::new("drop the index")).unwrap();
    assert!(announced.can_interrupt());
    assert!(announced.interrupt().is_ok());
    announced.end();

    let silent = open(&stream_json_harness_without_interrupt(&[THINKING]));
    wait_for_status(&silent, AgentStatus::Waiting);
    silent.send_turn(&Turn::new("drop the index")).unwrap();
    assert!(
        !silent.can_interrupt(),
        "a turn is open, so a false flag here is the session's refusal"
    );
    assert!(matches!(
        silent.interrupt(),
        Err(HarnessError::Unsupported(_))
    ));
    silent.end();
}
/// What an interrupt IS on the wire: one `control_request` line down the
/// same pipe the turns go down, carrying a fresh id per ask — which is what
/// lets the reader tell this session's ack from somebody else's.
#[test]
fn an_interrupt_is_one_control_request_line_with_an_id_of_its_own() {
    let dir = tempfile::tempdir().expect("temp dir");
    let capture = dir.path().join("stdin.jsonl");
    let session = open(&HarnessSpec::new("sh").arg("-c").arg(format!(
            "printf '%s\\n' '{INIT}'\nwhile IFS= read -r line; do printf '%s\\n' \"$line\" >> {}; done\n",
            capture.display()
        )));
    wait_for_status(&session, AgentStatus::Waiting);

    // A turn to stop: this child never answers one, so it stays open, and
    // an ask only travels while a turn is open.
    session.send_turn(&Turn::new("rewrite everything")).unwrap();
    session.interrupt().expect("this child advertised one");
    session.interrupt().expect("asking twice is allowed");

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut written = Vec::new();
    while Instant::now() < deadline && written.len() < 3 {
        written = std::fs::read_to_string(&capture)
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).expect("a protocol line"))
            .collect();
        std::thread::sleep(Duration::from_millis(5));
    }

    assert_eq!(
        written.len(),
        3,
        "the turn, then one line per ask: {written:?}"
    );
    assert_eq!(written[0]["type"], "user", "{written:?}");
    let written = &written[1..];
    for asked in written {
        assert_eq!(asked["type"], "control_request");
        assert_eq!(asked["request"]["subtype"], "interrupt");
        assert!(
            asked["request_id"]
                .as_str()
                .is_some_and(|id| !id.is_empty()),
            "an ack can only be matched to a request that named itself: {asked}"
        );
    }
    assert_ne!(
        written[0]["request_id"], written[1]["request_id"],
        "a fresh id per request, so one ask's ack cannot close another's"
    );
    session.end();
}
/// A CLI built before the interrupt landed refuses, says what to do
/// instead — the probes showed an ordinary message reaches the running turn
/// at its next step boundary — and is the SAME session afterwards. A
/// refusal is not a kill.
#[test]
fn a_child_that_advertises_no_interrupt_refuses_and_keeps_working() {
    let session = open(&stream_json_harness_without_interrupt(&[THINKING]));
    wait_for_status(&session, AgentStatus::Waiting);

    let refused = session
        .interrupt()
        .expect_err("this child cannot be stopped");
    let said = refused.to_string();
    assert!(
        said.contains("interrupt") && said.contains("message"),
        "the refusal names what is missing and where the human's words still land: {said}"
    );
    assert!(
        !said.contains("Esc"),
        "and never sends anyone to a terminal this session does not have: {said}"
    );

    session
        .send_turn(&Turn::new("carry on then"))
        .expect("the refusal left the session alive");
    assert_eq!(session.status(), AgentStatus::Working);
    session.end();
}
/// The rule the whole step exists to hold: a turn the human stopped leaves
/// no epitaph. `error_during_execution` is what an interrupted turn's
/// result carries, and reported as a crash it would end the human's own
/// stop with a crash notice quoting it.
#[test]
fn an_acked_interrupt_leaves_the_turn_it_stopped_no_epitaph() {
    let session = open(&stream_json_harness(&[THINKING]));
    wait_for_status(&session, AgentStatus::Waiting);
    session.send_turn(&Turn::new("rewrite everything")).unwrap();
    wait_for_status(&session, AgentStatus::Working);

    session.interrupt().expect("this child advertised one");
    wait_for_status(&session, AgentStatus::Waiting);

    assert_eq!(
        session.epitaph(),
        None,
        "the human stopped it — there is nothing to explain"
    );
    session.end();
}
/// The other two sides of the same equivalence, so the clearing can never
/// be a blanket amnesty on `error_during_execution`.
///
/// The ack is what makes an interrupt one the child ACTED on: it answers
/// the control request before it emits the result, so an interrupt still
/// unanswered at the result is one the child never acted on, and the
/// failure the result reports is the turn's own. A `control_response`
/// carrying somebody else's request id is noise, and a session that treated
/// it as its own would swallow a real crash.
#[test]
fn an_interrupt_the_child_never_answered_leaves_the_turns_own_error() {
    let session = open(&stream_json_harness_answering_another_request(&[THINKING]));
    wait_for_status(&session, AgentStatus::Waiting);
    session.send_turn(&Turn::new("rewrite everything")).unwrap();
    wait_for_status(&session, AgentStatus::Working);

    session.interrupt().expect("this child advertised one");
    wait_for_status(&session, AgentStatus::Waiting);

    assert_eq!(
        session.epitaph().as_deref(),
        Some("the tool call was refused"),
        "an interrupt the child never acted on does not excuse the turn's own failure"
    );
    session.end();
}
/// A press that lands after the turn it meant to stop has already ended.
///
/// The control is offered off a digest up to 1.6s old, so a result can land
/// inside that window — or race the ask by milliseconds. The rule is the
/// one the take-on-result holds: an interrupt can never leak into the turn
/// AFTER it, and taking the pending on a result only holds that when a
/// result intervenes. Recorded against a turn already closed, the pending
/// would be marked `steered` by the send that follows, and that turn's OWN
/// result would then hand `Working` to nothing — a session reporting
/// `Working` with nothing running, which the idle sweep will not demote,
/// since it demotes only what is not working — while clearing that turn's
/// own error from the epitaph.
#[test]
fn a_press_that_lands_between_turns_leaves_the_next_turn_alone() {
    let dir = tempfile::tempdir().expect("temp dir");
    let heard = dir.path().join("heard.jsonl");
    let session = open(&stream_json_harness_with_nothing_to_stop(
        &[FAILED_RESULT],
        &heard,
    ));
    wait_for_status(&session, AgentStatus::Waiting);

    // A turn runs and its result closes it — the window the press lands in.
    session.send_turn(&Turn::new("drop the index")).unwrap();
    wait_for_status(&session, AgentStatus::Waiting);

    // The human presses stop on that turn, a moment too late, and the
    // message rides along the way the composer sends it.
    session.interrupt().expect("this child advertised one");
    session.send_turn(&Turn::new("try the other file")).unwrap();

    // The fake may answer before send_turn returns. Its result must still
    // close this turn, including when it arrives immediately.
    wait_for_status(&session, AgentStatus::Waiting);
    assert_eq!(
        session.epitaph().as_deref(),
        Some("the tool call was refused"),
        "the new turn failed on its own account; no interrupt of an older turn excuses it"
    );

    // And the child was never told to stop a turn it had already finished.
    // Not merely tidy: the live CLI announces `interrupt_cancel_queued_v1`,
    // so a `control_request` sent with nothing running is a request that
    // could take the queued turn with it.
    let kinds: Vec<String> = std::fs::read_to_string(&heard)
        .expect("the child kept what it heard")
        .lines()
        .map(|line| {
            serde_json::from_str::<Value>(line).expect("a protocol line")["type"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    assert_eq!(
        kinds,
        vec!["user", "user"],
        "a press with no turn open is not spoken to the child at all"
    );
    session.end();
}
/// The subtler half of the rule: the result that closes an interrupted turn
/// hands `Working` on to the turn queued behind it.
///
/// Clearing the flag on that result would leave a session that is actively
/// running the steering turn reporting `Waiting` — and a steering turn is
/// exactly the kind that goes silent for minutes inside one tool call, so
/// the idle sweep would demote a working agent. There is no second result
/// to reopen it: the steering turn ends in its own single result.
#[tokio::test]
async fn a_steering_turn_behind_an_interrupt_keeps_the_session_working() {
    let session = open(&stream_json_harness(&[THINKING]));
    let mut activity = session.activity().expect("a reporting session");
    wait_for_status(&session, AgentStatus::Waiting);

    session.send_turn(&Turn::new("rewrite everything")).unwrap();
    next_activity(&mut activity).await;

    // In the order the daemon's steering flow speaks them, both from under
    // the state lock: stop the turn, then hand over the message.
    session.interrupt().expect("this child advertised one");
    session
        .send_turn(&Turn::new("actually, just the index"))
        .unwrap();

    // The child answers the interrupt, ends the stopped turn with
    // `error_during_execution`, and only then reads the steering turn — so
    // this second event can only arrive after that result was read.
    next_activity(&mut activity).await;
    assert_eq!(
        session.status(),
        AgentStatus::Working,
        "the steered turn is running; a session reporting Waiting here would be swept"
    );
    assert_eq!(session.epitaph(), None, "and the stop left no epitaph");
    session.end();
}
/// The quiet clock is the minutes-scale anomaly instrument the idle sweep
/// demotes on, and for a session protocol it reads protocol lines rather than
/// paint — the last thing the session actually said.
#[test]
fn quiet_for_reads_the_age_of_the_last_protocol_line() {
    let session = open(&stream_json_harness(&[RESULT]));
    wait_for_status(&session, AgentStatus::Waiting);
    assert!(
        session.quiet_for() < Duration::from_secs(1),
        "the init line just arrived"
    );

    session.backdate_last_output(Duration::from_secs(600));
    assert!(
        session.quiet_for() >= Duration::from_secs(600),
        "a session that has said nothing for ten minutes must report it"
    );
    session.end();
}
/// The one claim the fake cannot prove: the live protocol's mid-turn
/// semantics. Spawns the REAL `claude` binary with the REAL argv shape and
/// hands it a turn that runs a slow tool; while that tool runs, a second
/// turn is written. If streaming input delivers at the next step boundary
/// — the way the interactive TUI queues a message typed mid-run — the
/// agent's final answer obeys the follow-up inside the same turn. If the
/// follow-up instead waits for the first result, a second Working phase
/// appears and the answer still converges, but the printout says which
/// world we are in.
///
/// Ignored by default: it needs `claude` installed, authenticated, and a
/// real (small, haiku) model turn. Run by hand:
///
/// ```text
/// cargo test --lib real_adk -- --ignored --nocapture
/// ```
#[test]
#[ignore = "spawns the real claude binary; needs auth + network + a model turn"]
fn real_adk_session_steers_mid_turn() {
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

    // The real CLI announces itself only after the first stdin message
    // arrives (verified against 2.1.236), so the turn is written first and
    // init is awaited after — the same order the daemon's deliver uses.
    session
        .send_turn(&Turn::new(concat!(
            "You are being driven by an automated test. Do exactly this and nothing ",
            "else, then stop. First, use the Bash tool to run exactly: sleep 10\n",
            "After the sleep finishes, write a file named answer.txt in the current ",
            "directory whose entire contents are exactly the single word: APPLE",
        )))
        .unwrap();
    wait_until("init", Duration::from_secs(30), &|| {
        !matches!(session.status(), AgentStatus::Starting)
    });
    wait_until("the sleep tool to start", Duration::from_secs(90), &|| {
        seen.lock()
            .unwrap()
            .iter()
            .any(|line| line.starts_with("tool_use") && line.contains("sleep"))
    });

    // Not immediately on the tool_use line: the probes show a message
    // written within ~100ms of the tool_use event can be lost in the CLI's
    // loop transition, while one written seconds later — any human
    // follow-up — is delivered at the next step boundary.
    std::thread::sleep(Duration::from_secs(3));
    let steered_at = Instant::now();
    session
        .send_turn(&Turn::new(concat!(
            "Change of plan: answer.txt must contain exactly the single word BANANA ",
            "instead of APPLE. This message supersedes the previous instruction.",
        )))
        .unwrap();
    eprintln!("[steer] follow-up written while the sleep tool runs");

    wait_until("the turn to end", Duration::from_secs(240), &|| {
        matches!(session.status(), AgentStatus::Waiting)
    });
    let first_result_after = steered_at.elapsed();

    // A second Working phase here would mean the follow-up was NOT absorbed
    // into the running turn and ran as its own turn after the first result.
    let mut second_turn = false;
    let settled = Instant::now();
    while settled.elapsed() < Duration::from_secs(20) {
        if matches!(session.status(), AgentStatus::Working) {
            second_turn = true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    wait_until("any second turn to end", Duration::from_secs(240), &|| {
        matches!(session.status(), AgentStatus::Waiting)
    });

    let answer = std::fs::read_to_string(workspace.join("answer.txt"))
        .expect("the agent should have written answer.txt");
    eprintln!(
        "[verdict] answer.txt = {:?}; first result {}ms after steering; second turn: {}",
        answer.trim(),
        first_result_after.as_millis(),
        second_turn,
    );
    assert_eq!(
        answer.trim(),
        "BANANA",
        "the mid-turn follow-up must decide the answer (same turn or the very next)"
    );
    assert!(
            !second_turn,
            "the follow-up ran as a separate turn after the first result — mid-turn steering does NOT reach the running loop; the spec's claim needs revising"
        );
    session.end();
}
/// The other live claim: the native interrupt, against the REAL wire. While
/// a real turn sits inside a slow tool, `interrupt()` writes the
/// `control_request`; the live child must ack it, close the stopped turn
/// with `error_during_execution`, and run the steering turn queued behind
/// the interrupt in the SAME session. Three things only the real binary can
/// prove: the ack arrives (the epitaph clearing hangs on it — an unacked
/// interrupt would leave `error_during_execution` reading as a crash), the
/// stopped tool never finishes, and the conversation survives its own stop.
///
/// The parked tool is a python sleep rather than a plain `sleep 90`
/// because the installed CLI BLOCKS a standalone sleep outright — "Blocked:
/// standalone sleep 90 … use run_in_background" — and the model, told no,
/// obligingly reruns it in the background, where nothing is parked at all
/// and the turn ends immediately. This command the CLI runs in the
/// foreground, timeout and all, which is what parks the turn (verified on
/// the wire, 2026-08-30: tool call at 9s, still running at 12s, and the
/// interrupt cut it there).
///
/// Ignored by default for the same reason as the steering test above; run
/// with the same `cargo test --lib real_adk -- --ignored --nocapture`.
#[test]
#[ignore = "spawns the real claude binary; needs auth + network + a model turn"]
fn real_adk_session_interrupts_mid_tool() {
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

    // A turn that parks inside a tool long enough to be stopped: if the
    // interrupt were silently ignored, this command runs its full ninety
    // seconds and the deadline math below catches it.
    session
        .send_turn(&Turn::new(concat!(
            "You are being driven by an automated test. Do exactly this and nothing ",
            "else, then stop. First, use the Bash tool to run exactly this command ",
            "in the FOREGROUND, and do NOT set run_in_background: ",
            "python3 -c \"import time; time.sleep(90)\"\n",
            "Set the tool timeout to 150000. After it finishes, write a file named ",
            "answer.txt in the current directory whose entire contents are exactly ",
            "the single word: APPLE",
        )))
        .unwrap();
    // The turn just written holds status at `Working`, so init's arrival is
    // observed through the capability it carries: `can_interrupt()` turns
    // true the moment the child's own line announces
    // `interrupt_receipt_v1`. A timeout here means the live CLI stopped
    // advertising it — and then this leg proves nothing.
    wait_until(
        "the child to announce its interrupt",
        Duration::from_secs(30),
        &|| session.can_interrupt(),
    );
    wait_until("the sleep tool to start", Duration::from_secs(90), &|| {
        seen.lock()
            .unwrap()
            .iter()
            .any(|line| line.starts_with("tool_use") && line.contains("time.sleep"))
    });

    // The same berth the steering test gives the CLI's loop transition.
    std::thread::sleep(Duration::from_secs(3));
    let interrupted_at = Instant::now();
    session.interrupt().expect("the child advertised one");
    // The daemon's steering order, from `nudge_live_agent_tab`: stop, then
    // hand over. The steering message names nothing the first one did not,
    // so the right file appearing is also proof the conversation survived.
    session
        .send_turn(&Turn::new(concat!(
            "You were interrupted on purpose; that is expected. Do not sleep again. ",
            "Write the same file the first message named, but its entire contents ",
            "must be exactly the single word: CHERRY. Then stop.",
        )))
        .unwrap();
    eprintln!("[interrupt] control_request written mid-sleep, steering turn queued behind it");

    // Status must hold `Working` across the interrupted turn's result — the
    // steered hand-off — so `Waiting` here means the steering turn ended in
    // its own result.
    wait_until(
        "the steering turn to end",
        Duration::from_secs(240),
        &|| matches!(session.status(), AgentStatus::Waiting),
    );
    let settled_after = interrupted_at.elapsed();

    let answer = std::fs::read_to_string(workspace.join("answer.txt"))
        .expect("the steering turn should have written answer.txt");
    eprintln!(
        "[verdict] answer.txt = {:?}; settled {}ms after the interrupt; epitaph: {:?}",
        answer.trim(),
        settled_after.as_millis(),
        session.epitaph(),
    );
    // The stopped command still had some eighty-seven of its ninety seconds
    // to run, so an interrupt the child ignored cannot settle inside this
    // window however fast the steering turn is. On the wire the whole
    // stop-and-steer takes about five seconds.
    assert!(
            settled_after < Duration::from_secs(60),
            "the whole stop-and-steer took {}ms — the parked command had eighty-seven seconds left, so the turn was never stopped",
            settled_after.as_millis()
        );
    assert_eq!(
        answer.trim(),
        "CHERRY",
        "the steering turn must decide the file, in the same conversation"
    );
    assert_eq!(
            session.epitaph(),
            None,
            "the human stopped it — an epitaph here means the ack was missed and error_during_execution read as a crash"
        );
    assert!(
        !matches!(session.status(), AgentStatus::Ended { .. }),
        "an interrupted session is the same session, still alive"
    );
    // Step 12's half of this leg: the parked call must not outlive the
    // turn the interrupt ended — every call the session made closes. On
    // the live wire (claude 2.1.x, observed 2026-08-30) the CLI answers
    // the interrupted call ITSELF, with an `is_error` rejection ("The user
    // doesn't want to proceed…"), before the `error_during_execution`
    // result — so the boundary drain finds the map already empty. The
    // drain stays as the net beneath a wire that does not answer (a
    // crashed child, an older CLI), pinned by the fake in
    // `a_result_closes_every_call_its_turn_left_open`; what the live leg
    // holds is the invariant both paths serve: one completion per call,
    // and the interrupted call's completion is terminal — `Error` from the
    // CLI's own rejection, or `Unanswered` from the drain.
    let lines = seen.lock().unwrap().clone();
    let calls = lines
        .iter()
        .filter(|line| line.starts_with("tool_use:"))
        .count();
    let completions = lines
        .iter()
        .filter(|line| line.starts_with("tool_result["))
        .count();
    assert_eq!(
        completions, calls,
        "every call closes at its turn's boundary, the interrupted one included: {lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|line| line.starts_with("tool_result[Error]")
                || line.starts_with("tool_result[Unanswered]")),
        "the interrupted call's completion is terminal, never a fabricated success: {lines:?}"
    );
    session.end();
}

/// A child that records what it is told and acks every control request
/// with a success naming the request it was asked — and emits no result
/// for it, because a `set_model` ends no turn. Announces itself only once
/// the first line arrives, the way the live CLI does.
fn recording_child(capture: &Path, init: &str) -> HarnessSpec {
    HarnessSpec::new("sh").arg("-c").arg(format!(
        "announced=0\nwhile IFS= read -r line; do\n\
         printf '%s\\n' \"$line\" >> {capture}\n\
         case \"$line\" in\n\
         *control_request*)\n\
         asked=$(printf '%s' \"$line\" | sed -n 's/.*\"request_id\":\"\\([^\"]*\\)\".*/\\1/p')\n\
         printf '{{\"type\":\"control_response\",\"response\":{{\"subtype\":\"success\",\"request_id\":\"%s\"}}}}\\n' \"$asked\"\n\
         ;;\n\
         *)\n\
         if [ \"$announced\" = 0 ]; then announced=1; printf '%s\\n' '{init}'; fi\n\
         printf '%s\\n' '{RESULT}'\n\
         ;;\n\
         esac\n\
         done\n",
        capture = capture.display(),
    ))
}

fn lines_written(capture: &Path, at_least: usize) -> Vec<Value> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut written = Vec::new();
    while Instant::now() < deadline && written.len() < at_least {
        written = std::fs::read_to_string(capture)
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).expect("a protocol line"))
            .collect();
        std::thread::sleep(Duration::from_millis(5));
    }
    written
}

fn choosing(model: Option<&str>, effort: Option<&str>) -> ModelChoice {
    ModelChoice {
        provider: AgentProvider::ClaudeAdk,
        model: model.map(str::to_string),
        effort: effort.map(str::to_string),
    }
}

/// The codex carrier's per-turn choice, in this protocol's shape: a turn
/// frozen to a model the child is not running writes one `set_model`
/// control request ahead of the turn, on the same pipe, and the child's
/// ack is what moves the model the session reports. A turn frozen to the
/// model already running writes nothing extra.
#[test]
fn a_turn_choosing_another_model_asks_for_it_ahead_of_the_turn() {
    let dir = tempfile::tempdir().expect("temp dir");
    let capture = dir.path().join("stdin.jsonl");
    let session = open_with(
        &recording_child(&capture, INIT),
        &choosing(Some("claude-fable-5-1"), Some("high")),
    );
    assert_eq!(
        session.turn_choice_support(&choosing(Some("claude-opus-5"), Some("high"))),
        TurnChoiceSupport::Native,
        "a model change is taken in place"
    );

    session
        .send_turn(&Turn::with_choice(
            "first",
            choosing(Some("claude-fable-5-1"), Some("high")),
            1,
        ))
        .expect("the spawn's own choice needs no change");
    wait_for_status(&session, AgentStatus::Waiting);
    session
        .send_turn(&Turn::with_choice(
            "second",
            choosing(Some("claude-opus-5"), Some("high")),
            2,
        ))
        .expect("a model change is applied in place");
    wait_for_status(&session, AgentStatus::Waiting);
    session
        .send_turn(&Turn::with_choice(
            "third",
            choosing(Some("claude-opus-5"), Some("high")),
            3,
        ))
        .expect("the model already running needs no change");

    let written = lines_written(&capture, 4);
    let kinds: Vec<(String, String)> = written
        .iter()
        .map(|line| {
            (
                line["type"].as_str().unwrap_or_default().to_string(),
                line["request"]["subtype"]
                    .as_str()
                    .or_else(|| line["message"]["content"][0]["text"].as_str())
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect();
    assert_eq!(
        kinds,
        vec![
            ("user".to_string(), "first".to_string()),
            ("control_request".to_string(), "set_model".to_string()),
            ("user".to_string(), "second".to_string()),
            ("user".to_string(), "third".to_string()),
        ],
        "one ask, ahead of the turn that needs it, and none for a model already running: {written:?}"
    );
    assert_eq!(written[1]["request"]["model"], "claude-opus-5");
    assert!(
        becomes_true_within(Duration::from_secs(5), || session.active_model().as_deref()
            == Some("claude-opus-5")),
        "the ack moves the model the session reports: {:?}",
        session.active_model()
    );
    session.end();
}

/// What this child cannot take in place is refused with the sentence that
/// says where it lives — never dropped, because a turn that silently ran
/// on other settings would report itself as the settings the human chose.
/// Effort has no control request (probed: `update_settings` refuses a
/// session source), a model cannot be cleared back to the default once one
/// was named, and another provider's choice is another provider's session.
#[test]
fn a_choice_this_child_cannot_take_in_place_is_refused_and_says_so() {
    let session = open_with(
        &stream_json_harness(&[RESULT]),
        &choosing(Some("claude-fable-5-1"), Some("high")),
    );
    assert!(session.accepts_turn_choice());

    for (choice, what) in [
        (
            choosing(Some("claude-fable-5-1"), Some("low")),
            "an effort change",
        ),
        (
            choosing(None, Some("high")),
            "a model cleared to the default",
        ),
    ] {
        assert_eq!(
            session.turn_choice_support(&choice),
            TurnChoiceSupport::RestartRequired,
            "{what}"
        );
        let refused = session
            .send_turn(&Turn::with_choice("go", choice, 1))
            .expect_err(what);
        assert!(
            matches!(&refused, HarnessError::Unsupported(text) if text.contains("fresh session")),
            "{what}: {refused}"
        );
    }
    let foreign = ModelChoice {
        provider: AgentProvider::CodexAppServer,
        model: Some("gpt-6-astra".to_string()),
        effort: Some("high".to_string()),
    };
    assert_eq!(
        session.turn_choice_support(&foreign),
        TurnChoiceSupport::RestartRequired
    );
    let refused = session
        .send_turn(&Turn::with_choice("go", foreign, 1))
        .expect_err("another provider's choice");
    assert!(
        matches!(&refused, HarnessError::Unsupported(text) if text.contains("Codex")),
        "{refused}"
    );
    assert_eq!(
        session.status(),
        AgentStatus::Starting,
        "a refused turn was never written, so the child was never spoken to"
    );
    session.end();
}

/// The codex carrier's `verify_thread_settings`, for claude: the `init` line
/// says what model the child is running, and one running something other
/// than what Build asked for is ended with that as its last words — the
/// agent would otherwise report every turn as the model the human chose.
#[test]
fn a_child_running_another_model_than_asked_is_ended_with_that_as_its_epitaph() {
    let session = open_with(
        &stream_json_harness(&[RESULT]),
        &choosing(Some("claude-opus-5"), None),
    );
    session
        .send_turn(&Turn::new("go"))
        .expect("the turn is written");
    assert!(
        becomes_true_within(Duration::from_secs(5), || matches!(
            session.status(),
            AgentStatus::Ended { .. }
        )),
        "the child announced claude-fable-5-1 and was asked for claude-opus-5: {:?}",
        session.status()
    );
    let epitaph = session.epitaph().expect("the mismatch is the epitaph");
    assert!(
        epitaph.contains("claude-fable-5-1") && epitaph.contains("claude-opus-5"),
        "{epitaph}"
    );
    assert_eq!(
        session.start_refused().as_deref(),
        Some(epitaph.as_str()),
        "and the agent's start_error says it in the same sentence"
    );

    // And the model it WAS asked for is announced without incident.
    let session = open_with(
        &stream_json_harness(&[RESULT]),
        &choosing(Some("claude-fable-5-1"), None),
    );
    session
        .send_turn(&Turn::new("go"))
        .expect("the turn is written");
    wait_for_status(&session, AgentStatus::Waiting);
    assert_eq!(session.epitaph(), None);
    session.end();
}

/// Issue #72: `--model opus` announces the model the alias stands for, and an
/// agent asked for the alias is running what it asked for. An alias of another
/// family is still a mismatch, and says so.
#[test]
fn an_alias_is_the_model_it_resolves_to_and_no_other() {
    let session = open_with(
        &stream_json_harness(&[RESULT]),
        &choosing(Some("fable"), None),
    );
    session
        .send_turn(&Turn::new("go"))
        .expect("the turn is written");
    wait_for_status(&session, AgentStatus::Waiting);
    assert_eq!(session.epitaph(), None);
    assert_eq!(session.start_refused(), None);
    session.end();

    let session = open_with(
        &stream_json_harness(&[RESULT]),
        &choosing(Some("opus"), None),
    );
    session
        .send_turn(&Turn::new("go"))
        .expect("the turn is written");
    assert!(
        becomes_true_within(Duration::from_secs(5), || matches!(
            session.status(),
            AgentStatus::Ended { .. }
        )),
        "the child announced claude-fable-5-1 and was asked for opus: {:?}",
        session.status()
    );
    assert_eq!(
        session.start_refused().as_deref(),
        Some("Build stopped this agent's Claude Code session because it opened claude-fable-5-1, and the agent asks for opus.")
    );
    session.end();
}

/// And the answer the dying child had already written does not talk Build
/// out of it. A result that succeeded clears the error a turn reported —
/// but a child Build ENDED keeps the words it was ended over, or the crash
/// notice would explain a killed session with the last thing that went
/// right. The fake writes both lines at once, so the result is always read
/// after the mismatch rather than sometimes.
#[test]
fn a_result_already_in_flight_does_not_clear_the_epitaph_that_ended_the_child() {
    let session = open_with(
        &stream_json_harness_answering_in_the_same_breath(),
        &choosing(Some("claude-opus-5"), None),
    );
    assert!(
        becomes_true_within(Duration::from_secs(5), || matches!(
            session.status(),
            AgentStatus::Ended { .. }
        )),
        "the child announced claude-fable-5-1 and was asked for claude-opus-5: {:?}",
        session.status()
    );
    let epitaph = session
        .epitaph()
        .expect("the mismatch is still the epitaph, result or no result");
    assert!(
        epitaph.contains("claude-fable-5-1") && epitaph.contains("claude-opus-5"),
        "{epitaph}"
    );
    session.end();
}

/// The codex carrier's reconciliation timeout, for claude: a child handed a
/// turn that never announces itself is ended at the deadline with that as
/// its last words, instead of holding `Starting` until the idle sweep
/// explains the silence as nothing. The deadline counts from the first
/// turn, because the CLI says nothing until it has read one — a session
/// nobody has spoken to is `Starting` for as long as it likes.
#[test]
fn a_child_that_never_announces_itself_is_ended_at_the_deadline_after_its_first_turn() {
    let silent = HarnessSpec::new("sh").arg("-c").arg("cat >/dev/null");
    let deadline = Duration::from_millis(200);
    let (session, _activity) =
        AdkSession::spawn_with_startup_deadline(&silent, None, &adk_choice(), deadline)
            .expect("the silent child spawns");
    let now = Instant::now();
    let mut unprompted = ProtocolState::new(&adk_choice());
    assert!(
        !super::super::session::startup_deadline_elapsed(&unprompted, deadline, now + deadline * 2),
        "silence before the first turn has no deadline"
    );
    unprompted.first_turn_at = Some(now);
    assert!(
        !super::super::session::startup_deadline_elapsed(
            &unprompted,
            deadline,
            now + deadline - Duration::from_nanos(1)
        ),
        "a turn still inside the deadline is allowed to start"
    );
    assert!(
        super::super::session::startup_deadline_elapsed(&unprompted, deadline, now + deadline),
        "a silent turn expires exactly at the deadline"
    );
    unprompted.closed = true;
    assert!(
        !super::super::session::startup_deadline_elapsed(&unprompted, deadline, now + deadline),
        "an ended child cannot expire twice"
    );
    assert_eq!(
        session.status(),
        AgentStatus::Starting,
        "no turn yet, so no silence to hold against it"
    );
    assert_eq!(session.epitaph(), None);

    session
        .send_turn(&Turn::new("go"))
        .expect("the turn is written");
    assert!(
        becomes_true_within(Duration::from_secs(5), || matches!(
            session.status(),
            AgentStatus::Ended { .. }
        )),
        "{:?}",
        session.status()
    );
    assert!(
        session
            .epitaph()
            .is_some_and(|epitaph| epitaph.contains("did not announce itself")),
        "{:?}",
        session.epitaph()
    );

    // A child that DOES announce itself in time is left alone past the deadline.
    let (session, _activity) = AdkSession::spawn_with_startup_deadline(
        &stream_json_harness(&[RESULT]),
        None,
        &adk_choice(),
        Duration::from_secs(2),
    )
    .expect("the fake spawns");
    session
        .send_turn(&Turn::new("go"))
        .expect("the turn is written");
    wait_for_status(&session, AgentStatus::Waiting);
    let mut announced = ProtocolState::new(&adk_choice());
    announced.first_turn_at = Some(now);
    announced.announced = true;
    assert!(
        !super::super::session::startup_deadline_elapsed(
            &announced,
            Duration::from_secs(2),
            now + Duration::from_secs(3)
        ),
        "an announced child is exempt even after the deadline"
    );
    assert_eq!(session.status(), AgentStatus::Waiting);
    assert_eq!(session.epitaph(), None);
    session.end();
}

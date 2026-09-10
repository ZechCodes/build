// Exact test bodies moved from the former inline test module.
use super::*;

#[test]
fn the_workflow_fixture_mints_the_rows_it_always_minted() {
    let rows = reports_minted_by(WORKFLOW_FIXTURE);

    assert_eq!(
        kinds_of(&rows),
        vec![
            "reasoning",
            "tool_use",
            "task_update",
            "tool_result",
            "reasoning",
            "narration",
            "task_update",
            "task_update",
            "reasoning",
            "narration",
        ]
    );
    assert_eq!(
        task_rows_of(&rows),
        vec![
            "Count README.md lines and characters, then summarize — started".to_string(),
            "Count README.md lines and characters, then summarize — finished".to_string(),
            "Dynamic workflow \"Count README.md lines and characters, then summarize\" completed"
                .to_string(),
        ]
    );
}
#[test]
fn the_subagent_fixture_folds_the_subagents_own_rows_under_the_call_that_spawned_them() {
    let rows = reports_minted_by(SUBAGENT_FIXTURE);
    let spawning_call = Some(SUBAGENT_SPAWNING_CALL_ID);

    assert_eq!(
        kinds_and_parents_of(&rows),
        vec![
            ("reasoning", None),
            ("tool_use", None),
            ("task_update", None),
            ("tool_result", None),
            ("reasoning", spawning_call),
            ("reasoning", None),
            ("narration", None),
            ("tool_use", spawning_call),
            ("tool_result", spawning_call),
            ("reasoning", spawning_call),
            ("narration", spawning_call),
            ("task_update", None),
            ("task_update", None),
            ("reasoning", None),
            ("narration", None),
        ]
    );
    assert_eq!(
        task_rows_of(&rows),
        vec![
            "Read README.md and report character count — started".to_string(),
            "Read README.md and report character count — finished".to_string(),
            "4".to_string(),
        ]
    );
}
#[test]
fn the_shared_wire_fixture_is_what_the_recorded_streams_build() {
    let checked_in: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(SHARED_WIRE_FIXTURE)
            .unwrap_or_else(|why| panic!("the shared surfaces fixture reads: {why}")),
    )
    .expect("the shared surfaces fixture is JSON");

    assert_eq!(
        checked_in,
        the_wire_every_recorded_stream_builds(),
        "re-record it with `cargo test the_shared_wire_fixture -- --ignored --nocapture`"
    );
}
#[test]
#[ignore = "prints the shared surfaces fixture so it can be re-recorded"]
fn the_shared_wire_fixture_as_the_recorded_streams_build_it() {
    println!(
        "{}",
        serde_json::to_string_pretty(&the_wire_every_recorded_stream_builds())
            .expect("the wire value writes")
    );
}
#[test]
fn the_workflow_fixture_leaves_one_finished_workflow_of_two_phases_and_three_agents() {
    let surfaces = surfaces_of(&reader_over_every_line_of(WORKFLOW_FIXTURE))
        .expect("the workflow fixture leaves the session a snapshot");

    assert_eq!(surfaces.workflows.len(), 1, "{surfaces:?}");
    let workflow = &surfaces.workflows[0];
    assert_eq!(workflow.state.as_deref(), Some("done"));
    assert_eq!(
        workflow
            .phases
            .iter()
            .map(|phase| phase.title.as_str())
            .collect::<Vec<_>>(),
        vec!["Read", "Summarize"]
    );
    assert_eq!(agent_states_of(workflow), vec![Some("done"); 3]);
}
#[test]
fn the_subagent_fixture_leaves_one_finished_subagent_carrying_its_answer() {
    let surfaces = surfaces_of(&reader_over_every_line_of(SUBAGENT_FIXTURE))
        .expect("the subagent fixture leaves the session a snapshot");

    assert_eq!(surfaces.subagents.len(), 1, "{surfaces:?}");
    assert_eq!(surfaces.subagents[0].state.as_deref(), Some("done"));
    assert_eq!(surfaces.subagents[0].result.as_deref(), Some("4"));
}
#[test]
fn the_shell_and_checklist_fixture_leaves_three_finished_items_and_one_finished_shell() {
    let surfaces = surfaces_of(&reader_over_every_line_of(SHELL_AND_CHECKLIST_FIXTURE))
        .expect("the shell and checklist fixture leaves the session a snapshot");

    assert_eq!(
        surfaces
            .checklist
            .iter()
            .map(|item| item.state.as_deref())
            .collect::<Vec<_>>(),
        vec![Some("completed"); 3],
        "{surfaces:?}"
    );
    assert_eq!(surfaces.shells.len(), 1, "{surfaces:?}");
    assert_eq!(surfaces.shells[0].state.as_deref(), Some("done"));
    assert_eq!(surfaces.shells[0].exit_code, Some(0));
}
#[test]
fn the_counter_moves_once_per_line_that_moved_the_snapshot() {
    let whole_workflow = revision_counter_of(&reader_over_every_line_of(WORKFLOW_FIXTURE));
    let the_start_alone = revision_counter_of(&reader_over_the_workflow_lines(&[37]));

    assert_eq!(the_start_alone, 1);
    assert!(
            whole_workflow > the_start_alone,
            "the whole workflow moved the snapshot more than its first line: {whole_workflow} against {the_start_alone}"
        );
    assert_eq!(
        revision_counter_of(&reader_over_the_workflow_lines(&[46])),
        0,
        "a usage tick carrying no progress array moves nothing"
    );
}
#[test]
fn a_reader_nobody_is_watching_reads_the_stream_to_its_end() {
    let mut reader = reader_over_a_silent_session();
    drop(reader.revision.subscribe());

    for line in fixture_lines(WORKFLOW_FIXTURE) {
        reader.read_line(&line);
    }

    assert!(
        revision_counter_of(&reader) > 0,
        "a bump with nobody watching is not an error"
    );
    assert_eq!(
        surfaces_of(&reader)
            .expect("the whole stream was read")
            .workflows
            .len(),
        1
    );
}
#[test]
fn the_task_roster_stays_the_one_authority_for_whether_the_agent_is_working() {
    let reader = reader_over_every_line_of(WORKFLOW_FIXTURE);
    let state = reader.state.lock().unwrap();

    assert!(state.tasks.is_empty(), "{:?}", state.tasks);
    assert_eq!(state.live_status(), AgentStatus::Waiting);
    assert!(
        state.surfaces.snapshot().is_some(),
        "a finished workflow is still on the snapshot the rail paints"
    );
}
#[test]
fn a_minted_call_records_the_tool_its_answer_will_be_routed_by() {
    let lines = fixture_lines(SHELL_AND_CHECKLIST_FIXTURE);
    let mut reader = reader_over_a_silent_session();

    reader.read_line(&lines[26]);

    assert_eq!(
        reader.calls.get(FIRST_CREATE_CALL_ID),
        Some(&RecordedCall::Minted {
            tool: "TaskCreate".to_string(),
            parent_call_id: None,
        })
    );
}
#[test]
fn an_answered_call_is_taken_from_the_map_not_read() {
    let lines = fixture_lines(SHELL_AND_CHECKLIST_FIXTURE);
    let mut reader = reader_over_a_silent_session();

    reader.read_line(&lines[26]);
    assert_eq!(
        reader.calls.len(),
        1,
        "the call is held until its answer arrives"
    );
    reader.read_line(&lines[27]);
    assert!(
        reader.calls.is_empty(),
        "the answer takes the entry it paired, mid-turn: {:?}",
        reader.calls
    );

    for line in &lines {
        reader.read_line(line);
    }
    let after_one_pass = reader.calls.len();
    for line in &lines {
        reader.read_line(line);
    }

    assert_eq!(after_one_pass, 0, "every call in the fixture was answered");
    assert_eq!(reader.calls.len(), after_one_pass);
}
/// Every content block becomes one conversation event, in the order the
/// child reported it — the timeline reads as the turn happened.
#[tokio::test]
async fn activity_is_minted_in_the_order_the_protocol_reported_it() {
    let session = open(&stream_json_harness(&[
        THINKING,
        TOOL_USE,
        TOOL_RESULT,
        NARRATION,
        RESULT,
    ]));
    let mut activity = session.activity().expect("a reporting session");
    wait_for_status(&session, AgentStatus::Waiting);
    session.send_turn(&Turn::new("drop the index")).unwrap();

    assert_eq!(
        next_activity(&mut activity).await,
        AgentActivity::Reasoning {
            summary: "the index is unused".to_string()
        }
    );
    assert_eq!(
        next_activity(&mut activity).await,
        AgentActivity::ToolUse {
            call_id: "toolu_1".to_string(),
            summary: "Read bridge/src/app.rs".to_string()
        }
    );
    assert_eq!(
        next_activity(&mut activity).await,
        AgentActivity::ToolResult {
            call_id: "toolu_1".to_string(),
            outcome: ToolOutcome::Ok,
            summary: "fn main() {}".to_string()
        },
        "the answer names the call it answers, and carries the answer alone"
    );
    assert_eq!(
        next_activity(&mut activity).await,
        AgentActivity::Narration {
            summary: "dropped the index".to_string()
        }
    );
    session.end();
}
/// The pairing is by id, never by adjacency: two calls answered in the
/// reverse order each carry their own id, so the daemon lands each answer
/// on the row its own call minted.
#[tokio::test]
async fn two_calls_answered_out_of_order_each_name_their_own_call() {
    let session = open(&stream_json_harness(&[
        TOOL_USE,
        SECOND_TOOL_USE,
        SECOND_TOOL_RESULT,
        TOOL_RESULT,
        RESULT,
    ]));
    let mut activity = session.activity().expect("a reporting session");
    wait_for_status(&session, AgentStatus::Waiting);
    session.send_turn(&Turn::new("read both")).unwrap();

    for expected in ["toolu_1", "toolu_2"] {
        let AgentActivity::ToolUse { call_id, .. } = next_activity(&mut activity).await else {
            panic!("a call was expected");
        };
        assert_eq!(call_id, expected);
    }
    assert_eq!(
        next_activity(&mut activity).await,
        AgentActivity::ToolResult {
            call_id: "toolu_2".to_string(),
            outcome: ToolOutcome::Ok,
            summary: "pub struct Thread".to_string()
        }
    );
    assert_eq!(
        next_activity(&mut activity).await,
        AgentActivity::ToolResult {
            call_id: "toolu_1".to_string(),
            outcome: ToolOutcome::Ok,
            summary: "fn main() {}".to_string()
        }
    );
    session.end();
}
/// An answer to a call nobody announced still reaches the timeline: the
/// reader knows nothing about it beyond its id and what it says, and says
/// exactly that.
#[tokio::test]
async fn an_answer_to_a_call_nobody_announced_is_still_reported() {
    let session = open(&stream_json_harness(&[ORPHAN_TOOL_RESULT, RESULT]));
    let mut activity = session.activity().expect("a reporting session");
    wait_for_status(&session, AgentStatus::Waiting);
    session.send_turn(&Turn::new("read the file")).unwrap();

    assert_eq!(
        next_activity(&mut activity).await,
        AgentActivity::ToolResult {
            call_id: "toolu_nobody_announced".to_string(),
            outcome: ToolOutcome::Ok,
            summary: "an answer to nothing".to_string()
        }
    );
    session.end();
}
/// A tool the table never heard of — an MCP tool, or one newer than the
/// table — still reads as words rather than punctuation: the first
/// string-valued field it carried, and never a brace.
#[test]
fn an_unlisted_tool_mints_its_first_string_field_and_no_braces() {
    let summary = tool_call_summary(
        "mcp__linear__create_issue",
        &json!({ "estimate": 3, "title": "the pump drops rows", "labels": ["bug"] }),
    );
    assert_eq!(summary, "mcp__linear__create_issue the pump drops rows");
    assert!(!summary.contains('{'), "{summary}");
}
/// With no words anywhere in the call, the tool's name is the whole row —
/// and a listed tool whose meat key the call omitted falls through to the
/// same fallback rather than inventing one.
#[test]
fn a_call_with_no_words_mints_the_tool_name_alone() {
    assert_eq!(tool_call_summary("Ping", &json!({ "attempts": 3 })), "Ping");
    assert_eq!(tool_call_summary("Bash", &json!({})), "Bash");
    assert_eq!(
        tool_call_summary("Read", &json!({ "offset": 10, "reason": "audit the pump" })),
        "Read audit the pump",
        "a listed tool without its own key takes the unlisted rule",
    );
}
/// The same rule through the pump the child actually speaks to: a recorded
/// `Bash` call lands one row, and that row is the command line.
#[tokio::test]
async fn a_bash_call_reaches_the_conversation_as_its_command_line() {
    let session = open(&stream_json_harness(&[BASH_TOOL_USE, RESULT]));
    let mut activity = session.activity().expect("a reporting session");
    wait_for_status(&session, AgentStatus::Waiting);
    session.send_turn(&Turn::new("run the suite")).unwrap();

    assert_eq!(
        next_activity(&mut activity).await,
        AgentActivity::ToolUse {
            call_id: "toolu_bash".to_string(),
            summary: "Bash cargo test".to_string(),
        }
    );
    session.end();
}
/// Every task row leads with the work it names and ends with what happened
/// to it. A row carries no label in front of it any more, so a row that led
/// with `started` would spend its first word on its least informative one —
/// and the description is what a reader is scanning for.
///
/// One leg for all four mints, because they are one wording: a task
/// announcing itself, a patch that fails it, a roster that inserts it, a
/// roster that drops it, and the notification row — which was
/// description-first already and does not move.
#[tokio::test]
async fn task_rows_lead_with_the_work_and_end_with_what_happened() {
    let session = open(&stream_json_harness(&[
        TASK_STARTED,
        TASK_UPDATED_FAILED,
        TASK_ROSTER,
        TASK_ROSTER_EMPTY,
        TASK_STARTED,
        TASK_NOTIFICATION_MULTILINE,
        RESULT,
    ]));
    let mut activity = session.activity().expect("a reporting session");
    wait_for_status(&session, AgentStatus::Waiting);
    session.send_turn(&Turn::new("run the reindex")).unwrap();

    for want in [
        format!("{TASK_DESCRIPTION} — started"),
        format!("{TASK_DESCRIPTION} — failed: exit code 1"),
        format!("{TASK_DESCRIPTION} — started"),
        format!("{TASK_DESCRIPTION} — finished"),
        format!("{TASK_DESCRIPTION} — started"),
        format!("{TASK_DESCRIPTION}: Background command completed woke"),
        format!("{TASK_DESCRIPTION} — finished"),
    ] {
        assert_eq!(
            next_activity(&mut activity).await,
            AgentActivity::TaskUpdate {
                summary: want.clone()
            },
        );
    }
    session.end();
}
/// A `task_updated` that carries a terminal status ends the task itself —
/// `failed` when it said so, with the error it named — and the roster that
/// later omits the id mints nothing more, because by then nothing moves.
#[tokio::test]
async fn a_terminal_task_update_fails_the_task_once() {
    let session = open(&stream_json_harness(&[
        TASK_STARTED,
        TASK_UPDATED_FAILED,
        TASK_ROSTER_EMPTY,
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
        AgentActivity::TaskUpdate {
            summary: format!("{TASK_DESCRIPTION} — failed: exit code 1"),
        }
    );
    assert_eq!(
        next_activity(&mut activity).await,
        AgentActivity::Narration {
            summary: "dropped the index".to_string()
        },
        "the roster that follows a task already ended moves nothing"
    );

    wait_for_status(&session, AgentStatus::Waiting);
    session.end();
}
/// A notification is the task saying something worth reading, so its text is
/// minted under the task's own name — collapsed onto one line, because this
/// is operational text rather than the agent speaking — and, when the status
/// it carries is terminal, it closes the task as well: the text first, then
/// the ending it announces.
///
/// The second notification lands with the set already empty, so it mints its
/// text on its own and closes nothing: one row per transition, and by then
/// nothing moves. A patch that moves neither membership nor any
/// human-readable text is a progress counter ticking, and mints nothing at
/// all.
#[tokio::test]
async fn a_task_notification_is_minted_and_a_progress_patch_is_not() {
    let session = open(&stream_json_harness(&[
        TASK_STARTED,
        TASK_UPDATED_PROGRESS,
        TASK_NOTIFICATION,
        TASK_NOTIFICATION_MULTILINE,
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
            AgentActivity::TaskUpdate {
                summary: format!(
                    "{TASK_DESCRIPTION}: Background command \"{TASK_DESCRIPTION}\" completed (exit code 0)"
                ),
            },
            "the progress patch before it moved nothing and said nothing new"
        );
    assert_eq!(
        next_activity(&mut activity).await,
        AgentActivity::TaskUpdate {
            summary: format!("{TASK_DESCRIPTION} — finished"),
        },
        "the status it carried was terminal, so the notification ended the task"
    );
    assert_eq!(
            next_activity(&mut activity).await,
            AgentActivity::TaskUpdate {
                summary: "Background command completed woke".to_string(),
            },
            "several lines of output are one row, the way a tool answer is — and the set no longer holds a name to speak it under"
        );
    assert_eq!(
        next_activity(&mut activity).await,
        AgentActivity::Narration {
            summary: "dropped the index".to_string()
        }
    );

    wait_for_status(&session, AgentStatus::Waiting);
    session.end();
}
/// The other two terminal statuses a foreground notification carries, both
/// live-recorded: `failed`, which reads as a failure and names no error
/// because the notification carries none, and `stopped`, which does not —
/// something ended that work, which is not the same thing to read.
///
/// Both must remove. An unrecognised status would leave its task in the set
/// with no roster coming to clear it, which is the same pin under a
/// different name.
#[tokio::test]
async fn a_failed_foreground_notification_fails_and_a_stopped_one_finishes() {
    let session = open(&stream_json_harness(&[
        FOREGROUND_TASK_FAILED_STARTED,
        FOREGROUND_TASK_NOTIFICATION_FAILED,
        FOREGROUND_TASK_STARTED,
        FOREGROUND_TASK_NOTIFICATION_STOPPED,
        RESULT,
        NARRATION,
    ]));
    let mut activity = session.activity().expect("a reporting session");
    wait_for_status(&session, AgentStatus::Waiting);
    session
        .send_turn(&Turn::new("run the two commands"))
        .unwrap();

    let mut minted = Vec::new();
    for _ in 0..5 {
        minted.push(next_activity(&mut activity).await);
    }
    assert_eq!(
        minted,
        vec![
            AgentActivity::TaskUpdate {
                summary: format!("{FOREGROUND_TASK_FAILED_DESCRIPTION} — started"),
            },
            AgentActivity::TaskUpdate {
                summary: format!("{FOREGROUND_TASK_FAILED_DESCRIPTION} — failed"),
            },
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
/// The recorded probe, replayed in the order the live child emitted it:
/// roster, `task_started`, the turn's `result`, empty roster,
/// `task_updated`, `task_notification`.
///
/// Two of those six events move the set and four do not, so the timeline
/// gets exactly three rows — the start, the end, and what the task said.
/// The notification arrives after the roster already closed the task, which
/// is why it reads as its own text rather than under a name the set no
/// longer holds.
#[tokio::test]
async fn the_probes_own_order_mints_one_row_per_transition() {
    let session = open(&stream_json_harness(&[
        TASK_ROSTER,
        TASK_STARTED,
        RESULT,
        TASK_ROSTER_EMPTY,
        TASK_UPDATED_DONE,
        TASK_NOTIFICATION,
        NARRATION,
    ]));
    let mut activity = session.activity().expect("a reporting session");
    wait_for_status(&session, AgentStatus::Waiting);
    session.send_turn(&Turn::new("run the reindex")).unwrap();

    let mut minted = Vec::new();
    for _ in 0..4 {
        minted.push(next_activity(&mut activity).await);
    }
    assert_eq!(
        minted,
        vec![
            AgentActivity::TaskUpdate {
                summary: format!("{TASK_DESCRIPTION} — started"),
            },
            AgentActivity::TaskUpdate {
                summary: format!("{TASK_DESCRIPTION} — finished"),
            },
            AgentActivity::TaskUpdate {
                summary: format!(
                    "Background command \"{TASK_DESCRIPTION}\" completed (exit code 0)"
                ),
            },
            AgentActivity::Narration {
                summary: "dropped the index".to_string()
            },
        ]
    );

    wait_for_status(&session, AgentStatus::Waiting);
    session.end();
}
/// Build's own tools already arrive as themselves over the MCP socket —
/// `done` posts a completion, `post_thread_message` posts a message. Minting
/// the call as well would tell the timeline the same thing twice, so neither
/// the call nor the answer to it is minted.
#[tokio::test]
async fn builds_own_tool_calls_are_not_minted() {
    let session = open(&stream_json_harness(&[
        DONE_CALL,
        DONE_RESULT,
        NARRATION,
        RESULT,
    ]));
    let mut activity = session.activity().expect("a reporting session");
    wait_for_status(&session, AgentStatus::Waiting);
    session.send_turn(&Turn::new("finish up")).unwrap();

    assert_eq!(
        next_activity(&mut activity).await,
        AgentActivity::Narration {
            summary: "dropped the index".to_string()
        },
        "the done call and its answer belong to the socket, not the timeline"
    );
    session.end();
}
#[tokio::test]
async fn subagent_events_are_reported_under_the_call_that_spawned_them() {
    let session = open(&stream_json_harness(&[SUBAGENT_TEXT, NARRATION, RESULT]));
    let mut activity = session.activity().expect("a reporting session");
    wait_for_status(&session, AgentStatus::Waiting);
    session.send_turn(&Turn::new("delegate it")).unwrap();

    assert_eq!(
        next_report(&mut activity).await,
        ActivityReport {
            activity: AgentActivity::Narration {
                summary: "a subagent talking".to_string()
            },
            parent_call_id: Some("toolu_1".to_string()),
        },
        "the subagent's own text names the call it belongs under"
    );
    assert_eq!(
        next_report(&mut activity).await,
        ActivityReport::own_work(AgentActivity::Narration {
            summary: "dropped the index".to_string()
        }),
        "and the session's own narration names nothing"
    );
    session.end();
}

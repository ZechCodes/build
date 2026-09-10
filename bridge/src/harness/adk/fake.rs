use crate::pty::HarnessSpec;

/// The protocol lines a real headless session emits, recorded so the fake
/// harness below replays exactly what claude would say. Single quotes are
/// forbidden inside them: the fake is a `sh -c` script that quotes each
/// line, and a stray quote would rewrite the protocol rather than fail.
pub(crate) const INIT: &str = r#"{"type":"system","subtype":"init","session_id":"sess-adk","model":"claude-fable-5-1","capabilities":["msg_lifecycle_v1","interrupt_receipt_v1","interrupt_cancel_queued_v1"]}"#;
/// The same child on a CLI built before the interrupt landed: it announces
/// itself and names no capabilities at all. What a refusal is tested
/// against, and the reason the question is asked of the child rather than
/// of a version number.
pub(crate) const INIT_WITHOUT_INTERRUPT: &str = r#"{"type":"system","subtype":"init","session_id":"sess-adk","model":"claude-fable-5-1","capabilities":["msg_lifecycle_v1"]}"#;
pub(crate) const THINKING: &str = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"thinking","thinking":"the index is unused"}]},"parent_tool_use_id":null}"#;
pub(crate) const TOOL_USE: &str = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_1","name":"Read","input":{"file_path":"bridge/src/app.rs"}}]},"parent_tool_use_id":null}"#;
pub(crate) const TOOL_RESULT: &str = r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"fn main() {}"}]},"parent_tool_use_id":null}"#;
/// A call carrying more than one argument — the recorded shape above with
/// its id, tool name and input swapped for the two fields the `Bash` tool
/// takes, as the live CLI sends them. What a row minted from a multi-field
/// call is read against: the command, and not the model's paraphrase of it.
pub(crate) const BASH_TOOL_USE: &str = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_bash","name":"Bash","input":{"command":"cargo test","description":"Run the test suite"}}]},"parent_tool_use_id":null}"#;
/// An answer that FAILED — recorded from a live probe against claude
/// 2.1.236 on 2026-08-30: one headless turn that read a path which does not
/// exist. `is_error` is the whole of what says so, and the block carries no
/// other marker.
///
/// The same substitutions the recordings above take and nothing else: the
/// `tool_use_id` is swapped for this module's, and the probe's own working
/// directory for a short path. Every field the reader looks at is verbatim.
pub(crate) const ERROR_TOOL_RESULT: &str = r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"File does not exist. Note: your current working directory is /work.","is_error":true,"tool_use_id":"toolu_1"}]},"parent_tool_use_id":null}"#;
/// A SECOND call and its answer, the recorded pair above with its id and
/// the file it reads changed and nothing else — what an out-of-order
/// interleave is told apart by, since adjacency cannot tell it.
pub(crate) const SECOND_TOOL_USE: &str = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_2","name":"Read","input":{"file_path":"bridge/src/thread.rs"}}]},"parent_tool_use_id":null}"#;
pub(crate) const SECOND_TOOL_RESULT: &str = r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_2","content":"pub struct Thread"}]},"parent_tool_use_id":null}"#;
/// An answer to a call this session never saw announced — the shape a
/// broadcast lag or a reload leaves behind, and the only thing that still
/// mints a `tool_result` row of its own.
pub(crate) const ORPHAN_TOOL_RESULT: &str = r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_nobody_announced","content":"an answer to nothing"}]},"parent_tool_use_id":null}"#;
pub(crate) const NARRATION: &str = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"dropped the index"}]},"parent_tool_use_id":null}"#;
pub(crate) const RESULT: &str = r#"{"type":"result","subtype":"success","is_error":false,"result":"dropped the index","session_id":"sess-adk"}"#;
pub(crate) const DONE_CALL: &str = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_done","name":"mcp__build__done","input":{"phase":"build","status":"completed"}}]},"parent_tool_use_id":null}"#;
pub(crate) const DONE_RESULT: &str = r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_done","content":"recorded"}]},"parent_tool_use_id":null}"#;
pub(crate) const SUBAGENT_TEXT: &str = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"a subagent talking"}]},"parent_tool_use_id":"toolu_1"}"#;
pub(crate) const FAILED_RESULT: &str = r#"{"type":"result","subtype":"error_during_execution","is_error":true,"result":"the tool call was refused","session_id":"sess-adk"}"#;

/// The background-task lines, recorded from a live probe against claude
/// 2.1.236 on 2026-08-29: one headless turn that put `sleep 12 && echo
/// woke` in the background and wrote two lines of haiku.
///
/// The child emitted them in exactly this order — roster, `task_started`,
/// the turn's `result`, empty roster, `task_updated`, `task_notification`
/// — which is what [`the_probes_own_order_mints_one_row_per_transition`]
/// replays. Two substitutions and nothing else: the recorded `session_id`
/// and `uuid` values are swapped for this module's, and the notification's
/// `output_file` for a short path, so the recordings read as one session
/// and carry no machine paths. Every field the reader looks at is verbatim.
pub(crate) const TASK_ROSTER: &str = r#"{"type":"system","subtype":"background_tasks_changed","tasks":[{"task_id":"bi1jfa1kd","task_type":"local_bash","description":"Sleep 12 seconds then echo woke"}],"uuid":"task-uuid-1","session_id":"sess-adk"}"#;
pub(crate) const TASK_STARTED: &str = r#"{"type":"system","subtype":"task_started","task_id":"bi1jfa1kd","tool_use_id":"toolu_bg","description":"Sleep 12 seconds then echo woke","task_type":"local_bash","uuid":"task-uuid-2","session_id":"sess-adk"}"#;
/// The roster with nothing on it — how the live child says the work is
/// over, ahead of the `task_updated` that says which way it went.
pub(crate) const TASK_ROSTER_EMPTY: &str = r#"{"type":"system","subtype":"background_tasks_changed","tasks":[],"uuid":"task-uuid-3","session_id":"sess-adk"}"#;
pub(crate) const TASK_UPDATED_DONE: &str = r#"{"type":"system","subtype":"task_updated","task_id":"bi1jfa1kd","patch":{"status":"completed","end_time":1788057950364},"uuid":"task-uuid-4","session_id":"sess-adk"}"#;
pub(crate) const TASK_NOTIFICATION: &str = r#"{"type":"system","subtype":"task_notification","task_id":"bi1jfa1kd","tool_use_id":"toolu_bg","status":"completed","output_file":"/tmp/tasks/bi1jfa1kd.output","summary":"Background command \"Sleep 12 seconds then echo woke\" completed (exit code 0)","uuid":"task-uuid-5","session_id":"sess-adk"}"#;

/// Three the probe's own command had no reason to produce, each the
/// recorded shape above with one field changed and nothing else: a task
/// that FAILED rather than completed, a patch that reports progress rather
/// than an ending, and a notification whose text runs over several lines
/// the way a command quoting its own output would.
pub(crate) const TASK_UPDATED_FAILED: &str = r#"{"type":"system","subtype":"task_updated","task_id":"bi1jfa1kd","patch":{"status":"failed","error":"exit code 1","end_time":1788057950364},"uuid":"task-uuid-6","session_id":"sess-adk"}"#;
pub(crate) const TASK_UPDATED_PROGRESS: &str = r#"{"type":"system","subtype":"task_updated","task_id":"bi1jfa1kd","patch":{"output_lines":12},"uuid":"task-uuid-7","session_id":"sess-adk"}"#;
pub(crate) const TASK_NOTIFICATION_MULTILINE: &str = r#"{"type":"system","subtype":"task_notification","task_id":"bi1jfa1kd","tool_use_id":"toolu_bg","status":"completed","output_file":"/tmp/tasks/bi1jfa1kd.output","summary":"Background command completed\n\n  woke\n","uuid":"task-uuid-8","session_id":"sess-adk"}"#;

/// What the recorded task calls itself — the human-readable half of every
/// summary the reader mints for it.
pub(crate) const TASK_DESCRIPTION: &str = "Sleep 12 seconds then echo woke";

/// A FOREGROUND Bash command is a task too, and it ends differently —
/// recorded from a second live probe against claude 2.1.236 on 2026-08-30:
/// one headless turn that ran a plain `sleep 10` and answered after it.
///
/// The child emitted exactly two task lines for it, in this order:
/// `task_started`, then a `task_notification` carrying `status`
/// `completed`. No `task_updated` and no `background_tasks_changed`, ever —
/// so the notification is the ONLY event that says the work is over, and a
/// reader that took it for chatter would hold the task for the life of the
/// session. Note the `summary`: for a foreground task it is the
/// description, word for word, which is why text that only repeats the
/// task's own name mints no row of its own.
///
/// The same substitutions as the recordings above and nothing else — the
/// `session_id`, `uuid` and `tool_use_id` values are swapped for this
/// module's. Every field the reader looks at is verbatim.
pub(crate) const FOREGROUND_TASK_STARTED: &str = r#"{"type":"system","subtype":"task_started","task_id":"bwhwgc2zw","tool_use_id":"toolu_fg","description":"Sleep for 10 seconds","task_type":"local_bash","uuid":"task-uuid-9","session_id":"sess-adk"}"#;
pub(crate) const FOREGROUND_TASK_NOTIFICATION: &str = r#"{"type":"system","subtype":"task_notification","task_id":"bwhwgc2zw","tool_use_id":"toolu_fg","status":"completed","output_file":"","summary":"Sleep for 10 seconds","uuid":"task-uuid-10","session_id":"sess-adk"}"#;
pub(crate) const FOREGROUND_TASK_DESCRIPTION: &str = "Sleep for 10 seconds";

/// The same shape from the same probe run, for a foreground command that
/// EXITED NON-ZERO: `sleep 3; exit 7`. Two task lines again, and the
/// notification's status is `failed` — with no error text anywhere on it,
/// which is why the row it mints names the work and stops there.
pub(crate) const FOREGROUND_TASK_FAILED_STARTED: &str = r#"{"type":"system","subtype":"task_started","task_id":"boq8sla8p","tool_use_id":"toolu_fg2","description":"sleep 3; exit 7","task_type":"local_bash","uuid":"task-uuid-11","session_id":"sess-adk"}"#;
pub(crate) const FOREGROUND_TASK_NOTIFICATION_FAILED: &str = r#"{"type":"system","subtype":"task_notification","task_id":"boq8sla8p","tool_use_id":"toolu_fg2","status":"failed","output_file":"","summary":"sleep 3; exit 7","uuid":"task-uuid-12","session_id":"sess-adk"}"#;
pub(crate) const FOREGROUND_TASK_FAILED_DESCRIPTION: &str = "sleep 3; exit 7";

/// A third terminal status the same probe run turned up, from a task the
/// child killed when its turn ended: `stopped`. Recorded from that
/// notification with its task id, `tool_use_id`, `summary` and
/// `output_file` swapped for the completed foreground recording's, so one
/// `task_started` line above serves this ending too. Its `status` — the one
/// field this recording exists to pin — is verbatim.
pub(crate) const FOREGROUND_TASK_NOTIFICATION_STOPPED: &str = r#"{"type":"system","subtype":"task_notification","task_id":"bwhwgc2zw","tool_use_id":"toolu_fg","status":"stopped","output_file":"","summary":"Sleep for 10 seconds","uuid":"task-uuid-13","session_id":"sess-adk"}"#;

/// Which request the child names in the `control_response` it answers an
/// interrupt with.
enum Acknowledged {
    /// The request it was actually asked — what the live child does.
    TheOneAsked,
    /// Somebody else's, so the reader's matching is exercised rather than
    /// assumed: a response naming another request is noise, and the turn's
    /// own failure keeps its epitaph.
    AnotherRequest,
    /// The one asked, and NOTHING else: an interrupt that lands between
    /// turns has no turn to end, so no result follows the ack. What makes
    /// an interrupt recorded against a finished turn observable — the next
    /// result to arrive is then the NEXT turn's, and a session that took it
    /// as the stopped turn's would swallow that turn whole.
    WithNoTurnToStop,
}

/// A fake stream-json harness: it announces its session after a beat, then
/// replays `per_turn` for every turn written to its stdin, turn after turn
/// for as long as that stdin is open. The beat is what makes `Starting`
/// observable — a real child's init line does not arrive the instant it is
/// forked either.
///
/// It also speaks the control protocol, in the order the live wire does: a
/// `control_request` is answered with a `control_response`, then with the
/// `error_during_execution` result that closes the turn it stopped, and
/// only then does the loop read the next queued turn.
pub(crate) fn stream_json_harness(per_turn: &[&str]) -> HarnessSpec {
    harness_replaying(INIT, &[per_turn], true, Acknowledged::TheOneAsked, None)
}

/// The same child, appending every line written to its stdin to `heard`.
///
/// What Build SAID is otherwise invisible from outside the session, and the
/// steering flow IS two writes in one order — so a test watching only what
/// came back could not tell an interrupt that was sent from one that was
/// not.
pub(crate) fn stream_json_harness_recording_stdin(
    per_turn: &[&str],
    heard: &std::path::Path,
) -> HarnessSpec {
    harness_replaying(
        INIT,
        &[per_turn],
        true,
        Acknowledged::TheOneAsked,
        Some(heard),
    )
}

/// The same child, for one turn only: it answers, then leaves the way a
/// real one does when its work is over. That departure closes its stream,
/// which is what a no-terminal session's death rites hang off.
pub(crate) fn stream_json_harness_that_leaves(per_turn: &[&str]) -> HarnessSpec {
    harness_replaying(INIT, &[per_turn], false, Acknowledged::TheOneAsked, None)
}

/// A CLI that announces no interrupt. Build never sends it a
/// `control_request`, because [`AdkSession::can_interrupt`] reads the same
/// announcement the refusal does.
pub(crate) fn stream_json_harness_without_interrupt(per_turn: &[&str]) -> HarnessSpec {
    harness_replaying(
        INIT_WITHOUT_INTERRUPT,
        &[per_turn],
        true,
        Acknowledged::TheOneAsked,
        None,
    )
}

/// A child that answers an interrupt by naming a request nobody made, and
/// then fails the turn on its own account.
pub(crate) fn stream_json_harness_answering_another_request(per_turn: &[&str]) -> HarnessSpec {
    harness_replaying(INIT, &[per_turn], true, Acknowledged::AnotherRequest, None)
}

/// A recording child with nothing to stop: it acks a `control_request` and
/// emits no result, because an interrupt that lands between turns ends no
/// turn. Both halves matter to the stale press — the recorder says whether
/// the press was spoken at all, and the missing result is what leaves the
/// NEXT turn's result the only one there is.
pub(crate) fn stream_json_harness_with_nothing_to_stop(
    per_turn: &[&str],
    heard: &std::path::Path,
) -> HarnessSpec {
    harness_replaying(
        INIT,
        &[per_turn],
        true,
        Acknowledged::WithNoTurnToStop,
        Some(heard),
    )
}

/// A child that answers each turn with its OWN recording — the first turn
/// with the first, the second with the second, and every turn after that
/// with the last one.
///
/// What a single recording cannot express: work that is live after one turn
/// and over after the next, which is the whole shape the idle sweep has to
/// tell apart.
pub(crate) fn stream_json_harness_turn_by_turn(turns: &[&[&str]]) -> HarnessSpec {
    harness_replaying(INIT, turns, true, Acknowledged::TheOneAsked, None)
}

fn harness_replaying(
    init: &str,
    turns: &[&[&str]],
    turn_after_turn: bool,
    acknowledged: Acknowledged,
    heard: Option<&std::path::Path>,
) -> HarnessSpec {
    let recorded = |lines: &[&str]| {
        let mut replay = String::new();
        for line in lines {
            assert!(
                !line.contains('\''),
                "a recorded protocol line may not carry a single quote: {line}"
            );
            replay.push_str(&format!("printf '%s\\n' '{line}'\n"));
        }
        replay
    };
    // One recording needs no counter and produces the script it always did.
    // Several are answered by a `case` on how many turns have been read, in
    // which the last recording is the catch-all — so a child asked for more
    // turns than were recorded keeps answering rather than falling silent.
    let (counter, replay) = match turns {
        [only] => (String::new(), recorded(only)),
        _ => {
            let mut arms = String::new();
            for (index, lines) in turns.iter().enumerate() {
                let label = match index == turns.len() - 1 {
                    true => "*".to_string(),
                    false => (index + 1).to_string(),
                };
                arms.push_str(&format!("{label})\n{}\n;;\n", recorded(lines)));
            }
            (
                "turns=0\n".to_string(),
                format!("turns=$((turns+1))\ncase \"$turns\" in\n{arms}esac\n"),
            )
        }
    };
    // The one place the script interpolates rather than quoting a recording:
    // the id it echoes is a value it read at runtime.
    let echoed = match acknowledged {
        Acknowledged::TheOneAsked | Acknowledged::WithNoTurnToStop => "\"$asked\"",
        Acknowledged::AnotherRequest => "nobody-asked-this",
    };
    // The result that closes the turn the interrupt stopped — unless there
    // was no turn to stop, in which case the ack is the whole answer.
    let stopped_turn = match acknowledged {
        Acknowledged::WithNoTurnToStop => String::new(),
        _ => format!("printf '%s\\n' '{FAILED_RESULT}'\n"),
    };
    let mut script = format!("sleep 0.2\nprintf '%s\\n' '{init}'\n{counter}");
    script.push_str(match turn_after_turn {
        true => "while IFS= read -r turn; do\n",
        false => "if IFS= read -r turn; then\n",
    });
    if let Some(heard) = heard {
        script.push_str(&format!(
            "printf '%s\\n' \"$turn\" >> \"{}\"\n",
            heard.display()
        ));
    }
    script.push_str(&format!(
            "case \"$turn\" in\n\
             *control_request*)\n\
             asked=$(printf '%s' \"$turn\" | sed -n 's/.*\"request_id\":\"\\([^\"]*\\)\".*/\\1/p')\n\
             printf '{{\"type\":\"control_response\",\"response\":{{\"subtype\":\"success\",\"request_id\":\"%s\"}}}}\\n' {echoed}\n\
             {stopped_turn}\
             ;;\n\
             *)\n\
             {replay}\
             ;;\n\
             esac\n"
        ));
    script.push_str(match turn_after_turn {
        true => "done\n",
        false => "fi\n",
    });
    HarnessSpec::new("sh").arg("-c").arg(script)
}

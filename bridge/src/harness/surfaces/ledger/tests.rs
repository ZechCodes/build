use super::*;
use crate::harness::adk::{tool_result_text, TOOL_SUMMARY_LIMIT};
use crate::harness::stream_fixtures::{
    fixture_events, fixture_line, recorded_workflow_surfaces,
    the_line_counter_carrying_a_spawning_call_id, CHAR_COUNTER_AGENT_ID, FIRST_CREATE_CALL_ID,
    FIRST_UPDATE_CALL_ID, LINE_COUNTER_AGENT_ID, SHELL_AND_CHECKLIST_FIXTURE, SHELL_LAUNCHED_AT_MS,
    SHELL_LAUNCH_ANSWER_LINE, SHELL_LAUNCH_CALL_LINE, SHELL_NOTIFICATION_LINE, SHELL_OUTPUT_PATH,
    SHELL_STARTED_LINE, SHELL_TASK_ID, SHELL_UPDATED_LINE, SUBAGENT_FIXTURE,
    SUBAGENT_SPAWNING_CALL_ID, SUBAGENT_TASK_ID, WORKFLOW_FIXTURE, WORKFLOW_SPAWNING_CALL_ID,
    WORKFLOW_TASK_ID,
};

fn one_checklist_item() -> SurfaceChecklistItem {
    SurfaceChecklistItem {
        id: "task-1".to_string(),
        subject: "Count the lines".to_string(),
        description: Some("Read README.md and count".to_string()),
        state: Some(ChecklistState::InProgress),
    }
}

fn one_shell() -> SurfaceShell {
    SurfaceShell {
        id: "bash-1".to_string(),
        description: Some("run the suite".to_string()),
        state: Some(RUNNING.to_string()),
        started_at: None,
        exit_code: None,
        tail: vec!["test one ... ok".to_string()],
        closed_by_notification: false,
    }
}

fn no_call_sequence(_spawning_call_id: &str) -> Option<u64> {
    None
}

#[test]
fn a_snapshot_holding_only_a_checklist_writes_only_that_key() {
    let surfaces = AgentSurfaces {
        checklist: vec![one_checklist_item()],
        ..AgentSurfaces::default()
    };

    let written = surfaces.wire_value(&no_call_sequence);
    let keys: Vec<&str> = written
        .as_object()
        .expect("the snapshot writes an object")
        .keys()
        .map(String::as_str)
        .collect();

    assert_eq!(keys, vec!["checklist"]);
}

#[test]
fn an_all_empty_snapshot_is_empty_and_writes_nothing() {
    let surfaces = AgentSurfaces::default();

    assert!(surfaces.is_empty());
    assert_eq!(
        surfaces.wire_value(&no_call_sequence),
        serde_json::json!({})
    );
}

#[test]
fn a_subagent_carries_the_call_sequence_the_closure_answers() {
    let surfaces = AgentSurfaces {
        subagents: vec![the_line_counter_carrying_a_spawning_call_id()],
        ..AgentSurfaces::default()
    };
    let answers_forty_one = |spawning_call_id: &str| {
        assert_eq!(spawning_call_id, WORKFLOW_SPAWNING_CALL_ID);
        Some(41)
    };

    let answered = surfaces.wire_value(&answers_forty_one);
    assert_eq!(answered["subagents"][0]["call_sequence"], 41);

    let unanswered = surfaces.wire_value(&no_call_sequence);
    assert!(
        !unanswered["subagents"][0]
            .as_object()
            .expect("a subagent writes an object")
            .contains_key("call_sequence"),
        "an unanswered spawning call writes no key at all: {unanswered}"
    );
}

#[test]
fn no_snapshot_ever_writes_an_internal_field() {
    let surfaces = AgentSurfaces {
        workflows: vec![SurfaceWorkflow {
            id: WORKFLOW_TASK_ID.to_string(),
            name: "count-and-summarize".to_string(),
            description: Some("Count README.md lines".to_string()),
            state: Some("running".to_string()),
            phases: vec![SurfacePhase {
                title: "Read".to_string(),
                agents: vec![the_line_counter_carrying_a_spawning_call_id()],
            }],
        }],
        subagents: vec![the_line_counter_carrying_a_spawning_call_id()],
        shells: vec![one_shell()],
        checklist: vec![one_checklist_item()],
        ..AgentSurfaces::default()
    };

    let written = surfaces.wire_value(&no_call_sequence).to_string();

    assert!(!written.contains("spawning_call_id"), "{written}");
}

#[test]
fn a_running_shells_output_path_is_the_ledgers_alone_and_never_the_snapshots() {
    let mut ledger = SurfaceLedger::default();
    let shell = one_shell();
    let output_path = PathBuf::from("/private/tmp/shell-out.log");
    ledger
        .shell_outputs
        .insert(shell.id.clone(), output_path.clone());
    ledger.shells.push(shell.clone());

    assert_eq!(
        ledger.running_shell_outputs(),
        vec![(shell.id.clone(), output_path)]
    );

    let written = ledger
        .snapshot()
        .expect("the ledger holds a snapshot")
        .wire_value(&no_call_sequence)
        .to_string();
    assert!(!written.contains("shell_outputs"), "{written}");
    assert!(!written.contains("shell-out.log"), "{written}");
}

#[test]
fn a_shell_that_is_no_longer_running_is_no_longer_tailed() {
    let mut ledger = SurfaceLedger::default();
    let shell = one_shell();
    ledger.shell_outputs.insert(
        shell.id.clone(),
        PathBuf::from("/private/tmp/shell-out.log"),
    );
    ledger.shells.push(SurfaceShell {
        state: Some("done".to_string()),
        ..shell
    });

    assert!(ledger.running_shell_outputs().is_empty());
}

#[test]
fn the_task_state_table_maps_terminal_statuses_and_leaves_the_rest_alone() {
    assert_eq!(wire_task_state("completed"), Some("done"));
    assert_eq!(wire_task_state("failed"), Some("failed"));
    assert_eq!(wire_task_state("error"), Some("failed"));
    assert_eq!(wire_task_state("timed_out"), Some("failed"));
    assert_eq!(wire_task_state("killed"), Some("done"));
    assert_eq!(wire_task_state("stopped"), Some("done"));
    assert_eq!(wire_task_state("cancelled"), Some("done"));
    assert_eq!(wire_task_state("running"), None);
}

#[test]
fn the_agent_state_table_reads_a_start_without_a_start_time_as_queued() {
    assert_eq!(wire_agent_state("start", true), Some("running"));
    assert_eq!(wire_agent_state("start", false), Some("queued"));
    assert_eq!(wire_agent_state("progress", true), Some("running"));
    assert_eq!(wire_agent_state("progress", false), Some("queued"));
    assert_eq!(wire_agent_state("done", false), Some("done"));
    assert_eq!(wire_agent_state("done", true), Some("done"));
    assert_eq!(wire_agent_state("thinking", true), None);
    assert_eq!(wire_agent_state("thinking", false), None);
}

fn feed(ledger: &mut SurfaceLedger, event: &Value) -> bool {
    let subtype = event["subtype"]
        .as_str()
        .expect("every fixture system line names a subtype")
        .to_string();
    ledger.read_task_event(&subtype, event)
}

fn feed_line(ledger: &mut SurfaceLedger, fixture: &str, line_number: usize) -> bool {
    feed(ledger, &fixture_line(fixture, line_number))
}

fn snapshot_of(ledger: &SurfaceLedger) -> AgentSurfaces {
    ledger.snapshot().expect("the ledger holds a snapshot")
}

fn the_only<T: Clone + std::fmt::Debug>(held: &[T]) -> T {
    assert_eq!(held.len(), 1, "{held:?}");
    held[0].clone()
}

fn written(ledger: &SurfaceLedger) -> String {
    snapshot_of(ledger)
        .wire_value(&no_call_sequence)
        .to_string()
}

fn agent_named(workflow: &SurfaceWorkflow, label: &str) -> SurfaceAgent {
    workflow
        .phases
        .iter()
        .flat_map(|phase| phase.agents.iter())
        .find(|agent| agent.label == label)
        .unwrap_or_else(|| panic!("a {label} agent is in {workflow:?}"))
        .clone()
}

fn phase_holding(workflow: &SurfaceWorkflow, label: &str) -> String {
    workflow
        .phases
        .iter()
        .find(|phase| phase.agents.iter().any(|agent| agent.label == label))
        .unwrap_or_else(|| panic!("a phase holds {label} in {workflow:?}"))
        .title
        .clone()
}

fn ledger_through_the_final_progress_array() -> SurfaceLedger {
    let mut ledger = SurfaceLedger::default();
    for line_number in [37, 40, 46, 63] {
        feed_line(&mut ledger, WORKFLOW_FIXTURE, line_number);
    }
    ledger
}

#[test]
fn the_recorded_snapshot_is_what_the_fixtures_build() {
    let mut ledger = ledger_through_the_final_progress_array();
    feed(&mut ledger, &fixture_line(SUBAGENT_FIXTURE, 11));
    feed(&mut ledger, &fixture_line(SUBAGENT_FIXTURE, 31));

    assert_eq!(
        ledger.snapshot().expect("the ledger holds a snapshot"),
        recorded_workflow_surfaces()
    );
}

#[test]
fn a_ledger_that_has_read_nothing_holds_no_snapshot() {
    assert!(SurfaceLedger::default().snapshot().is_none());
}

#[test]
fn a_started_local_workflow_opens_a_running_workflow() {
    let mut ledger = SurfaceLedger::default();

    assert!(feed_line(&mut ledger, WORKFLOW_FIXTURE, 37));

    let workflow = the_only(&snapshot_of(&ledger).workflows);
    assert_eq!(workflow.id, WORKFLOW_TASK_ID);
    assert_eq!(workflow.name, "readme-analysis");
    assert_eq!(
        workflow.description.as_deref(),
        Some("Count README.md lines and characters, then summarize")
    );
    assert_eq!(workflow.state.as_deref(), Some("running"));
    assert!(workflow.phases.is_empty(), "{workflow:?}");
}

#[test]
fn a_started_workflow_never_holds_the_script_it_was_handed() {
    let mut ledger = SurfaceLedger::default();
    feed_line(&mut ledger, WORKFLOW_FIXTURE, 37);

    assert!(
        !written(&ledger).contains("export const meta"),
        "the workflow script must not reach the snapshot: {}",
        written(&ledger)
    );
}

#[test]
fn the_first_progress_array_names_both_phases_and_a_queued_agent() {
    let mut ledger = SurfaceLedger::default();
    feed_line(&mut ledger, WORKFLOW_FIXTURE, 37);

    assert!(feed_line(&mut ledger, WORKFLOW_FIXTURE, 40));

    let workflow = the_only(&snapshot_of(&ledger).workflows);
    let titles: Vec<&str> = workflow
        .phases
        .iter()
        .map(|phase| phase.title.as_str())
        .collect();
    assert_eq!(titles, vec!["Read", "Summarize"]);

    let line_counter = agent_named(&workflow, "line-counter");
    assert_eq!(line_counter.id, LINE_COUNTER_AGENT_ID);
    assert_eq!(line_counter.state.as_deref(), Some("running"));
    assert_eq!(phase_holding(&workflow, "line-counter"), "Read");

    let char_counter = agent_named(&workflow, "char-counter");
    assert_eq!(char_counter.id, format!("{WORKFLOW_TASK_ID}:2"));
    assert_eq!(char_counter.state.as_deref(), Some("queued"));
    assert_eq!(phase_holding(&workflow, "char-counter"), "Read");
}

#[test]
fn a_usage_tick_carrying_no_progress_array_moves_nothing() {
    let mut ledger = SurfaceLedger::default();
    feed_line(&mut ledger, WORKFLOW_FIXTURE, 37);
    feed_line(&mut ledger, WORKFLOW_FIXTURE, 40);
    let before = written(&ledger);

    assert!(!feed_line(&mut ledger, WORKFLOW_FIXTURE, 46));

    assert_eq!(written(&ledger), before);
}

#[test]
fn the_final_progress_array_takes_the_real_id_and_the_agent_totals() {
    let ledger = ledger_through_the_final_progress_array();

    let workflow = the_only(&snapshot_of(&ledger).workflows);
    assert_eq!(
        agent_named(&workflow, "char-counter").id,
        CHAR_COUNTER_AGENT_ID
    );
    for label in ["line-counter", "char-counter", "summarizer"] {
        assert_eq!(
            agent_named(&workflow, label).state.as_deref(),
            Some("done"),
            "{label} reads done"
        );
    }
    assert_eq!(phase_holding(&workflow, "summarizer"), "Summarize");

    let line_counter = agent_named(&workflow, "line-counter");
    assert_eq!(line_counter.tokens, Some(11_409));
    assert_eq!(line_counter.tool_calls, Some(1));
    assert_eq!(line_counter.duration_ms, Some(4_732));
    assert_eq!(line_counter.result.as_deref(), Some("2"));
    assert_eq!(
        line_counter.last_tool,
        Some(SurfaceTool {
            name: "Read".to_string(),
            summary: Some(
                "/private/tmp/claude-501/-Users-adam--superconductor-worktre…".to_string()
            ),
        })
    );
}

#[test]
fn every_free_text_field_a_workflow_carries_reaches_the_snapshot_bounded() {
    let mut ledger = ledger_through_the_final_progress_array();
    let sprawl = "x".repeat(4_000);
    let mut sprawling = fixture_line(WORKFLOW_FIXTURE, 63);
    for entry in sprawling["workflow_progress"]
        .as_array_mut()
        .expect("the final progress line carries an array")
    {
        for field in [
            "title",
            "label",
            "model",
            "lastToolSummary",
            "resultPreview",
        ] {
            if !entry[field].is_null() {
                entry[field] = json!(sprawl);
            }
        }
        if entry["type"] == json!("workflow_agent") {
            entry["error"] = json!(sprawl);
        }
    }

    assert!(feed(&mut ledger, &sprawling));

    for text in every_free_text_in(&written(&ledger)) {
        assert!(
            text.chars().count() <= TOOL_SUMMARY_LIMIT + 1,
            "{text} is unbounded"
        );
    }
}

#[test]
fn a_sprawling_workflow_name_and_subagent_label_reach_the_snapshot_bounded() {
    let sprawl = "x".repeat(4_000);
    let mut ledger = SurfaceLedger::default();
    let mut started_workflow = fixture_line(WORKFLOW_FIXTURE, 37);
    started_workflow["workflow_name"] = json!(sprawl);
    started_workflow["description"] = json!(sprawl);
    let mut started_subagent = fixture_line(SUBAGENT_FIXTURE, 11);
    started_subagent["description"] = json!(sprawl);

    assert!(feed(&mut ledger, &started_workflow));
    assert!(feed(&mut ledger, &started_subagent));

    for text in every_free_text_in(&written(&ledger)) {
        assert!(
            text.chars().count() <= TOOL_SUMMARY_LIMIT + 1,
            "{text} is unbounded"
        );
    }
}

fn every_free_text_in(written: &str) -> Vec<String> {
    fn walk(value: &Value, held: &mut Vec<String>) {
        match value {
            Value::String(text) => held.push(text.clone()),
            Value::Array(entries) => entries.iter().for_each(|entry| walk(entry, held)),
            Value::Object(fields) => fields.values().for_each(|field| walk(field, held)),
            _ => {}
        }
    }
    let mut held = Vec::new();
    walk(
        &serde_json::from_str::<Value>(written).expect("the snapshot is json"),
        &mut held,
    );
    held
}

#[test]
fn a_progress_array_replaces_the_phases_rather_than_merging_into_them() {
    let mut ledger = ledger_through_the_final_progress_array();
    let mut without_the_char_counter = fixture_line(WORKFLOW_FIXTURE, 63);
    let kept: Vec<Value> = without_the_char_counter["workflow_progress"]
        .as_array()
        .expect("the final progress line carries an array")
        .iter()
        .filter(|entry| entry["label"] != json!("char-counter"))
        .cloned()
        .collect();
    without_the_char_counter["workflow_progress"] = json!(kept);

    assert!(feed(&mut ledger, &without_the_char_counter));

    let workflow = the_only(&snapshot_of(&ledger).workflows);
    assert!(
        !written(&ledger).contains("char-counter"),
        "a dropped agent leaves the snapshot: {workflow:?}"
    );
    assert_eq!(
        agent_named(&workflow, "line-counter").id,
        LINE_COUNTER_AGENT_ID
    );
}

#[test]
fn an_agent_whose_phase_is_in_no_phase_entry_still_reaches_a_phase() {
    let mut ledger = ledger_through_the_final_progress_array();
    let mut naming_an_unlisted_phase = fixture_line(WORKFLOW_FIXTURE, 63);
    naming_an_unlisted_phase["workflow_progress"] = json!([{
        "type": "workflow_agent",
        "index": 9,
        "label": "verifier",
        "phaseIndex": 7,
        "phaseTitle": "Verify",
        "state": "start",
        "queuedAt": 1_788_290_178_060u64,
    }]);

    assert!(feed(&mut ledger, &naming_an_unlisted_phase));

    let workflow = the_only(&snapshot_of(&ledger).workflows);
    assert_eq!(phase_holding(&workflow, "verifier"), "Verify");
    assert_eq!(
        agent_named(&workflow, "verifier").id,
        format!("{WORKFLOW_TASK_ID}:9")
    );
}

#[test]
fn a_phase_carrying_no_index_is_numbered_by_its_place_among_the_phases() {
    let mut ledger = ledger_through_the_final_progress_array();
    let mut phases_without_indexes = fixture_line(WORKFLOW_FIXTURE, 63);
    phases_without_indexes["workflow_progress"] = json!([
        {
            "type": "workflow_agent",
            "index": 4,
            "label": "verifier",
            "phaseIndex": 0,
            "state": "start",
            "queuedAt": 1_788_290_178_060u64,
        },
        { "type": "workflow_phase", "title": "Read" },
        { "type": "workflow_phase", "title": "Summarize" },
    ]);

    assert!(feed(&mut ledger, &phases_without_indexes));

    let workflow = the_only(&snapshot_of(&ledger).workflows);
    let titles: Vec<&str> = workflow
        .phases
        .iter()
        .map(|phase| phase.title.as_str())
        .collect();
    assert_eq!(titles, vec!["Read", "Summarize"], "{workflow:?}");
    assert_eq!(phase_holding(&workflow, "verifier"), "Read");
}

#[test]
fn the_closing_lines_finish_the_workflow_and_an_unclaimed_status_changes_nothing() {
    let mut ledger = ledger_through_the_final_progress_array();

    assert!(feed_line(&mut ledger, WORKFLOW_FIXTURE, 65));
    assert_eq!(
        the_only(&snapshot_of(&ledger).workflows).state.as_deref(),
        Some("done")
    );

    assert!(!feed_line(&mut ledger, WORKFLOW_FIXTURE, 66));
    assert_eq!(
        the_only(&snapshot_of(&ledger).workflows).state.as_deref(),
        Some("done")
    );

    assert!(!feed(
        &mut ledger,
        &json!({
            "subtype": "task_notification",
            "task_id": WORKFLOW_TASK_ID,
            "status": "running",
        })
    ));
    assert_eq!(
        the_only(&snapshot_of(&ledger).workflows).state.as_deref(),
        Some("done")
    );

    assert!(feed(
        &mut ledger,
        &json!({
            "subtype": "task_notification",
            "task_id": WORKFLOW_TASK_ID,
            "status": "failed",
        })
    ));
    assert_eq!(
        the_only(&snapshot_of(&ledger).workflows).state.as_deref(),
        Some("failed")
    );
}

#[test]
fn a_progress_line_for_a_workflow_that_never_started_is_ignored() {
    let mut ledger = SurfaceLedger::default();

    assert!(!feed_line(&mut ledger, WORKFLOW_FIXTURE, 40));
    assert!(ledger.snapshot().is_none());

    feed_line(&mut ledger, WORKFLOW_FIXTURE, 37);
    let before = written(&ledger);
    let mut for_another_task = fixture_line(WORKFLOW_FIXTURE, 63);
    for_another_task["task_id"] = json!("some-other-task");

    assert!(!feed(&mut ledger, &for_another_task));
    assert_eq!(written(&ledger), before);
}

#[test]
fn a_started_subagent_is_no_business_of_the_workflow_parser() {
    let mut ledger = SurfaceLedger::default();

    feed(&mut ledger, &fixture_line(SUBAGENT_FIXTURE, 11));

    let snapshot = ledger.snapshot().expect("the ledger holds a snapshot");
    assert!(snapshot.workflows.is_empty(), "{snapshot:?}");
}

fn ledger_through_the_started_subagent() -> SurfaceLedger {
    let mut ledger = SurfaceLedger::default();
    feed_line(&mut ledger, SUBAGENT_FIXTURE, 11);
    ledger
}

#[test]
fn a_started_local_agent_opens_a_running_subagent() {
    let mut ledger = SurfaceLedger::default();

    assert!(feed_line(&mut ledger, SUBAGENT_FIXTURE, 11));

    let subagent = the_only(&snapshot_of(&ledger).subagents);
    assert_eq!(subagent.id, SUBAGENT_TASK_ID);
    assert_eq!(
        subagent.label,
        "Read README.md and report character count".to_string()
    );
    assert_eq!(subagent.state.as_deref(), Some("running"));
    assert_eq!(
        subagent.spawning_call_id.as_deref(),
        Some(SUBAGENT_SPAWNING_CALL_ID)
    );
    assert_eq!(subagent.started_at, None);
}

#[test]
fn an_agent_launch_answer_reports_the_childs_resolved_model() {
    let mut ledger = SurfaceLedger::default();
    let call = fixture_line(SUBAGENT_FIXTURE, 9)["message"]["content"][0].clone();
    let answer = fixture_line(SUBAGENT_FIXTURE, 12);

    assert!(!ledger.read_tool_call("Agent", &call));
    assert!(feed_line(&mut ledger, SUBAGENT_FIXTURE, 11));
    assert_eq!(
        the_only(&snapshot_of(&ledger).subagents).model.as_deref(),
        Some("haiku")
    );
    assert!(ledger.read_tool_answer(
        "Agent",
        SUBAGENT_SPAWNING_CALL_ID,
        &answer,
        "",
        A_READERS_CLOCK,
    ));

    let subagent = the_only(&snapshot_of(&ledger).subagents);
    assert_eq!(subagent.model.as_deref(), Some("claude-haiku-4-5-20251001"));
    assert_eq!(subagent.reasoning_effort, None);
}

#[test]
fn child_event_metadata_reports_reasoning_effort_without_parent_inference() {
    let mut ledger = SurfaceLedger::default();
    let mut started = fixture_line(SUBAGENT_FIXTURE, 11);
    started["model"] = json!("claude-sonnet-4-5-20250929");
    started["reasoningEffort"] = json!("high");

    assert!(feed(&mut ledger, &started));

    let subagent = the_only(&snapshot_of(&ledger).subagents);
    assert_eq!(
        subagent.model.as_deref(),
        Some("claude-sonnet-4-5-20250929")
    );
    assert_eq!(subagent.reasoning_effort.as_deref(), Some("high"));
}

#[test]
fn a_child_message_upgrades_spawn_metadata_and_a_repeat_start_preserves_it() {
    let mut ledger = SurfaceLedger::default();
    let mut call = fixture_line(SUBAGENT_FIXTURE, 9)["message"]["content"][0].clone();
    call["input"]["effort"] = json!("medium");
    ledger.read_tool_call("Agent", &call);
    feed_line(&mut ledger, SUBAGENT_FIXTURE, 11);

    assert!(ledger.read_subagent_message(
        SUBAGENT_SPAWNING_CALL_ID,
        &json!({"model":"claude-haiku-4-5-20251001", "reasoningEffort":"high"}),
    ));
    assert!(!feed_line(&mut ledger, SUBAGENT_FIXTURE, 11));
    assert!(!ledger.read_tool_answer(
        "Agent",
        SUBAGENT_SPAWNING_CALL_ID,
        &json!({"tool_use_result":{"status":"completed"}}),
        "",
        A_READERS_CLOCK,
    ));

    let subagent = the_only(&snapshot_of(&ledger).subagents);
    assert_eq!(subagent.model.as_deref(), Some("claude-haiku-4-5-20251001"));
    assert_eq!(subagent.reasoning_effort.as_deref(), Some("high"));
}

#[test]
fn a_launch_answer_arriving_before_task_started_is_retained() {
    let mut ledger = SurfaceLedger::default();
    let call = fixture_line(SUBAGENT_FIXTURE, 9)["message"]["content"][0].clone();
    let answer = fixture_line(SUBAGENT_FIXTURE, 12);
    ledger.read_tool_call("Agent", &call);

    assert!(!ledger.read_tool_answer(
        "Agent",
        SUBAGENT_SPAWNING_CALL_ID,
        &answer,
        "",
        A_READERS_CLOCK,
    ));
    assert!(feed_line(&mut ledger, SUBAGENT_FIXTURE, 11));

    assert_eq!(
        the_only(&snapshot_of(&ledger).subagents).model.as_deref(),
        Some("claude-haiku-4-5-20251001")
    );
}

#[test]
fn a_repeated_started_line_keeps_what_the_subagent_has_accumulated() {
    let mut ledger = ledger_through_the_started_subagent();
    feed_line(&mut ledger, SUBAGENT_FIXTURE, 25);
    let progressed = the_only(&snapshot_of(&ledger).subagents);

    assert!(!feed_line(&mut ledger, SUBAGENT_FIXTURE, 11));

    let restarted = the_only(&snapshot_of(&ledger).subagents);
    assert_eq!(restarted.last_tool, progressed.last_tool);
    assert_eq!(restarted.tokens, progressed.tokens);
    assert_eq!(restarted.tool_calls, progressed.tool_calls);
    assert_eq!(restarted.duration_ms, progressed.duration_ms);
    assert_eq!(
        restarted.spawning_call_id.as_deref(),
        Some(SUBAGENT_SPAWNING_CALL_ID)
    );
}

#[test]
fn a_progress_step_reaches_the_snapshot_as_one_bounded_line() {
    let mut ledger = ledger_through_the_started_subagent();
    let mut sprawling = fixture_line(SUBAGENT_FIXTURE, 25);
    sprawling["description"] = json!(format!("first line\nsecond line\n{}", "x".repeat(400)));

    assert!(feed(&mut ledger, &sprawling));

    let summary = the_only(&snapshot_of(&ledger).subagents)
        .last_tool
        .expect("a progressed subagent names its tool")
        .summary
        .expect("the tool carries the current step");
    assert!(!summary.contains('\n'), "{summary}");
    assert_eq!(summary.chars().count(), 241, "{summary}");
    assert!(
        summary.starts_with("first line second line xxx"),
        "{summary}"
    );
}

#[test]
fn a_started_subagent_never_holds_the_prompt_or_the_call_that_spawned_it() {
    let ledger = ledger_through_the_started_subagent();

    let snapshot = written(&ledger);
    for withheld in [
        "Read the README.md file",
        "subagent_type",
        "general-purpose",
        "spawning_call_id",
        SUBAGENT_SPAWNING_CALL_ID,
    ] {
        assert!(
            !snapshot.contains(withheld),
            "{withheld} must not reach the snapshot: {snapshot}"
        );
    }
}

#[test]
fn a_subagent_progress_line_takes_the_current_step_and_the_usage_totals() {
    let mut ledger = ledger_through_the_started_subagent();

    assert!(feed_line(&mut ledger, SUBAGENT_FIXTURE, 25));

    let subagent = the_only(&snapshot_of(&ledger).subagents);
    assert_eq!(
        subagent.last_tool,
        Some(SurfaceTool {
            name: "Read".to_string(),
            summary: Some("Reading README.md".to_string()),
        })
    );
    assert_eq!(subagent.tokens, Some(12_069));
    assert_eq!(subagent.tool_calls, Some(1));
    assert_eq!(subagent.duration_ms, Some(2_832));
    assert_eq!(subagent.label, "Read README.md and report character count");
}

#[test]
fn the_same_subagent_progress_line_twice_moves_nothing_the_second_time() {
    let mut ledger = ledger_through_the_started_subagent();
    feed_line(&mut ledger, SUBAGENT_FIXTURE, 25);
    let before = written(&ledger);

    assert!(!feed_line(&mut ledger, SUBAGENT_FIXTURE, 25));

    assert_eq!(written(&ledger), before);
}

#[test]
fn the_closing_lines_finish_the_subagent_and_take_its_answer() {
    let mut ledger = ledger_through_the_started_subagent();
    feed_line(&mut ledger, SUBAGENT_FIXTURE, 25);

    assert!(feed_line(&mut ledger, SUBAGENT_FIXTURE, 30));
    assert_eq!(
        the_only(&snapshot_of(&ledger).subagents).state.as_deref(),
        Some("done")
    );

    assert!(feed_line(&mut ledger, SUBAGENT_FIXTURE, 31));
    let closed = the_only(&snapshot_of(&ledger).subagents);
    assert_eq!(closed.state.as_deref(), Some("done"));
    assert_eq!(closed.result.as_deref(), Some("4"));
    assert_eq!(closed.error, None);
}

#[test]
fn a_notification_for_a_subagent_that_never_started_mints_nothing() {
    let mut ledger = SurfaceLedger::default();

    assert!(!feed_line(&mut ledger, SUBAGENT_FIXTURE, 31));
    assert!(ledger.snapshot().is_none());
}

#[test]
fn a_timed_out_subagent_reads_failed_and_an_unclaimed_status_changes_nothing() {
    let mut ledger = ledger_through_the_started_subagent();

    assert!(feed(
        &mut ledger,
        &json!({
            "subtype": "task_notification",
            "task_id": SUBAGENT_TASK_ID,
            "status": "timed_out",
            "summary": "the reader gave up",
        })
    ));
    assert_eq!(
        the_only(&snapshot_of(&ledger).subagents).state.as_deref(),
        Some("failed")
    );

    assert!(!feed(
        &mut ledger,
        &json!({
            "subtype": "task_notification",
            "task_id": SUBAGENT_TASK_ID,
            "status": "reticulating",
            "summary": "still going",
        })
    ));
    let unclaimed = the_only(&snapshot_of(&ledger).subagents);
    assert_eq!(unclaimed.state.as_deref(), Some("failed"));
    assert_eq!(unclaimed.error.as_deref(), Some("the reader gave up"));
    assert_eq!(unclaimed.result, None);
}

#[test]
fn a_notification_summary_reaches_the_snapshot_as_one_bounded_line() {
    let mut ledger = ledger_through_the_started_subagent();

    assert!(feed(
        &mut ledger,
        &json!({
            "subtype": "task_notification",
            "task_id": SUBAGENT_TASK_ID,
            "status": "completed",
            "summary": format!("first line\nsecond line\n{}", "x".repeat(400)),
        })
    ));

    let result = the_only(&snapshot_of(&ledger).subagents)
        .result
        .expect("a completed subagent carries its answer");
    assert!(!result.contains('\n'), "{result}");
    assert_eq!(result.chars().count(), 241, "{result}");
    assert!(result.starts_with("first line second line xxx"), "{result}");
}

#[test]
fn the_subagent_fixture_leaves_every_other_kind_empty() {
    let mut ledger = SurfaceLedger::default();
    for event in fixture_events(SUBAGENT_FIXTURE)
        .iter()
        .filter(|event| event["subtype"].is_string())
    {
        feed(&mut ledger, event);
    }

    let snapshot = ledger.snapshot().expect("the ledger holds a snapshot");
    assert!(snapshot.workflows.is_empty(), "{snapshot:?}");
    assert!(snapshot.shells.is_empty(), "{snapshot:?}");
    assert!(snapshot.checklist.is_empty(), "{snapshot:?}");

    let keys: Vec<String> = snapshot
        .wire_value(&no_call_sequence)
        .as_object()
        .expect("the snapshot writes an object")
        .keys()
        .cloned()
        .collect();
    assert_eq!(
        keys,
        vec!["observations".to_string(), "subagents".to_string()]
    );
}

fn the_launch_answer() -> Value {
    fixture_line(SHELL_AND_CHECKLIST_FIXTURE, SHELL_LAUNCH_ANSWER_LINE)
}

fn ledger_through_the_launched_shell() -> SurfaceLedger {
    let mut ledger = SurfaceLedger::default();
    feed_line(&mut ledger, SHELL_AND_CHECKLIST_FIXTURE, SHELL_STARTED_LINE);
    feed_tool_answer(&mut ledger, SHELL_LAUNCH_CALL_LINE, &the_launch_answer());
    ledger
}

fn tail_reading(lines: &[&str], exit_code: Option<i32>) -> ShellTail {
    ShellTail {
        lines: lines.iter().map(|line| line.to_string()).collect(),
        exit_code,
    }
}

#[test]
fn a_started_background_shell_is_running_with_nothing_to_tail_yet() {
    let mut ledger = SurfaceLedger::default();

    assert!(feed_line(
        &mut ledger,
        SHELL_AND_CHECKLIST_FIXTURE,
        SHELL_STARTED_LINE
    ));

    assert_eq!(
        the_only(&snapshot_of(&ledger).shells),
        SurfaceShell {
            id: SHELL_TASK_ID.to_string(),
            description: Some("Background job with ticks and finished message".to_string()),
            state: Some(RUNNING.to_string()),
            started_at: None,
            exit_code: None,
            tail: Vec::new(),
            closed_by_notification: false,
        }
    );
    assert!(
        ledger.running_shell_outputs().is_empty(),
        "no path has been named yet, so there is nothing to tail"
    );
}

#[test]
fn the_launching_answer_names_the_one_file_the_poller_tails() {
    let mut ledger = SurfaceLedger::default();
    feed_line(&mut ledger, SHELL_AND_CHECKLIST_FIXTURE, SHELL_STARTED_LINE);

    assert!(feed_tool_answer(
        &mut ledger,
        SHELL_LAUNCH_CALL_LINE,
        &the_launch_answer()
    ));

    assert_eq!(
        ledger.running_shell_outputs(),
        vec![(SHELL_TASK_ID.to_string(), PathBuf::from(SHELL_OUTPUT_PATH))]
    );
    assert!(
        !written(&ledger).contains(".output"),
        "the path is the ledger's alone: {}",
        written(&ledger)
    );
}

#[test]
fn a_launching_answer_naming_no_output_path_records_none() {
    let mut ledger = SurfaceLedger::default();
    feed_line(&mut ledger, SHELL_AND_CHECKLIST_FIXTURE, SHELL_STARTED_LINE);
    let mut pathless = the_launch_answer();
    pathless["message"]["content"][0]["content"] = json!(format!(
        "Command running in background with ID: {SHELL_TASK_ID}."
    ));

    assert!(!feed_tool_answer(
        &mut ledger,
        SHELL_LAUNCH_CALL_LINE,
        &pathless
    ));

    assert!(ledger.running_shell_outputs().is_empty());
}

#[test]
fn a_launching_answer_for_a_shell_the_ledger_never_started_records_none() {
    let mut ledger = SurfaceLedger::default();
    feed_line(&mut ledger, SHELL_AND_CHECKLIST_FIXTURE, SHELL_STARTED_LINE);
    let mut a_stranger = the_launch_answer();
    a_stranger["tool_use_result"]["backgroundTaskId"] = json!("someone-elses-shell");

    assert!(!feed_tool_answer(
        &mut ledger,
        SHELL_LAUNCH_CALL_LINE,
        &a_stranger
    ));

    assert!(ledger.running_shell_outputs().is_empty());
}

#[test]
fn a_launched_shell_is_stamped_with_the_time_its_launch_answer_carries() {
    let ledger = ledger_through_the_launched_shell();

    assert_eq!(
        the_only(&snapshot_of(&ledger).shells).started_at,
        Some(SHELL_LAUNCHED_AT_MS)
    );
}

#[test]
fn a_launch_answer_carrying_no_time_is_stamped_with_the_readers_clock() {
    let mut ledger = SurfaceLedger::default();
    feed_line(&mut ledger, SHELL_AND_CHECKLIST_FIXTURE, SHELL_STARTED_LINE);
    let mut unstamped = the_launch_answer();
    unstamped["timestamp"] = json!("half past the moon");

    assert!(feed_tool_answer_at(
        &mut ledger,
        SHELL_LAUNCH_CALL_LINE,
        &unstamped,
        A_READERS_CLOCK
    ));

    assert_eq!(
        the_only(&snapshot_of(&ledger).shells).started_at,
        Some(A_READERS_CLOCK)
    );
}

#[test]
fn a_shell_the_launch_answer_never_reached_writes_no_start_time() {
    let mut ledger = SurfaceLedger::default();
    feed_line(&mut ledger, SHELL_AND_CHECKLIST_FIXTURE, SHELL_STARTED_LINE);

    assert_eq!(the_only(&snapshot_of(&ledger).shells).started_at, None);
    assert!(
        !written(&ledger).contains("started_at"),
        "an unknown start time writes no key: {}",
        written(&ledger)
    );
}

#[test]
fn a_second_launch_answer_leaves_the_first_start_time_standing() {
    let mut ledger = ledger_through_the_launched_shell();

    feed_tool_answer_at(
        &mut ledger,
        SHELL_LAUNCH_CALL_LINE,
        &the_launch_answer(),
        A_READERS_CLOCK,
    );

    assert_eq!(
        the_only(&snapshot_of(&ledger).shells).started_at,
        Some(SHELL_LAUNCHED_AT_MS)
    );
}

#[test]
fn a_tail_repeating_what_the_shell_already_holds_moves_nothing() {
    let mut ledger = ledger_through_the_launched_shell();
    let ticking = tail_reading(&["tick 1", "tick 2"], None);

    assert!(ledger.read_shell_tail(SHELL_TASK_ID, ticking.clone()));
    assert_eq!(
        the_only(&snapshot_of(&ledger).shells).tail,
        vec!["tick 1", "tick 2"]
    );

    assert!(!ledger.read_shell_tail(SHELL_TASK_ID, ticking));
}

#[test]
fn a_tail_for_a_shell_the_ledger_does_not_hold_moves_nothing() {
    let mut ledger = ledger_through_the_launched_shell();

    assert!(!ledger.read_shell_tail("never-started", tail_reading(&["tick 1"], None)));

    assert_eq!(
        the_only(&snapshot_of(&ledger).shells).tail,
        Vec::<String>::new()
    );
}

#[test]
fn the_marker_closes_a_shell_no_notification_has_closed() {
    let mut ledger = ledger_through_the_launched_shell();

    assert!(ledger.read_shell_tail(
        SHELL_TASK_ID,
        tail_reading(&["finished", "[exited with code 0]"], Some(0))
    ));

    let closed = the_only(&snapshot_of(&ledger).shells);
    assert_eq!(closed.exit_code, Some(0));
    assert_eq!(closed.state.as_deref(), Some("done"));
    assert!(ledger.running_shell_outputs().is_empty());
}

#[test]
fn a_marker_naming_a_non_zero_code_fails_the_shell() {
    let mut ledger = ledger_through_the_launched_shell();

    assert!(ledger.read_shell_tail(
        SHELL_TASK_ID,
        tail_reading(&["finished", "[exited with code 3]"], Some(3))
    ));

    let closed = the_only(&snapshot_of(&ledger).shells);
    assert_eq!(closed.exit_code, Some(3));
    assert_eq!(closed.state.as_deref(), Some("failed"));
}

#[test]
fn the_notification_closes_the_shell_and_ends_the_tailing() {
    let mut ledger = ledger_through_the_launched_shell();

    assert!(feed_line(
        &mut ledger,
        SHELL_AND_CHECKLIST_FIXTURE,
        SHELL_UPDATED_LINE
    ));
    assert!(feed_line(
        &mut ledger,
        SHELL_AND_CHECKLIST_FIXTURE,
        SHELL_NOTIFICATION_LINE
    ));

    let closed = the_only(&snapshot_of(&ledger).shells);
    assert_eq!(closed.state.as_deref(), Some("done"));
    assert_eq!(closed.exit_code, Some(0));
    assert!(ledger.running_shell_outputs().is_empty());
}

fn the_notification_reporting(status: &str, summary: &str) -> Value {
    let mut notification = fixture_line(SHELL_AND_CHECKLIST_FIXTURE, SHELL_NOTIFICATION_LINE);
    notification["status"] = json!(status);
    notification["summary"] = json!(summary);
    notification
}

#[test]
fn a_notification_reporting_a_failure_closes_the_shell_at_the_code_it_names() {
    let mut ledger = ledger_through_the_launched_shell();

    assert!(feed(
            &mut ledger,
            &the_notification_reporting(
                "failed",
                "Background command \"Background job with ticks and finished message\" completed (exit code 137)",
            )
        ));

    let closed = the_only(&snapshot_of(&ledger).shells);
    assert_eq!(closed.state.as_deref(), Some("failed"));
    assert_eq!(closed.exit_code, Some(137));
    assert!(ledger.running_shell_outputs().is_empty());
}

#[test]
fn a_notification_reporting_completion_at_a_non_zero_code_fails_the_shell() {
    let mut ledger = ledger_through_the_launched_shell();

    assert!(feed(
            &mut ledger,
            &the_notification_reporting(
                "completed",
                "Background command \"Background job with ticks and finished message\" completed (exit code 1)",
            )
        ));

    let closed = the_only(&snapshot_of(&ledger).shells);
    assert_eq!(closed.state.as_deref(), Some("failed"));
    assert_eq!(closed.exit_code, Some(1));
}

#[test]
fn a_notification_naming_no_exit_code_claims_none() {
    let mut ledger = ledger_through_the_launched_shell();

    assert!(feed(
        &mut ledger,
        &the_notification_reporting("failed", "Background command was killed")
    ));

    let closed = the_only(&snapshot_of(&ledger).shells);
    assert_eq!(closed.state.as_deref(), Some("failed"));
    assert_eq!(closed.exit_code, None);
}

#[test]
fn a_marker_arriving_after_the_notification_claims_no_exit_code() {
    let mut ledger = ledger_through_the_launched_shell();
    feed_line(&mut ledger, SHELL_AND_CHECKLIST_FIXTURE, SHELL_UPDATED_LINE);
    feed_line(
        &mut ledger,
        SHELL_AND_CHECKLIST_FIXTURE,
        SHELL_NOTIFICATION_LINE,
    );

    assert!(ledger.read_shell_tail(
        SHELL_TASK_ID,
        tail_reading(&["finished", "[exited with code 3]"], Some(3))
    ));

    let closed = the_only(&snapshot_of(&ledger).shells);
    assert_eq!(
        closed.exit_code,
        Some(0),
        "the notification is authoritative"
    );
    assert_eq!(closed.tail, vec!["finished", "[exited with code 3]"]);
}

#[test]
fn a_shell_restarted_under_the_same_id_reads_its_own_marker_again() {
    let mut ledger = ledger_through_the_launched_shell();
    feed_line(
        &mut ledger,
        SHELL_AND_CHECKLIST_FIXTURE,
        SHELL_NOTIFICATION_LINE,
    );

    assert!(feed_line(
        &mut ledger,
        SHELL_AND_CHECKLIST_FIXTURE,
        SHELL_STARTED_LINE
    ));
    assert!(ledger.read_shell_tail(
        SHELL_TASK_ID,
        tail_reading(&["[exited with code 3]"], Some(3))
    ));

    let restarted = the_only(&snapshot_of(&ledger).shells);
    assert_eq!(restarted.state.as_deref(), Some("failed"));
    assert_eq!(restarted.exit_code, Some(3));
}

#[test]
fn a_marker_arriving_after_a_notification_that_named_no_code_claims_nothing() {
    let mut ledger = ledger_through_the_launched_shell();
    assert!(feed(
        &mut ledger,
        &the_notification_reporting("failed", "Background command was killed")
    ));

    assert!(ledger.read_shell_tail(
        SHELL_TASK_ID,
        tail_reading(&["tick 9", "[exited with code 137]"], Some(137))
    ));

    let closed = the_only(&snapshot_of(&ledger).shells);
    assert_eq!(
        closed.state.as_deref(),
        Some("failed"),
        "the notification is authoritative even when it named no exit code"
    );
    assert_eq!(closed.exit_code, None);
    assert_eq!(closed.tail, vec!["tick 9", "[exited with code 137]"]);
}

#[test]
fn a_status_line_carrying_no_summary_leaves_the_exit_code_the_marker_wrote() {
    let mut ledger = ledger_through_the_launched_shell();
    assert!(ledger.read_shell_tail(
        SHELL_TASK_ID,
        tail_reading(&["finished", "[exited with code 3]"], Some(3))
    ));

    assert!(!feed_line(
        &mut ledger,
        SHELL_AND_CHECKLIST_FIXTURE,
        SHELL_UPDATED_LINE
    ));

    let closed = the_only(&snapshot_of(&ledger).shells);
    assert_eq!(
        closed.exit_code,
        Some(3),
        "a task_updated line carries no summary, so it claims no exit code"
    );
    assert_eq!(
        closed.state.as_deref(),
        Some("failed"),
        "and a status line claiming completion cannot un-fail a non-zero exit"
    );
}

#[test]
fn a_marker_after_a_status_line_still_records_the_code_no_notification_named() {
    let mut ledger = ledger_through_the_launched_shell();
    feed_line(&mut ledger, SHELL_AND_CHECKLIST_FIXTURE, SHELL_UPDATED_LINE);

    assert!(ledger.read_shell_tail(
        SHELL_TASK_ID,
        tail_reading(&["finished", "[exited with code 3]"], Some(3))
    ));

    assert_eq!(the_only(&snapshot_of(&ledger).shells).exit_code, Some(3));
}

#[test]
fn a_notification_naming_no_code_clears_an_exit_code_the_marker_wrote() {
    let mut ledger = ledger_through_the_launched_shell();
    assert!(ledger.read_shell_tail(
        SHELL_TASK_ID,
        tail_reading(&["[exited with code 3]"], Some(3))
    ));

    assert!(feed(
        &mut ledger,
        &the_notification_reporting("failed", "Background command was killed")
    ));

    let closed = the_only(&snapshot_of(&ledger).shells);
    assert_eq!(closed.state.as_deref(), Some("failed"));
    assert_eq!(closed.exit_code, None);
}

fn tool_call_block(call_line: usize) -> Value {
    fixture_line(SHELL_AND_CHECKLIST_FIXTURE, call_line)["message"]["content"][0].clone()
}

fn tool_named_by(call_line: usize) -> String {
    tool_call_block(call_line)["name"]
        .as_str()
        .unwrap_or_else(|| panic!("{SHELL_AND_CHECKLIST_FIXTURE}:{call_line} names a tool"))
        .to_string()
}

fn feed_tool_call(ledger: &mut SurfaceLedger, call_line: usize) -> bool {
    ledger.read_tool_call(&tool_named_by(call_line), &tool_call_block(call_line))
}

const A_READERS_CLOCK: u64 = 1_788_300_000_000;

fn feed_tool_answer(ledger: &mut SurfaceLedger, call_line: usize, answer: &Value) -> bool {
    feed_tool_answer_at(ledger, call_line, answer, A_READERS_CLOCK)
}

fn feed_tool_answer_at(
    ledger: &mut SurfaceLedger,
    call_line: usize,
    answer: &Value,
    now_ms: u64,
) -> bool {
    let call_id = tool_call_block(call_line)["id"]
        .as_str()
        .unwrap_or_else(|| panic!("{SHELL_AND_CHECKLIST_FIXTURE}:{call_line} carries a call id"))
        .to_string();
    let answered_text = tool_result_text(&answer["message"]["content"][0]);
    ledger.read_tool_answer(
        &tool_named_by(call_line),
        &call_id,
        answer,
        &answered_text,
        now_ms,
    )
}

fn feed_tool_answer_from_the_next_line(ledger: &mut SurfaceLedger, call_line: usize) -> bool {
    let answer = fixture_line(SHELL_AND_CHECKLIST_FIXTURE, call_line + 1);
    feed_tool_answer(ledger, call_line, &answer)
}

fn feed_tool_pair(ledger: &mut SurfaceLedger, call_line: usize) -> bool {
    feed_tool_call(ledger, call_line);
    feed_tool_answer_from_the_next_line(ledger, call_line)
}

fn ledger_through_the_three_creates() -> SurfaceLedger {
    let mut ledger = SurfaceLedger::default();
    for call_line in [27, 29, 31] {
        feed_tool_pair(&mut ledger, call_line);
    }
    ledger
}

fn the_checklist(ledger: &SurfaceLedger) -> Vec<SurfaceChecklistItem> {
    snapshot_of(ledger).checklist
}

fn the_checklist_item(ledger: &SurfaceLedger, id: &str) -> SurfaceChecklistItem {
    the_checklist(ledger)
        .into_iter()
        .find(|item| item.id == id)
        .unwrap_or_else(|| panic!("the checklist holds {id}: {:?}", the_checklist(ledger)))
}

fn todo_write_block(todos: &[(&str, &str)]) -> Value {
    let listed: Vec<Value> = todos
        .iter()
        .map(|(content, status)| {
            json!({
                "content": content,
                "status": status,
                "activeForm": format!("{content}ing"),
            })
        })
        .collect();
    json!({
        "type": "tool_use",
        "id": "toolu_01TodoWriteSynthetic",
        "name": "TodoWrite",
        "input": { "todos": listed },
    })
}

#[test]
fn a_task_create_call_with_no_answer_is_not_yet_a_checklist_item() {
    let mut ledger = SurfaceLedger::default();

    assert!(!feed_tool_call(&mut ledger, 27));

    assert!(ledger.snapshot().is_none());
    assert_eq!(ledger.pending_checklist_creates.len(), 1);
}

#[test]
fn a_task_create_answer_appends_the_item_its_call_described() {
    let mut ledger = SurfaceLedger::default();
    feed_tool_call(&mut ledger, 27);

    assert!(feed_tool_answer_from_the_next_line(&mut ledger, 27));

    assert_eq!(
        the_checklist(&ledger),
        vec![SurfaceChecklistItem {
            id: "1".to_string(),
            subject: "Start the background job".to_string(),
            description: Some(
                "Launch the background bash script with run_in_background: true".to_string()
            ),
            state: Some(ChecklistState::Pending),
        }]
    );
    assert!(ledger.pending_checklist_creates.is_empty());
}

#[test]
fn a_task_create_answer_naming_no_task_id_drains_the_pending_entry_and_mints_nothing() {
    let mut ledger = SurfaceLedger::default();
    feed_tool_call(&mut ledger, 27);
    let mut nameless = fixture_line(SHELL_AND_CHECKLIST_FIXTURE, 28);
    nameless["tool_use_result"]["task"]
        .as_object_mut()
        .expect("the answer carries a task object")
        .remove("id");

    assert!(!ledger.read_tool_answer(
        "TaskCreate",
        FIRST_CREATE_CALL_ID,
        &nameless,
        "",
        A_READERS_CLOCK
    ));

    assert!(ledger.snapshot().is_none());
    assert!(
        ledger.pending_checklist_creates.is_empty(),
        "a malformed answer leaves no pending entry behind"
    );
}

#[test]
fn a_task_create_call_carrying_no_call_id_is_not_held() {
    let mut ledger = SurfaceLedger::default();
    let mut anonymous = tool_call_block(27);
    anonymous
        .as_object_mut()
        .expect("the call is one block")
        .remove("id");

    assert!(!ledger.read_tool_call("TaskCreate", &anonymous));

    assert!(ledger.pending_checklist_creates.is_empty());
}

#[test]
fn a_todo_write_call_carrying_no_list_leaves_the_checklist_alone() {
    let mut ledger = ledger_through_the_three_creates();
    let before = written(&ledger);

    assert!(!ledger.read_tool_call(
            "TodoWrite",
            &json!({ "type": "tool_use", "id": "toolu_01TodoWriteEmpty", "name": "TodoWrite", "input": {} })
        ));

    assert_eq!(written(&ledger), before);
}

#[test]
fn a_turn_that_ended_between_a_create_and_its_answer_holds_nothing_pending() {
    let mut ledger = SurfaceLedger::default();
    feed_tool_call(&mut ledger, 27);
    assert_eq!(ledger.pending_checklist_creates.len(), 1);

    ledger.close_pending_creates();

    assert!(ledger.pending_checklist_creates.is_empty());
    assert!(ledger.snapshot().is_none());
}

#[test]
fn the_three_creates_read_in_the_order_they_were_made() {
    let ledger = ledger_through_the_three_creates();

    let listed: Vec<(String, String)> = the_checklist(&ledger)
        .into_iter()
        .map(|item| (item.id, item.subject))
        .collect();
    assert_eq!(
        listed,
        vec![
            ("1".to_string(), "Start the background job".to_string()),
            ("2".to_string(), "Wait for it".to_string()),
            ("3".to_string(), "Report".to_string()),
        ]
    );
}

#[test]
fn each_task_update_answer_completes_the_item_it_names() {
    let mut ledger = ledger_through_the_three_creates();

    assert!(feed_tool_pair(&mut ledger, 52));
    assert_eq!(
        the_checklist_item(&ledger, "1").state.as_deref(),
        Some("completed")
    );
    assert_eq!(
        the_checklist_item(&ledger, "2").state.as_deref(),
        Some("pending")
    );

    assert!(feed_tool_pair(&mut ledger, 94));
    assert!(feed_tool_pair(&mut ledger, 96));

    for id in ["1", "2", "3"] {
        assert_eq!(
            the_checklist_item(&ledger, id).state.as_deref(),
            Some("completed"),
            "item {id} reads completed"
        );
    }
}

#[test]
fn a_task_update_naming_a_task_that_was_never_created_mints_nothing() {
    let mut ledger = SurfaceLedger::default();

    assert!(!feed_tool_pair(&mut ledger, 52));

    assert!(ledger.snapshot().is_none());
}

#[test]
fn an_unknown_checklist_status_is_preserved_for_diagnostics() {
    let mut ledger = ledger_through_the_three_creates();
    let mut unclaimed = fixture_line(SHELL_AND_CHECKLIST_FIXTURE, 53);
    unclaimed["tool_use_result"]["statusChange"]["to"] = json!("banana");

    assert!(ledger.read_tool_answer(
        "TaskUpdate",
        FIRST_UPDATE_CALL_ID,
        &unclaimed,
        "",
        A_READERS_CLOCK
    ));

    assert_eq!(
        the_checklist_item(&ledger, "1").state.as_deref(),
        Some("banana")
    );
}

#[test]
fn a_tool_the_checklist_never_heard_of_moves_nothing() {
    let mut ledger = ledger_through_the_three_creates();
    let before = written(&ledger);

    for call_line in [17, 92] {
        assert!(!feed_tool_call(&mut ledger, call_line), "line {call_line}");
        assert!(
            !feed_tool_answer_from_the_next_line(&mut ledger, call_line),
            "line {}",
            call_line + 1
        );
    }

    assert_eq!(written(&ledger), before);
}

#[test]
fn a_successful_todo_write_result_replaces_the_whole_checklist() {
    let mut ledger = ledger_through_the_three_creates();
    ledger.claude_checklist = Some(ClaudeChecklist::default());
    let call = todo_write_block(&[
        ("Read the file", "completed"),
        ("Write the test", "in_progress"),
        ("Run the suite", "pending"),
    ]);

    assert!(!ledger.read_tool_call("TodoWrite", &call));
    assert!(written(&ledger).contains("Start the background job"));
    assert!(ledger.read_tool_answer(
        "TodoWrite",
        "toolu_01TodoWriteSynthetic",
        &json!({
            "message": { "content": [{
                "type": "tool_result",
                "tool_use_id": "toolu_01TodoWriteSynthetic",
                "is_error": false,
            }] }
        }),
        "todos updated",
        A_READERS_CLOCK,
    ));

    let listed: Vec<(String, Option<String>)> = the_checklist(&ledger)
        .into_iter()
        .map(|item| {
            (
                item.subject,
                item.state.map(|state| state.as_str().to_string()),
            )
        })
        .collect();
    assert_eq!(
        listed,
        vec![
            ("Read the file".to_string(), Some("completed".to_string())),
            (
                "Write the test".to_string(),
                Some("in_progress".to_string())
            ),
            ("Run the suite".to_string(), Some("pending".to_string())),
        ]
    );
    assert!(
        !written(&ledger).contains("Start the background job"),
        "a wholesale replace drops what the creates minted: {}",
        written(&ledger)
    );
}

fn publish_todos(ledger: &mut SurfaceLedger, call_id: &str, todos: Value) -> bool {
    let call = json!({
        "id": call_id,
        "name": "TodoWrite",
        "input": { "todos": todos },
    });
    assert!(!ledger.read_tool_call("TodoWrite", &call));
    ledger.read_tool_answer(
        "TodoWrite",
        call_id,
        &json!({
            "message": { "content": [{
                "type": "tool_result",
                "tool_use_id": call_id,
                "is_error": false,
            }] }
        }),
        "todos updated",
        A_READERS_CLOCK,
    )
}

fn create_task(ledger: &mut SurfaceLedger, call_id: &str, task_id: &str, subject: &str) -> bool {
    assert!(!ledger.read_tool_call(
        "TaskCreate",
        &json!({
            "id": call_id,
            "input": { "subject": subject, "description": format!("details for {subject}") },
        }),
    ));
    ledger.read_tool_answer(
        "TaskCreate",
        call_id,
        &json!({ "tool_use_result": { "task": { "id": task_id, "subject": subject } } }),
        "task created",
        A_READERS_CLOCK,
    )
}

fn update_task(ledger: &mut SurfaceLedger, task_id: &str, state: &str) -> bool {
    ledger.read_tool_answer(
        "TaskUpdate",
        "update-call",
        &json!({
            "tool_use_result": {
                "taskId": task_id,
                "statusChange": { "to": state },
            }
        }),
        "task updated",
        A_READERS_CLOCK,
    )
}

#[test]
fn todo_duplicate_dedup_includes_omission_count_and_text_truncation() {
    let mut ledger = SurfaceLedger::for_claude(7);
    let prefix: Vec<Value> = (0..CHECKLIST_ITEM_LIMIT)
        .map(|index| json!({ "content": format!("item {index}"), "status": "pending" }))
        .collect();
    let mut first = prefix.clone();
    first.push(json!({ "content": "omitted one", "status": "pending" }));
    assert!(publish_todos(&mut ledger, "count-257", json!(first)));
    assert_eq!(
        ledger
            .snapshot()
            .unwrap()
            .observations
            .checklist
            .unwrap()
            .omitted_count(),
        Some(1)
    );
    let mut second = prefix;
    second.extend([
        json!({ "content": "omitted one", "status": "pending" }),
        json!({ "content": "omitted two", "status": "pending" }),
    ]);
    assert!(publish_todos(&mut ledger, "count-258", json!(second)));
    assert_eq!(
        ledger
            .snapshot()
            .unwrap()
            .observations
            .checklist
            .unwrap()
            .omitted_count(),
        Some(2)
    );

    let exact = "x".repeat(CHECKLIST_TEXT_LIMIT);
    assert!(publish_todos(
        &mut ledger,
        "text-cut",
        json!([{ "content": format!("{exact}x"), "status": "pending" }]),
    ));
    assert_eq!(
        ledger
            .snapshot()
            .unwrap()
            .observations
            .checklist
            .unwrap()
            .coverage(),
        Some(SurfaceCoverage::Partial)
    );
    assert!(publish_todos(
        &mut ledger,
        "text-exact",
        json!([{ "content": exact, "status": "pending" }]),
    ));
    assert_eq!(
        ledger
            .snapshot()
            .unwrap()
            .observations
            .checklist
            .unwrap()
            .coverage(),
        Some(SurfaceCoverage::Complete)
    );
}

#[test]
fn duplicate_todo_success_restores_source_and_freshness_when_needed() {
    let mut ledger = SurfaceLedger::for_claude(7);
    let todos = json!([{ "content": "todo A", "status": "pending" }]);
    assert!(publish_todos(&mut ledger, "todo-a-1", todos.clone()));
    assert!(!publish_todos(
        &mut ledger,
        "todo-a-duplicate",
        todos.clone()
    ));

    assert!(create_task(&mut ledger, "create-b", "task-b", "task B"));
    assert_eq!(ledger.snapshot().unwrap().checklist[0].subject, "task B");
    assert!(publish_todos(&mut ledger, "todo-a-restore", todos.clone()));
    assert_eq!(ledger.snapshot().unwrap().checklist[0].subject, "todo A");

    assert!(ledger.mark_retained_checklist_stale());
    assert!(publish_todos(&mut ledger, "todo-a-refresh", todos));
    assert_eq!(
        ledger
            .snapshot()
            .unwrap()
            .observations
            .checklist
            .unwrap()
            .freshness(),
        Some(SurfaceFreshness::Current)
    );
}

#[test]
fn task_updates_use_the_retained_task_collection_after_todo_display() {
    let mut ledger = SurfaceLedger::for_claude(7);
    assert!(create_task(
        &mut ledger,
        "create-task",
        "todo:0",
        "retained task"
    ));
    assert!(publish_todos(
        &mut ledger,
        "todo",
        json!([{ "content": "displayed todo", "status": "pending" }]),
    ));

    assert!(update_task(&mut ledger, "todo:0", "completed"));

    let item = &ledger.snapshot().unwrap().checklist[0];
    assert_eq!(item.subject, "retained task");
    assert_eq!(item.state.as_deref(), Some("completed"));
    assert_eq!(
        ledger
            .snapshot()
            .unwrap()
            .checklist_provenance
            .unwrap()
            .source,
        ChecklistSource::TaskCreate
    );
}

#[test]
fn ledger_retains_all_live_subagents_and_bounds_cached_terminal_history() {
    let mut ledger = SurfaceLedger::default();
    for index in 0..TERMINAL_EXECUTION_ITEM_LIMIT + 5 {
        let id = format!("done-{index}");
        assert!(ledger.apply_subagent("task_started", &id, &json!({ "description": "done" }),));
        assert!(ledger.close_subagent(&id, Some("completed"), None));
    }
    for index in 0..3 {
        let id = format!("live-{index}");
        assert!(ledger.apply_subagent("task_started", &id, &json!({ "description": "live" }),));
    }

    assert_eq!(ledger.subagents.len(), TERMINAL_EXECUTION_ITEM_LIMIT + 3);
    let snapshot = ledger.snapshot().unwrap();
    assert_eq!(snapshot.subagents.len(), TERMINAL_EXECUTION_ITEM_LIMIT + 3);
    assert!(snapshot.subagents.iter().any(|agent| agent.id == "live-0"));
    assert_eq!(
        snapshot.observations.subagents.unwrap().omitted_count(),
        Some(5)
    );
}

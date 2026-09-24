use std::path::{Path, PathBuf};

use serde_json::Value;
use tempfile::TempDir;

use super::surfaces::{
    AgentSurfaces, SurfaceAgent, SurfaceCoverage, SurfaceObservation, SurfaceObservations,
    SurfacePhase, SurfaceTool, SurfaceWorkflow,
};

pub(crate) const WORKFLOW_FIXTURE: &str = "workflow.jsonl";
pub(crate) const SUBAGENT_FIXTURE: &str = "subagent.jsonl";
pub(crate) const SHELL_AND_CHECKLIST_FIXTURE: &str = "shell-and-checklist.jsonl";

pub(crate) fn fixture_text(file_name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/claude-stream")
        .join(file_name);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|why| panic!("the {file_name} fixture reads: {why}"))
}

pub(crate) fn fixture_lines(file_name: &str) -> Vec<String> {
    fixture_text(file_name)
        .lines()
        .map(str::to_string)
        .collect()
}

pub(crate) fn fixture_events(file_name: &str) -> Vec<Value> {
    fixture_text(file_name)
        .lines()
        .enumerate()
        .map(|(position, line)| {
            serde_json::from_str(line).unwrap_or_else(|why| {
                panic!("{file_name}:{} is one JSON event: {why}", position + 1)
            })
        })
        .collect()
}

pub(crate) fn fixture_line(file_name: &str, line_number: usize) -> Value {
    let events = fixture_events(file_name);
    events
        .get(line_number - 1)
        .unwrap_or_else(|| panic!("{file_name} has a line {line_number}"))
        .clone()
}

pub(crate) const SHELL_TASK_ID: &str = "bn93ge6bt";
pub(crate) const SHELL_LAUNCH_CALL_ID: &str = "toolu_018xEixjjwkZczgWKGqQPat6";
pub(crate) const SHELL_LAUNCH_CALL_LINE: usize = 40;
pub(crate) const SHELL_STARTED_LINE: usize = 42;
pub(crate) const SHELL_LAUNCH_ANSWER_LINE: usize = 43;
pub(crate) const SHELL_LAUNCHED_AT_MS: u64 = 1788291725678;
pub(crate) const SHELL_UPDATED_LINE: usize = 83;
pub(crate) const SHELL_NOTIFICATION_LINE: usize = 84;
pub(crate) const SHELL_OUTPUT_PATH: &str = "/private/tmp/claude-501/-private-tmp-claude-501--Users-adam--superconductor-worktrees-Build-sc-trapped-dewar-4eba-61c19380-be59-489a-9244-b8f732217cc1-scratchpad-probe/fb1738ea-687c-4b18-b494-944dc64dda8c/tasks/bn93ge6bt.output";

pub(crate) fn shell_output_file_holding(text: &str) -> (TempDir, PathBuf) {
    let directory = tempfile::tempdir().expect("a temp directory");
    let path = directory.path().join("shell.output");
    std::fs::write(&path, text).expect("the output file writes");
    (directory, path)
}

pub(crate) fn hundred_numbered_lines() -> String {
    (1..=100)
        .map(|number| format!("line {number}\n"))
        .collect::<String>()
}

pub(crate) const WORKFLOW_SPAWNING_CALL_ID: &str = "toolu_01TPFUY53rBEJPmkKu7rJPWV";
pub(crate) const SUBAGENT_SPAWNING_CALL_ID: &str = "toolu_01P8eCnYQFMqdCaXBXSCcAVd";
pub(crate) const WORKFLOW_TASK_ID: &str = "w81x1fmx5";
pub(crate) const SUBAGENT_TASK_ID: &str = "aba8d0dbf79bd05f1";
pub(crate) const LINE_COUNTER_AGENT_ID: &str = "acdd7854c4bce379a";
pub(crate) const CHAR_COUNTER_AGENT_ID: &str = "a1a79b6791abd41ee";
pub(crate) const SUMMARIZER_AGENT_ID: &str = "abecba7acf45aac98";
pub(crate) const FIRST_CREATE_CALL_ID: &str = "toolu_01V6RPmcsmyRyEVKSdcpKTMJ";
pub(crate) const FIRST_UPDATE_CALL_ID: &str = "toolu_01UExMFQFbhqwFX9Qz4M3L1q";

const WORKFLOW_LAST_TOOL_SUMMARY: &str =
    "/private/tmp/claude-501/-Users-adam--superconductor-worktre…";

pub(crate) fn recorded_workflow_surfaces() -> AgentSurfaces {
    AgentSurfaces {
        observations: SurfaceObservations {
            workflows: Some(SurfaceObservation::current_unstamped(
                SurfaceCoverage::Partial,
            )),
            subagents: Some(SurfaceObservation::current_unstamped(
                SurfaceCoverage::Partial,
            )),
            ..SurfaceObservations::default()
        },
        workflows: vec![SurfaceWorkflow {
            id: WORKFLOW_TASK_ID.to_string(),
            name: "readme-analysis".to_string(),
            description: Some("Count README.md lines and characters, then summarize".to_string()),
            state: Some("running".to_string()),
            phases: vec![
                SurfacePhase {
                    title: "Read".to_string(),
                    agents: vec![
                        recorded_workflow_agent(
                            LINE_COUNTER_AGENT_ID,
                            "line-counter",
                            1_788_290_134_700,
                            4_732,
                            11_409,
                            "2",
                        ),
                        recorded_workflow_agent(
                            CHAR_COUNTER_AGENT_ID,
                            "char-counter",
                            1_788_290_134_700,
                            34_926,
                            11_324,
                            "4",
                        ),
                    ],
                },
                SurfacePhase {
                    title: "Summarize".to_string(),
                    agents: vec![recorded_workflow_agent(
                        SUMMARIZER_AGENT_ID,
                        "summarizer",
                        1_788_290_170_370,
                        7_689,
                        11_381,
                        "The README.md file contains only \"hi\" — there are no statistics to summarize.",
                    )],
                },
            ],
        }],
        subagents: vec![SurfaceAgent {
            id: SUBAGENT_TASK_ID.to_string(),
            label: "Read README.md and report character count".to_string(),
            state: Some("done".to_string()),
            result: Some("4".to_string()),
            spawning_call_id: Some(SUBAGENT_SPAWNING_CALL_ID.to_string()),
            ..SurfaceAgent::default()
        }],
        ..AgentSurfaces::default()
    }
}

pub(crate) fn the_line_counter_carrying_a_spawning_call_id() -> SurfaceAgent {
    let recorded = recorded_workflow_surfaces();
    let line_counter = recorded.workflows[0].phases[0].agents[0].clone();
    SurfaceAgent {
        spawning_call_id: Some(WORKFLOW_SPAWNING_CALL_ID.to_string()),
        ..line_counter
    }
}

fn recorded_workflow_agent(
    id: &str,
    label: &str,
    started_at: u64,
    duration_ms: u64,
    tokens: u64,
    result: &str,
) -> SurfaceAgent {
    SurfaceAgent {
        id: id.to_string(),
        label: label.to_string(),
        model: Some("claude-haiku-4-5-20251001".to_string()),
        state: Some("done".to_string()),
        started_at: Some(started_at),
        duration_ms: Some(duration_ms),
        tokens: Some(tokens),
        tool_calls: Some(1),
        last_tool: Some(SurfaceTool {
            name: "Read".to_string(),
            summary: Some(WORKFLOW_LAST_TOOL_SUMMARY.to_string()),
        }),
        result: Some(result.to_string()),
        attempt: Some(1),
        ..SurfaceAgent::default()
    }
}

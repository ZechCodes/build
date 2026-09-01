use std::path::Path;

use serde_json::Value;

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

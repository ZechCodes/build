use std::path::PathBuf;

use serde_json::Value;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use super::claude_checklist::ClaudeTodo;
use super::ledger::SubagentChoice;
use super::*;
use crate::harness::adk::{
    one_line, task_status_failed, task_status_is_terminal, TOOL_SUMMARY_LIMIT,
};
use crate::harness::shell_tail::{exit_code_stated_in, ShellTail};

pub(super) const RUNNING: &str = "running";
pub(super) const DONE: &str = "done";
pub(super) const FAILED: &str = "failed";
pub(super) const QUEUED: &str = "queued";
pub(super) const OUTPUT_PATH_PREAMBLE: &str = "Output is being written to: ";
pub(super) const EXIT_CODE_PREAMBLE: &str = "exit code ";

pub(super) enum ShellReport {
    Started {
        description: Option<String>,
    },
    Launched {
        output_path: PathBuf,
        started_at: u64,
    },
    Closed {
        state: &'static str,
        exit_code: Option<i32>,
    },
    StatusChanged {
        state: &'static str,
    },
    Tailed(ShellTail),
}
pub(super) fn bounded_terminal_entries<T: Clone>(
    entries: &[T],
    terminal: impl Fn(&T) -> bool,
) -> (Vec<T>, usize) {
    let terminal_count = entries.iter().filter(|entry| terminal(entry)).count();
    let omitted = terminal_count.saturating_sub(TERMINAL_EXECUTION_ITEM_LIMIT);
    if omitted == 0 {
        return (entries.to_vec(), 0);
    }
    let mut terminals_to_skip = omitted;
    let retained = entries
        .iter()
        .filter(|entry| {
            if terminal(entry) && terminals_to_skip > 0 {
                terminals_to_skip -= 1;
                false
            } else {
                true
            }
        })
        .cloned()
        .collect();
    (retained, omitted)
}

pub(super) fn trim_terminal_entries<T>(
    entries: &mut Vec<T>,
    terminal: impl Fn(&T) -> bool,
) -> usize {
    let terminal_count = entries.iter().filter(|entry| terminal(entry)).count();
    let omitted = terminal_count.saturating_sub(TERMINAL_EXECUTION_ITEM_LIMIT);
    if omitted == 0 {
        return 0;
    }
    let mut terminals_to_remove = omitted;
    entries.retain(|entry| {
        if terminal(entry) && terminals_to_remove > 0 {
            terminals_to_remove -= 1;
            false
        } else {
            true
        }
    });
    omitted
}

pub(super) fn execution_observation<T>(
    entries: &[T],
    omitted: usize,
) -> Option<SurfaceObservation> {
    (!entries.is_empty()).then(|| {
        SurfaceObservation::current_unstamped(SurfaceCoverage::Partial).with_omitted_count(omitted)
    })
}

pub(super) fn status_reported_by<'event>(
    subtype: &str,
    event: &'event Value,
) -> Option<&'event str> {
    match subtype {
        "task_updated" => event["patch"]["status"].as_str(),
        "task_notification" => event["status"].as_str(),
        _ => None,
    }
}

pub(super) fn shell_close_reported_by(subtype: &str, event: &Value) -> Option<ShellReport> {
    let state = status_reported_by(subtype, event).and_then(wire_task_state)?;
    match subtype {
        "task_notification" => Some(ShellReport::Closed {
            state,
            exit_code: event["summary"]
                .as_str()
                .and_then(|summary| exit_code_stated_in(summary, EXIT_CODE_PREAMBLE)),
        }),
        _ => Some(ShellReport::StatusChanged { state }),
    }
}

pub(super) fn stamped_at(event: &Value) -> Option<u64> {
    let written = event["timestamp"].as_str()?;
    let stamped = OffsetDateTime::parse(written, &Rfc3339).ok()?;
    u64::try_from(stamped.unix_timestamp_nanos() / 1_000_000).ok()
}

pub(super) fn output_path_named_in(answered: &str) -> Option<PathBuf> {
    let (_, after_the_preamble) = answered.split_once(OUTPUT_PATH_PREAMBLE)?;
    let named = after_the_preamble
        .split_whitespace()
        .next()?
        .trim_end_matches('.');
    match named.is_empty() {
        true => None,
        false => Some(PathBuf::from(named)),
    }
}

pub(super) fn todo_item(position: usize, todo: ClaudeTodo) -> SurfaceChecklistItem {
    SurfaceChecklistItem {
        id: format!("todo:{position}"),
        subject: todo.content,
        description: todo.active_form,
        state: Some(ChecklistState::from_provider(&todo.state)),
    }
}

pub(super) fn tool_result_succeeded(event: &Value, call_id: &str) -> bool {
    event["message"]["content"]
        .as_array()
        .and_then(|blocks| {
            blocks.iter().find(|block| {
                block["type"].as_str() == Some("tool_result")
                    && block["tool_use_id"].as_str() == Some(call_id)
            })
        })
        .is_some_and(|block| block["is_error"].as_bool() != Some(true))
}

pub(super) fn receipt_time() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .expect("the current time formats as RFC 3339")
}

pub(super) fn started_subagent(
    held: &SurfaceAgent,
    task_id: &str,
    event: &Value,
    spawn_choice: Option<&SubagentChoice>,
) -> SurfaceAgent {
    SurfaceAgent {
        id: task_id.to_string(),
        label: bounded_text(event, "description").unwrap_or_default(),
        model: bounded_text(event, "model")
            .or_else(|| held.model.clone())
            .or_else(|| spawn_choice.and_then(|choice| choice.model.clone())),
        reasoning_effort: bounded_text(event, "reasoningEffort")
            .or_else(|| bounded_text(event, "reasoning_effort"))
            .or_else(|| bounded_text(event, "effort"))
            .or_else(|| held.reasoning_effort.clone())
            .or_else(|| spawn_choice.and_then(|choice| choice.reasoning_effort.clone())),
        state: Some(RUNNING.to_string()),
        spawning_call_id: read_text(event, "tool_use_id"),
        ..held.clone()
    }
}

pub(super) fn progressed_subagent(held: &SurfaceAgent, event: &Value) -> SurfaceAgent {
    let usage = &event["usage"];
    SurfaceAgent {
        last_tool: bounded_text(event, "last_tool_name")
            .map(|name| SurfaceTool {
                name,
                summary: bounded_text(event, "description"),
            })
            .or_else(|| held.last_tool.clone()),
        tokens: usage["total_tokens"].as_u64().or(held.tokens),
        tool_calls: usage["tool_uses"].as_u64().or(held.tool_calls),
        duration_ms: usage["duration_ms"].as_u64().or(held.duration_ms),
        ..held.clone()
    }
}

pub(super) fn replace_when_changed<T: PartialEq>(held: &mut T, reported: T) -> bool {
    match *held == reported {
        true => false,
        false => {
            *held = reported;
            true
        }
    }
}

pub(super) trait SurfaceEntry {
    fn entry_id(&self) -> &str;
}

impl SurfaceEntry for SurfaceWorkflow {
    fn entry_id(&self) -> &str {
        &self.id
    }
}

impl SurfaceEntry for SurfaceAgent {
    fn entry_id(&self) -> &str {
        &self.id
    }
}

impl SurfaceEntry for SurfaceShell {
    fn entry_id(&self) -> &str {
        &self.id
    }
}

pub(super) fn held_named<'held, T: SurfaceEntry>(
    held: &'held mut [T],
    id: &str,
) -> Option<&'held mut T> {
    held.iter_mut().find(|entry| entry.entry_id() == id)
}

pub(super) fn upsert_by_id<T: SurfaceEntry + PartialEq>(held: &mut Vec<T>, reported: T) -> bool {
    match held
        .iter()
        .position(|entry| entry.entry_id() == reported.entry_id())
    {
        Some(position) => replace_when_changed(&mut held[position], reported),
        None => {
            held.push(reported);
            true
        }
    }
}

pub(super) fn read_text(source: &Value, field: &str) -> Option<String> {
    source[field].as_str().map(str::to_string)
}

pub(super) fn bounded_text(source: &Value, field: &str) -> Option<String> {
    read_text(source, field).map(|written| one_line(&written, TOOL_SUMMARY_LIMIT))
}

pub(super) fn is_progress_entry(entry: &Value, entry_type: &str) -> bool {
    entry["type"].as_str() == Some(entry_type)
}

pub(super) fn read_workflow_phases(task_id: &str, entries: &[Value]) -> Vec<SurfacePhase> {
    let mut phases: Vec<(u64, SurfacePhase)> = entries
        .iter()
        .filter(|entry| is_progress_entry(entry, "workflow_phase"))
        .enumerate()
        .map(|(phase_ordinal, entry)| {
            (
                entry["index"].as_u64().unwrap_or(phase_ordinal as u64),
                SurfacePhase {
                    title: bounded_text(entry, "title").unwrap_or_default(),
                    agents: Vec::new(),
                },
            )
        })
        .collect();

    for (position, entry) in entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| is_progress_entry(entry, "workflow_agent"))
    {
        let phase_index = entry["phaseIndex"].as_u64();
        let phase_title = bounded_text(entry, "phaseTitle");
        let landing = phase_index
            .and_then(|wanted| phases.iter().position(|(index, _)| *index == wanted))
            .or_else(|| {
                phase_title
                    .as_ref()
                    .and_then(|wanted| phases.iter().position(|(_, phase)| &phase.title == wanted))
            })
            .unwrap_or_else(|| {
                phases.push((
                    phase_index.unwrap_or_default(),
                    SurfacePhase {
                        title: phase_title.unwrap_or_default(),
                        agents: Vec::new(),
                    },
                ));
                phases.len() - 1
            });
        phases[landing]
            .1
            .agents
            .push(read_workflow_agent(task_id, position as u64, entry));
    }

    phases.into_iter().map(|(_, phase)| phase).collect()
}

pub(super) fn read_workflow_agent(task_id: &str, position: u64, entry: &Value) -> SurfaceAgent {
    let index = entry["index"].as_u64().unwrap_or(position);
    let started_at = entry["startedAt"].as_u64();
    SurfaceAgent {
        id: read_text(entry, "agentId").unwrap_or_else(|| format!("{task_id}:{index}")),
        label: bounded_text(entry, "label").unwrap_or_default(),
        description: None,
        model: bounded_text(entry, "model"),
        reasoning_effort: bounded_text(entry, "reasoningEffort")
            .or_else(|| bounded_text(entry, "reasoning_effort"))
            .or_else(|| bounded_text(entry, "effort")),
        state: entry["state"]
            .as_str()
            .and_then(|token| wire_agent_state(token, started_at.is_some()))
            .map(str::to_string),
        started_at,
        duration_ms: entry["durationMs"].as_u64(),
        tokens: entry["tokens"].as_u64(),
        tool_calls: entry["toolCalls"].as_u64(),
        last_tool: bounded_text(entry, "lastToolName").map(|name| SurfaceTool {
            name,
            summary: bounded_text(entry, "lastToolSummary"),
        }),
        result: bounded_text(entry, "resultPreview"),
        error: bounded_text(entry, "error"),
        attempt: entry["attempt"].as_u64().map(|attempt| attempt as u32),
        spawning_call_id: None,
    }
}

pub(super) fn shell_state_for(
    exit_code: Option<i32>,
    reported_state: &'static str,
) -> &'static str {
    match exit_code {
        Some(code) if code != 0 => FAILED,
        _ => reported_state,
    }
}

pub(super) fn wire_task_state(status: &str) -> Option<&'static str> {
    match task_status_failed(status) {
        true => Some(FAILED),
        false => match task_status_is_terminal(status) {
            true => Some(DONE),
            false => None,
        },
    }
}

pub(super) fn wire_agent_state(token: &str, has_started_at: bool) -> Option<&'static str> {
    match token {
        "start" | "progress" => match has_started_at {
            true => Some(RUNNING),
            false => Some(QUEUED),
        },
        DONE => Some(DONE),
        _ => None,
    }
}

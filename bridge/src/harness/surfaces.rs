use std::collections::HashMap;
use std::path::PathBuf;

use serde::Serialize;
use serde_json::{json, Value};

use super::adk::{one_line, task_status_failed, task_status_is_terminal, TOOL_SUMMARY_LIMIT};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct AgentSurfaces {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub workflows: Vec<SurfaceWorkflow>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub subagents: Vec<SurfaceAgent>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub shells: Vec<SurfaceShell>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub checklist: Vec<SurfaceChecklistItem>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct SurfaceWorkflow {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub phases: Vec<SurfacePhase>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct SurfacePhase {
    pub title: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub agents: Vec<SurfaceAgent>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct SurfaceAgent {
    pub id: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_tool: Option<SurfaceTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attempt: Option<u32>,
    #[serde(skip)]
    pub spawning_call_id: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct SurfaceTool {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct SurfaceShell {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tail: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct SurfaceChecklistItem {
    pub id: String,
    pub subject: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
}

impl AgentSurfaces {
    pub fn is_empty(&self) -> bool {
        self.workflows.is_empty()
            && self.subagents.is_empty()
            && self.shells.is_empty()
            && self.checklist.is_empty()
    }

    pub fn wire_value(&self, call_sequence_of: &dyn Fn(&str) -> Option<u64>) -> Value {
        let mut written =
            serde_json::to_value(self).expect("a surfaces snapshot serializes to an object");
        let spawning_sequences = self.subagents.iter().map(|subagent| {
            subagent
                .spawning_call_id
                .as_deref()
                .and_then(call_sequence_of)
        });
        if let Some(entries) = written.get_mut("subagents").and_then(Value::as_array_mut) {
            for (entry, spawning_sequence) in entries.iter_mut().zip(spawning_sequences) {
                match (entry.as_object_mut(), spawning_sequence) {
                    (Some(fields), Some(sequence)) => {
                        fields.insert("call_sequence".to_string(), json!(sequence));
                    }
                    _ => continue,
                }
            }
        }
        written
    }
}

#[derive(Debug, Default)]
pub struct SurfaceLedger {
    workflows: Vec<SurfaceWorkflow>,
    subagents: Vec<SurfaceAgent>,
    shells: Vec<SurfaceShell>,
    checklist: Vec<SurfaceChecklistItem>,
    shell_outputs: HashMap<String, PathBuf>,
    pending_checklist_creates: HashMap<String, PendingChecklistCreate>,
}

const SHELL_RUNNING: &str = "running";

#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingChecklistCreate {
    subject: String,
    description: Option<String>,
}

impl SurfaceLedger {
    pub fn read_task_event(&mut self, subtype: &str, event: &Value) -> bool {
        let task_id = match event["task_id"].as_str() {
            Some(named) => named.to_string(),
            None => return false,
        };
        match subtype {
            "task_started" => match event["task_type"].as_str() {
                Some("local_workflow") => self.apply_workflow(subtype, &task_id, event),
                Some("local_agent") => self.apply_subagent(subtype, &task_id, event),
                _ => false,
            },
            "task_progress" | "task_updated" | "task_notification" => {
                match (self.holds_workflow(&task_id), self.holds_subagent(&task_id)) {
                    (true, _) => self.apply_workflow(subtype, &task_id, event),
                    (_, true) => self.apply_subagent(subtype, &task_id, event),
                    _ => false,
                }
            }
            _ => false,
        }
    }

    pub fn read_tool_call(&mut self, name: &str, tool_use_block: &Value) -> bool {
        self.apply_checklist_call(name, tool_use_block)
    }

    pub fn read_tool_answer(&mut self, tool: &str, call_id: &str, whole_event: &Value) -> bool {
        self.apply_checklist_answer(tool, call_id, whole_event)
    }

    pub fn running_shell_outputs(&self) -> Vec<(String, PathBuf)> {
        self.shells
            .iter()
            .filter(|shell| shell.state.as_deref() == Some(SHELL_RUNNING))
            .filter_map(|shell| {
                self.shell_outputs
                    .get(&shell.id)
                    .map(|output_path| (shell.id.clone(), output_path.clone()))
            })
            .collect()
    }

    pub fn close_pending_creates(&mut self) {
        self.pending_checklist_creates.clear();
    }

    pub fn snapshot(&self) -> Option<AgentSurfaces> {
        let held = AgentSurfaces {
            workflows: self.workflows.clone(),
            subagents: self.subagents.clone(),
            shells: self.shells.clone(),
            checklist: self.checklist.clone(),
        };
        match held.is_empty() {
            true => None,
            false => Some(held),
        }
    }

    fn holds_workflow(&self, task_id: &str) -> bool {
        self.workflows.iter().any(|held| held.id == task_id)
    }

    fn apply_workflow(&mut self, subtype: &str, task_id: &str, event: &Value) -> bool {
        match subtype {
            "task_started" => {
                let started = SurfaceWorkflow {
                    id: task_id.to_string(),
                    name: read_text(event, "workflow_name").unwrap_or_default(),
                    description: read_text(event, "description"),
                    state: Some("running".to_string()),
                    phases: Vec::new(),
                };
                match self.workflow_named(task_id) {
                    Some(held) => replace_when_changed(held, started),
                    None => {
                        self.workflows.push(started);
                        true
                    }
                }
            }
            "task_progress" => {
                let reported = match event["workflow_progress"].as_array() {
                    Some(entries) => read_workflow_phases(task_id, entries),
                    None => return false,
                };
                match self.workflow_named(task_id) {
                    Some(held) => replace_when_changed(&mut held.phases, reported),
                    None => false,
                }
            }
            "task_updated" => self.close_workflow(task_id, event["patch"]["status"].as_str()),
            "task_notification" => self.close_workflow(task_id, event["status"].as_str()),
            _ => false,
        }
    }

    fn close_workflow(&mut self, task_id: &str, status: Option<&str>) -> bool {
        let closed = match status.and_then(wire_task_state) {
            Some(state) => Some(state.to_string()),
            None => return false,
        };
        match self.workflow_named(task_id) {
            Some(held) => replace_when_changed(&mut held.state, closed),
            None => false,
        }
    }

    fn workflow_named(&mut self, task_id: &str) -> Option<&mut SurfaceWorkflow> {
        self.workflows.iter_mut().find(|held| held.id == task_id)
    }

    fn holds_subagent(&self, task_id: &str) -> bool {
        self.subagents.iter().any(|held| held.id == task_id)
    }

    fn apply_subagent(&mut self, subtype: &str, task_id: &str, event: &Value) -> bool {
        match subtype {
            "task_started" => match self.subagent_named(task_id) {
                Some(held) => {
                    let restarted = started_subagent(held, task_id, event);
                    replace_when_changed(held, restarted)
                }
                None => {
                    let started = started_subagent(&SurfaceAgent::default(), task_id, event);
                    self.subagents.push(started);
                    true
                }
            },
            "task_progress" => match self.subagent_named(task_id) {
                Some(held) => {
                    let progressed = progressed_subagent(held, event);
                    replace_when_changed(held, progressed)
                }
                None => false,
            },
            "task_updated" => self.close_subagent(task_id, event["patch"]["status"].as_str(), None),
            "task_notification" => self.close_subagent(
                task_id,
                event["status"].as_str(),
                bounded_text(event, "summary"),
            ),
            _ => false,
        }
    }

    fn close_subagent(
        &mut self,
        task_id: &str,
        status: Option<&str>,
        summary: Option<String>,
    ) -> bool {
        let claimed = match status.and_then(wire_task_state) {
            Some(state) => state,
            None => return false,
        };
        match self.subagent_named(task_id) {
            Some(held) => {
                let closed = SurfaceAgent {
                    state: Some(claimed.to_string()),
                    result: summary.or_else(|| held.result.clone()),
                    ..held.clone()
                };
                replace_when_changed(held, closed)
            }
            None => false,
        }
    }

    fn subagent_named(&mut self, task_id: &str) -> Option<&mut SurfaceAgent> {
        self.subagents.iter_mut().find(|held| held.id == task_id)
    }

    fn apply_checklist_call(&mut self, tool: &str, block: &Value) -> bool {
        match tool {
            "TaskCreate" => {
                let call_id = match block["id"].as_str() {
                    Some(named) => named.to_string(),
                    None => return false,
                };
                let input = &block["input"];
                self.pending_checklist_creates.insert(
                    call_id,
                    PendingChecklistCreate {
                        subject: bounded_text(input, "subject").unwrap_or_default(),
                        description: bounded_text(input, "description"),
                    },
                );
                false
            }
            "TodoWrite" => match block["input"]["todos"].as_array() {
                Some(todos) => replace_when_changed(&mut self.checklist, read_todo_list(todos)),
                None => false,
            },
            _ => false,
        }
    }

    fn apply_checklist_answer(&mut self, tool: &str, call_id: &str, event: &Value) -> bool {
        match tool {
            "TaskCreate" => {
                self.drain_pending_create_and_append(call_id, &event["tool_use_result"]["task"])
            }
            "TaskUpdate" => self.apply_status_change(&event["tool_use_result"]),
            _ => false,
        }
    }

    fn drain_pending_create_and_append(&mut self, call_id: &str, task: &Value) -> bool {
        let described = self.pending_checklist_creates.remove(call_id);
        let id = match task["id"].as_str() {
            Some(named) => named.to_string(),
            None => return false,
        };
        let created = SurfaceChecklistItem {
            id,
            subject: bounded_text(task, "subject")
                .or_else(|| described.as_ref().map(|pending| pending.subject.clone()))
                .unwrap_or_default(),
            description: described.and_then(|pending| pending.description),
            state: Some(CHECKLIST_PENDING.to_string()),
        };
        match self.checklist_item_named(&created.id) {
            Some(held) => replace_when_changed(held, created),
            None => {
                self.checklist.push(created);
                true
            }
        }
    }

    fn apply_status_change(&mut self, answered: &Value) -> bool {
        let restated = match answered["statusChange"]["to"]
            .as_str()
            .and_then(wire_checklist_state)
        {
            Some(claimed) => Some(claimed.to_string()),
            None => return false,
        };
        let task_id = match answered["taskId"].as_str() {
            Some(named) => named.to_string(),
            None => return false,
        };
        match self.checklist_item_named(&task_id) {
            Some(held) => replace_when_changed(&mut held.state, restated),
            None => false,
        }
    }

    fn checklist_item_named(&mut self, id: &str) -> Option<&mut SurfaceChecklistItem> {
        self.checklist.iter_mut().find(|held| held.id == id)
    }
}

fn read_todo_list(todos: &[Value]) -> Vec<SurfaceChecklistItem> {
    todos
        .iter()
        .enumerate()
        .map(|(position, todo)| SurfaceChecklistItem {
            id: format!("todo:{position}"),
            subject: bounded_text(todo, "content").unwrap_or_default(),
            description: None,
            state: todo["status"]
                .as_str()
                .and_then(wire_checklist_state)
                .map(str::to_string),
        })
        .collect()
}

fn started_subagent(held: &SurfaceAgent, task_id: &str, event: &Value) -> SurfaceAgent {
    SurfaceAgent {
        id: task_id.to_string(),
        label: read_text(event, "description").unwrap_or_default(),
        state: Some("running".to_string()),
        spawning_call_id: read_text(event, "tool_use_id"),
        ..held.clone()
    }
}

fn progressed_subagent(held: &SurfaceAgent, event: &Value) -> SurfaceAgent {
    let usage = &event["usage"];
    SurfaceAgent {
        last_tool: read_text(event, "last_tool_name")
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

fn replace_when_changed<T: PartialEq>(held: &mut T, reported: T) -> bool {
    match *held == reported {
        true => false,
        false => {
            *held = reported;
            true
        }
    }
}

fn read_text(source: &Value, field: &str) -> Option<String> {
    source[field].as_str().map(str::to_string)
}

fn bounded_text(source: &Value, field: &str) -> Option<String> {
    read_text(source, field).map(|written| one_line(&written, TOOL_SUMMARY_LIMIT))
}

fn is_progress_entry(entry: &Value, entry_type: &str) -> bool {
    entry["type"].as_str() == Some(entry_type)
}

fn read_workflow_phases(task_id: &str, entries: &[Value]) -> Vec<SurfacePhase> {
    let mut phases: Vec<(u64, SurfacePhase)> = entries
        .iter()
        .filter(|entry| is_progress_entry(entry, "workflow_phase"))
        .enumerate()
        .map(|(phase_ordinal, entry)| {
            (
                entry["index"].as_u64().unwrap_or(phase_ordinal as u64),
                SurfacePhase {
                    title: read_text(entry, "title").unwrap_or_default(),
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
        let phase_title = read_text(entry, "phaseTitle");
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

fn read_workflow_agent(task_id: &str, position: u64, entry: &Value) -> SurfaceAgent {
    let index = entry["index"].as_u64().unwrap_or(position);
    let started_at = entry["startedAt"].as_u64();
    SurfaceAgent {
        id: read_text(entry, "agentId").unwrap_or_else(|| format!("{task_id}:{index}")),
        label: read_text(entry, "label").unwrap_or_default(),
        model: read_text(entry, "model"),
        state: entry["state"]
            .as_str()
            .and_then(|token| wire_agent_state(token, started_at.is_some()))
            .map(str::to_string),
        started_at,
        duration_ms: entry["durationMs"].as_u64(),
        tokens: entry["tokens"].as_u64(),
        tool_calls: entry["toolCalls"].as_u64(),
        last_tool: read_text(entry, "lastToolName").map(|name| SurfaceTool {
            name,
            summary: read_text(entry, "lastToolSummary"),
        }),
        result: read_text(entry, "resultPreview"),
        error: read_text(entry, "error"),
        attempt: entry["attempt"].as_u64().map(|attempt| attempt as u32),
        spawning_call_id: None,
    }
}

fn wire_task_state(status: &str) -> Option<&'static str> {
    match task_status_failed(status) {
        true => Some("failed"),
        false => match task_status_is_terminal(status) {
            true => Some("done"),
            false => None,
        },
    }
}

const CHECKLIST_PENDING: &str = "pending";

fn wire_checklist_state(token: &str) -> Option<&'static str> {
    match token {
        CHECKLIST_PENDING => Some(CHECKLIST_PENDING),
        "in_progress" => Some("in_progress"),
        "completed" => Some("completed"),
        "blocked" => Some("blocked"),
        _ => None,
    }
}

fn wire_agent_state(token: &str, has_started_at: bool) -> Option<&'static str> {
    match token {
        "start" | "progress" => match has_started_at {
            true => Some("running"),
            false => Some("queued"),
        },
        "done" => Some("done"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::stream_fixtures::{
        fixture_events, fixture_line, SHELL_AND_CHECKLIST_FIXTURE, SUBAGENT_FIXTURE,
        WORKFLOW_FIXTURE,
    };

    fn line_counter_from_the_workflow_fixture() -> SurfaceAgent {
        SurfaceAgent {
            id: "acdd7854c4bce379a".to_string(),
            label: "line-counter".to_string(),
            model: Some("claude-haiku-4-5-20251001".to_string()),
            state: Some("done".to_string()),
            started_at: Some(1_788_290_134_700),
            duration_ms: Some(4_732),
            tokens: Some(11_409),
            tool_calls: Some(1),
            last_tool: Some(SurfaceTool {
                name: "Read".to_string(),
                summary: Some(
                    "/private/tmp/claude-501/-Users-zech--superconductor-worktre…".to_string(),
                ),
            }),
            result: Some("2".to_string()),
            error: None,
            attempt: Some(1),
            spawning_call_id: Some("toolu_01TPFUY53rBEJPmkKu7rJPWV".to_string()),
        }
    }

    fn one_checklist_item() -> SurfaceChecklistItem {
        SurfaceChecklistItem {
            id: "task-1".to_string(),
            subject: "Count the lines".to_string(),
            description: Some("Read README.md and count".to_string()),
            state: Some("in_progress".to_string()),
        }
    }

    fn one_shell() -> SurfaceShell {
        SurfaceShell {
            id: "bash-1".to_string(),
            description: Some("run the suite".to_string()),
            state: Some(SHELL_RUNNING.to_string()),
            exit_code: None,
            tail: vec!["test one ... ok".to_string()],
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
            subagents: vec![line_counter_from_the_workflow_fixture()],
            ..AgentSurfaces::default()
        };
        let answers_forty_one = |spawning_call_id: &str| {
            assert_eq!(spawning_call_id, "toolu_01TPFUY53rBEJPmkKu7rJPWV");
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
                id: "w81x1fmx5".to_string(),
                name: "count-and-summarize".to_string(),
                description: Some("Count README.md lines".to_string()),
                state: Some("running".to_string()),
                phases: vec![SurfacePhase {
                    title: "Read".to_string(),
                    agents: vec![line_counter_from_the_workflow_fixture()],
                }],
            }],
            subagents: vec![line_counter_from_the_workflow_fixture()],
            shells: vec![one_shell()],
            checklist: vec![one_checklist_item()],
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

    const WORKFLOW_TASK_ID: &str = "w81x1fmx5";

    fn feed(ledger: &mut SurfaceLedger, event: &Value) -> bool {
        let subtype = event["subtype"]
            .as_str()
            .expect("every fixture system line names a subtype")
            .to_string();
        ledger.read_task_event(&subtype, event)
    }

    fn feed_workflow_line(ledger: &mut SurfaceLedger, line_number: usize) -> bool {
        feed(ledger, &fixture_line(WORKFLOW_FIXTURE, line_number))
    }

    fn written(ledger: &SurfaceLedger) -> String {
        ledger
            .snapshot()
            .expect("the ledger holds a snapshot")
            .wire_value(&no_call_sequence)
            .to_string()
    }

    fn the_only_workflow(ledger: &SurfaceLedger) -> SurfaceWorkflow {
        let snapshot = ledger.snapshot().expect("the ledger holds a snapshot");
        assert_eq!(snapshot.workflows.len(), 1, "{snapshot:?}");
        snapshot.workflows[0].clone()
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
            feed_workflow_line(&mut ledger, line_number);
        }
        ledger
    }

    #[test]
    fn a_ledger_that_has_read_nothing_holds_no_snapshot() {
        assert!(SurfaceLedger::default().snapshot().is_none());
    }

    #[test]
    fn a_started_local_workflow_opens_a_running_workflow() {
        let mut ledger = SurfaceLedger::default();

        assert!(feed_workflow_line(&mut ledger, 37));

        let workflow = the_only_workflow(&ledger);
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
        feed_workflow_line(&mut ledger, 37);

        assert!(
            !written(&ledger).contains("export const meta"),
            "the workflow script must not reach the snapshot: {}",
            written(&ledger)
        );
    }

    #[test]
    fn the_first_progress_array_names_both_phases_and_a_queued_agent() {
        let mut ledger = SurfaceLedger::default();
        feed_workflow_line(&mut ledger, 37);

        assert!(feed_workflow_line(&mut ledger, 40));

        let workflow = the_only_workflow(&ledger);
        let titles: Vec<&str> = workflow
            .phases
            .iter()
            .map(|phase| phase.title.as_str())
            .collect();
        assert_eq!(titles, vec!["Read", "Summarize"]);

        let line_counter = agent_named(&workflow, "line-counter");
        assert_eq!(line_counter.id, "acdd7854c4bce379a");
        assert_eq!(line_counter.state.as_deref(), Some("running"));
        assert_eq!(phase_holding(&workflow, "line-counter"), "Read");

        let char_counter = agent_named(&workflow, "char-counter");
        assert_eq!(char_counter.id, "w81x1fmx5:2");
        assert_eq!(char_counter.state.as_deref(), Some("queued"));
        assert_eq!(phase_holding(&workflow, "char-counter"), "Read");
    }

    #[test]
    fn a_usage_tick_carrying_no_progress_array_moves_nothing() {
        let mut ledger = SurfaceLedger::default();
        feed_workflow_line(&mut ledger, 37);
        feed_workflow_line(&mut ledger, 40);
        let before = written(&ledger);

        assert!(!feed_workflow_line(&mut ledger, 46));

        assert_eq!(written(&ledger), before);
    }

    #[test]
    fn the_final_progress_array_takes_the_real_id_and_the_agent_totals() {
        let ledger = ledger_through_the_final_progress_array();

        let workflow = the_only_workflow(&ledger);
        assert_eq!(
            agent_named(&workflow, "char-counter").id,
            "a1a79b6791abd41ee"
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
                    "/private/tmp/claude-501/-Users-zech--superconductor-worktre…".to_string()
                ),
            })
        );
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

        let workflow = the_only_workflow(&ledger);
        assert!(
            !written(&ledger).contains("char-counter"),
            "a dropped agent leaves the snapshot: {workflow:?}"
        );
        assert_eq!(
            agent_named(&workflow, "line-counter").id,
            "acdd7854c4bce379a"
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

        let workflow = the_only_workflow(&ledger);
        assert_eq!(phase_holding(&workflow, "verifier"), "Verify");
        assert_eq!(agent_named(&workflow, "verifier").id, "w81x1fmx5:9");
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

        let workflow = the_only_workflow(&ledger);
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

        assert!(feed_workflow_line(&mut ledger, 65));
        assert_eq!(the_only_workflow(&ledger).state.as_deref(), Some("done"));

        assert!(!feed_workflow_line(&mut ledger, 66));
        assert_eq!(the_only_workflow(&ledger).state.as_deref(), Some("done"));

        assert!(!feed(
            &mut ledger,
            &json!({
                "subtype": "task_notification",
                "task_id": WORKFLOW_TASK_ID,
                "status": "running",
            })
        ));
        assert_eq!(the_only_workflow(&ledger).state.as_deref(), Some("done"));

        assert!(feed(
            &mut ledger,
            &json!({
                "subtype": "task_notification",
                "task_id": WORKFLOW_TASK_ID,
                "status": "failed",
            })
        ));
        assert_eq!(the_only_workflow(&ledger).state.as_deref(), Some("failed"));
    }

    #[test]
    fn a_progress_line_for_a_workflow_that_never_started_is_ignored() {
        let mut ledger = SurfaceLedger::default();

        assert!(!feed_workflow_line(&mut ledger, 40));
        assert!(ledger.snapshot().is_none());

        feed_workflow_line(&mut ledger, 37);
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

    const SUBAGENT_TASK_ID: &str = "aba8d0dbf79bd05f1";
    const SPAWNING_CALL_ID: &str = "toolu_01P8eCnYQFMqdCaXBXSCcAVd";

    fn feed_subagent_line(ledger: &mut SurfaceLedger, line_number: usize) -> bool {
        feed(ledger, &fixture_line(SUBAGENT_FIXTURE, line_number))
    }

    fn the_only_subagent(ledger: &SurfaceLedger) -> SurfaceAgent {
        let snapshot = ledger.snapshot().expect("the ledger holds a snapshot");
        assert_eq!(snapshot.subagents.len(), 1, "{snapshot:?}");
        snapshot.subagents[0].clone()
    }

    fn ledger_through_the_started_subagent() -> SurfaceLedger {
        let mut ledger = SurfaceLedger::default();
        feed_subagent_line(&mut ledger, 11);
        ledger
    }

    #[test]
    fn a_started_local_agent_opens_a_running_subagent() {
        let mut ledger = SurfaceLedger::default();

        assert!(feed_subagent_line(&mut ledger, 11));

        let subagent = the_only_subagent(&ledger);
        assert_eq!(subagent.id, SUBAGENT_TASK_ID);
        assert_eq!(
            subagent.label,
            "Read README.md and report character count".to_string()
        );
        assert_eq!(subagent.state.as_deref(), Some("running"));
        assert_eq!(subagent.spawning_call_id.as_deref(), Some(SPAWNING_CALL_ID));
        assert_eq!(subagent.started_at, None);
    }

    #[test]
    fn a_repeated_started_line_keeps_what_the_subagent_has_accumulated() {
        let mut ledger = ledger_through_the_started_subagent();
        feed_subagent_line(&mut ledger, 25);
        let progressed = the_only_subagent(&ledger);

        assert!(!feed_subagent_line(&mut ledger, 11));

        let restarted = the_only_subagent(&ledger);
        assert_eq!(restarted.last_tool, progressed.last_tool);
        assert_eq!(restarted.tokens, progressed.tokens);
        assert_eq!(restarted.tool_calls, progressed.tool_calls);
        assert_eq!(restarted.duration_ms, progressed.duration_ms);
        assert_eq!(
            restarted.spawning_call_id.as_deref(),
            Some(SPAWNING_CALL_ID)
        );
    }

    #[test]
    fn a_progress_step_reaches_the_snapshot_as_one_bounded_line() {
        let mut ledger = ledger_through_the_started_subagent();
        let mut sprawling = fixture_line(SUBAGENT_FIXTURE, 25);
        sprawling["description"] = json!(format!("first line\nsecond line\n{}", "x".repeat(400)));

        assert!(feed(&mut ledger, &sprawling));

        let summary = the_only_subagent(&ledger)
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
            SPAWNING_CALL_ID,
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

        assert!(feed_subagent_line(&mut ledger, 25));

        let subagent = the_only_subagent(&ledger);
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
        feed_subagent_line(&mut ledger, 25);
        let before = written(&ledger);

        assert!(!feed_subagent_line(&mut ledger, 25));

        assert_eq!(written(&ledger), before);
    }

    #[test]
    fn the_closing_lines_finish_the_subagent_and_take_its_answer() {
        let mut ledger = ledger_through_the_started_subagent();
        feed_subagent_line(&mut ledger, 25);

        assert!(feed_subagent_line(&mut ledger, 30));
        assert_eq!(the_only_subagent(&ledger).state.as_deref(), Some("done"));

        assert!(feed_subagent_line(&mut ledger, 31));
        let closed = the_only_subagent(&ledger);
        assert_eq!(closed.state.as_deref(), Some("done"));
        assert_eq!(closed.result.as_deref(), Some("4"));
    }

    #[test]
    fn a_notification_for_a_subagent_that_never_started_mints_nothing() {
        let mut ledger = SurfaceLedger::default();

        assert!(!feed_subagent_line(&mut ledger, 31));
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
        assert_eq!(the_only_subagent(&ledger).state.as_deref(), Some("failed"));

        assert!(!feed(
            &mut ledger,
            &json!({
                "subtype": "task_notification",
                "task_id": SUBAGENT_TASK_ID,
                "status": "reticulating",
                "summary": "still going",
            })
        ));
        let unclaimed = the_only_subagent(&ledger);
        assert_eq!(unclaimed.state.as_deref(), Some("failed"));
        assert_eq!(unclaimed.result.as_deref(), Some("the reader gave up"));
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

        let result = the_only_subagent(&ledger)
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
        assert_eq!(keys, vec!["subagents".to_string()]);
    }

    #[test]
    fn a_started_background_shell_is_no_business_of_the_workflow_parser() {
        let mut ledger = SurfaceLedger::default();

        assert!(!feed(
            &mut ledger,
            &fixture_line(SHELL_AND_CHECKLIST_FIXTURE, 42)
        ));
        assert!(ledger.snapshot().is_none());
    }

    const FIRST_CREATE_CALL_ID: &str = "toolu_01V6RPmcsmyRyEVKSdcpKTMJ";
    const FIRST_UPDATE_CALL_ID: &str = "toolu_01UExMFQFbhqwFX9Qz4M3L1q";

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

    fn feed_tool_answer_from_the_next_line(ledger: &mut SurfaceLedger, call_line: usize) -> bool {
        let call_id = tool_call_block(call_line)["id"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("{SHELL_AND_CHECKLIST_FIXTURE}:{call_line} carries a call id")
            })
            .to_string();
        let answer = fixture_line(SHELL_AND_CHECKLIST_FIXTURE, call_line + 1);
        ledger.read_tool_answer(&tool_named_by(call_line), &call_id, &answer)
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
        ledger
            .snapshot()
            .expect("the ledger holds a snapshot")
            .checklist
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
                state: Some("pending".to_string()),
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

        assert!(!ledger.read_tool_answer("TaskCreate", FIRST_CREATE_CALL_ID, &nameless));

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
    fn a_status_token_the_checklist_does_not_know_leaves_the_item_alone() {
        let mut ledger = ledger_through_the_three_creates();
        let mut unclaimed = fixture_line(SHELL_AND_CHECKLIST_FIXTURE, 53);
        unclaimed["tool_use_result"]["statusChange"]["to"] = json!("banana");

        assert!(!ledger.read_tool_answer("TaskUpdate", FIRST_UPDATE_CALL_ID, &unclaimed));

        assert_eq!(
            the_checklist_item(&ledger, "1").state.as_deref(),
            Some("pending")
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
    fn a_todo_write_call_replaces_the_whole_checklist() {
        let mut ledger = ledger_through_the_three_creates();

        assert!(ledger.read_tool_call(
            "TodoWrite",
            &todo_write_block(&[
                ("Read the file", "completed"),
                ("Write the test", "in_progress"),
                ("Run the suite", "pending"),
            ])
        ));

        let listed: Vec<(String, Option<String>)> = the_checklist(&ledger)
            .into_iter()
            .map(|item| (item.subject, item.state))
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

        assert!(ledger.read_tool_call("TodoWrite", &todo_write_block(&[("Ship it", "pending")])));

        assert_eq!(the_checklist(&ledger).len(), 1);
        assert!(
            !written(&ledger).contains("Read the file"),
            "a later list drops what an earlier one carried: {}",
            written(&ledger)
        );
    }
}

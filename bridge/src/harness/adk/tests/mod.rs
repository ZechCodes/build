use super::activity::{
    reader_and_its_status, reader_publishing_status_into, reader_reporting_into,
    reports_already_sent, running_shell_outputs,
};
use super::fake::*;
use super::protocol::{RecordedCall, ACTIVITY_BACKLOG};
use super::reader::ProtocolReader;
use super::translation::tool_call_summary;
use super::*;
use crate::harness::shell_tail::SHELL_TAIL_LINES;
use crate::harness::stream_fixtures::{
    fixture_line, fixture_lines, hundred_numbered_lines, shell_output_file_holding,
    FIRST_CREATE_CALL_ID, SHELL_AND_CHECKLIST_FIXTURE, SHELL_LAUNCH_ANSWER_LINE,
    SHELL_LAUNCH_CALL_ID, SHELL_LAUNCH_CALL_LINE, SHELL_NOTIFICATION_LINE, SHELL_STARTED_LINE,
    SHELL_TASK_ID, SUBAGENT_FIXTURE, SUBAGENT_SPAWNING_CALL_ID, WORKFLOW_FIXTURE,
};
use crate::harness::surfaces::AgentSurfaces;
use crate::harness::surfaces::SurfaceWorkflow;
use crate::harness::{
    ActivityReport, AgentActivity, AgentStatus, HarnessError, ToolOutcome, Turn, TurnChoiceSupport,
};
use crate::models::{AgentProvider, ModelChoice};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

fn row_kind(reported: &ActivityReport) -> &'static str {
    match &reported.activity {
        AgentActivity::Compaction { .. } => "compaction",
        AgentActivity::Reasoning { .. } => "reasoning",
        AgentActivity::ToolUse { .. } => "tool_use",
        AgentActivity::ToolResult { .. } => "tool_result",
        AgentActivity::Narration { .. } => "narration",
        AgentActivity::TaskUpdate { .. } => "task_update",
    }
}

fn kinds_of(rows: &[ActivityReport]) -> Vec<&'static str> {
    rows.iter().map(row_kind).collect()
}

fn kinds_and_parents_of(rows: &[ActivityReport]) -> Vec<(&'static str, Option<&str>)> {
    rows.iter()
        .map(|reported| (row_kind(reported), reported.parent_call_id.as_deref()))
        .collect()
}

fn task_rows_of(rows: &[ActivityReport]) -> Vec<String> {
    rows.iter()
        .filter(|reported| row_kind(reported) == "task_update")
        .map(|reported| reported.activity.summary().to_string())
        .collect()
}

const SPAWNED_AGENT_CALL: &str = "toolu_01SpawningCallSynthetic";

fn a_spawned_agents_todo_write_line() -> String {
    json!({
        "type": "assistant",
        "parent_tool_use_id": SPAWNED_AGENT_CALL,
        "message": { "content": [{
            "type": "tool_use",
            "id": "toolu_01TodoWriteBySpawnedAgent",
            "name": "TodoWrite",
            "input": { "todos": [{
                "content": "read the file",
                "status": "in_progress",
                "activeForm": "reading the file",
            }] },
        }] },
    })
    .to_string()
}

fn reader_and_what_it_reports() -> (ProtocolReader, broadcast::Receiver<ActivityReport>) {
    let (sender, heard) = broadcast::channel(ACTIVITY_BACKLOG);
    (
        reader_reporting_into(Arc::new(Mutex::new(Some(sender)))),
        heard,
    )
}

fn reader_over_a_silent_session() -> ProtocolReader {
    reader_reporting_into(Arc::new(Mutex::new(None)))
}

fn reader_with_a_live_activity_slot() -> ProtocolReader {
    let (sender, _heard) = broadcast::channel(ACTIVITY_BACKLOG);
    reader_reporting_into(Arc::new(Mutex::new(Some(sender))))
}

fn read_lines_into(reader: &mut ProtocolReader, lines: &[String]) {
    for line in lines {
        reader.read_line(line);
    }
}

fn fixture_lines_numbered(file_name: &str, line_numbers: &[usize]) -> Vec<String> {
    let lines = fixture_lines(file_name);
    line_numbers
        .iter()
        .map(|line_number| lines[line_number - 1].clone())
        .collect()
}

fn reader_over_every_line_of(file_name: &str) -> ProtocolReader {
    let mut reader = reader_over_a_silent_session();
    read_lines_into(&mut reader, &fixture_lines(file_name));
    reader
}

fn reader_over_the_workflow_lines(line_numbers: &[usize]) -> ProtocolReader {
    let mut reader = reader_over_a_silent_session();
    read_lines_into(
        &mut reader,
        &fixture_lines_numbered(WORKFLOW_FIXTURE, line_numbers),
    );
    reader
}

const SHARED_WIRE_FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/agent_surfaces.json"
);
const SPAWNING_CALL_SEQUENCE: u64 = 12;

fn the_wire_every_recorded_stream_builds() -> serde_json::Value {
    let mut reader = reader_over_a_silent_session();
    for fixture in [
        WORKFLOW_FIXTURE,
        SUBAGENT_FIXTURE,
        SHELL_AND_CHECKLIST_FIXTURE,
    ] {
        read_lines_into(&mut reader, &fixture_lines(fixture));
    }
    surfaces_of(&reader)
        .expect("the recorded streams build a snapshot")
        .wire_value(&|call_id| {
            (call_id == SUBAGENT_SPAWNING_CALL_ID).then_some(SPAWNING_CALL_SEQUENCE)
        })
}

fn surfaces_of(reader: &ProtocolReader) -> Option<AgentSurfaces> {
    reader.state.lock().unwrap().surfaces.snapshot()
}

fn revision_counter_of(reader: &ProtocolReader) -> u64 {
    *reader.revision.subscribe().borrow()
}

fn agent_states_of(workflow: &SurfaceWorkflow) -> Vec<Option<&str>> {
    workflow
        .phases
        .iter()
        .flat_map(|phase| phase.agents.iter())
        .map(|agent| agent.state.as_deref())
        .collect()
}

const TASK_CREATE_CALL_LINE: usize = 27;
const TURN_RESULT_LINE: usize = 81;

fn pending_create_count_of(reader: &ProtocolReader) -> usize {
    reader.state.lock().unwrap().surfaces.pending_create_count()
}

fn a_background_shell_line_set(task_id: &str, call_id: &str, output_path: &Path) -> Vec<String> {
    let mut call = fixture_line(SHELL_AND_CHECKLIST_FIXTURE, SHELL_LAUNCH_CALL_LINE);
    call["message"]["content"][0]["id"] = json!(call_id);
    let mut started = fixture_line(SHELL_AND_CHECKLIST_FIXTURE, SHELL_STARTED_LINE);
    started["task_id"] = json!(task_id);
    started["tool_use_id"] = json!(call_id);
    let mut answer = fixture_line(SHELL_AND_CHECKLIST_FIXTURE, SHELL_LAUNCH_ANSWER_LINE);
    answer["message"]["content"][0]["tool_use_id"] = json!(call_id);
    answer["message"]["content"][0]["content"] = json!(format!(
            "Command running in background with ID: {task_id}. Output is being written to: {}. You will be notified when it completes.",
            output_path.display()
        ));
    answer["tool_use_result"]["backgroundTaskId"] = json!(task_id);
    [call, started, answer]
        .iter()
        .map(Value::to_string)
        .collect()
}

struct ReaderEndingItsSessionWhenDropped {
    reader: ProtocolReader,
}

impl std::ops::Deref for ReaderEndingItsSessionWhenDropped {
    type Target = ProtocolReader;

    fn deref(&self) -> &ProtocolReader {
        &self.reader
    }
}

impl std::ops::DerefMut for ReaderEndingItsSessionWhenDropped {
    fn deref_mut(&mut self) -> &mut ProtocolReader {
        &mut self.reader
    }
}

impl Drop for ReaderEndingItsSessionWhenDropped {
    fn drop(&mut self) {
        self.reader.activity.lock().unwrap().take();
        let polling = self.reader.shell_poller.lock().unwrap().take();
        if let Some(polling) = polling {
            polling.join().expect(
                    "the shell tail poller ends cleanly before the temporary directory it reads is removed",
                );
        }
    }
}

fn reader_tailing(task_id: &str, output_path: &Path) -> ReaderEndingItsSessionWhenDropped {
    let mut reader = reader_with_a_live_activity_slot();
    read_lines_into(
        &mut reader,
        &a_background_shell_line_set(task_id, SHELL_LAUNCH_CALL_ID, output_path),
    );
    ReaderEndingItsSessionWhenDropped { reader }
}

fn polling_thread_of(reader: &ProtocolReader) -> std::thread::ThreadId {
    reader
        .shell_poller
        .lock()
        .unwrap()
        .as_ref()
        .expect("a shell tail poller is running")
        .thread()
        .id()
}

fn no_poller_is_running(reader: &ProtocolReader) -> bool {
    reader.shell_poller.lock().unwrap().is_none()
}

fn tail_of_the_shell(reader: &ProtocolReader, shell_id: &str) -> Vec<String> {
    surfaces_of(reader)
        .into_iter()
        .flat_map(|surfaces| surfaces.shells)
        .find(|shell| shell.id == shell_id)
        .map(|shell| shell.tail)
        .unwrap_or_default()
}

fn becomes_true_within(limit: Duration, ready: impl Fn() -> bool) -> bool {
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        if ready() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    ready()
}

/// The choice every fake is opened with unless a test says otherwise: this
/// provider, no model named, no effort named — so the recorded `init` line
/// is checked against nothing and every recording plays as it always did.
fn adk_choice() -> ModelChoice {
    ModelChoice {
        provider: AgentProvider::ClaudeAdk,
        ..ModelChoice::default()
    }
}

fn open(spec: &HarnessSpec) -> AdkSession {
    open_with(spec, &adk_choice())
}

fn open_with(spec: &HarnessSpec, choice: &ModelChoice) -> AdkSession {
    AdkSession::spawn(spec, None, choice)
        .expect("the fake harness spawns")
        .0
}

/// Wait for the session to report `want`, or fail saying what it reported
/// instead. Statuses here are protocol-driven, so the wait is for a line to
/// arrive rather than for a clock to run out.
fn wait_for_status(session: &AdkSession, want: AgentStatus) {
    if becomes_true_within(Duration::from_secs(5), || session.status() == want) {
        return;
    }
    panic!(
        "the session never reported {want:?} — it is {:?}",
        session.status()
    );
}

async fn next_activity(rx: &mut broadcast::Receiver<ActivityReport>) -> AgentActivity {
    next_report(rx).await.activity
}

async fn next_report(rx: &mut broadcast::Receiver<ActivityReport>) -> ActivityReport {
    match tokio::time::timeout(Duration::from_secs(5), rx.recv()).await {
        Ok(Ok(report)) => report,
        Ok(Err(err)) => panic!("the activity stream ended before it reported: {err}"),
        Err(_) => panic!("no activity arrived within five seconds"),
    }
}

fn spawn_options() -> SpawnOptions {
    SpawnOptions {
        continue_session: false,
        resume_session_id: None,
        owner_id: "agent-01J".to_string(),
        mcp_session_token: "token-42".to_string(),
        cwd: PathBuf::from("/tmp/worktree"),
    }
}

fn context() -> HarnessContext {
    HarnessContext {
        bridge_exe: PathBuf::from("/usr/local/bin/build-bridge"),
        mcp_socket: PathBuf::from("/tmp/build-mcp.sock"),
        state_root: PathBuf::from("/tmp/build-state"),
    }
}

mod activity;

mod protocol_reader;
mod provider;
mod session;
mod translation;
mod turn_context;

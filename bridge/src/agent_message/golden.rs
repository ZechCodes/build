//! What each harness is handed for a fixed set of deliveries, pinned byte for
//! byte in `golden/<harness>/<scenario>.txt`.
//!
//! The scenarios are the messages Build sends most: the user in the thread,
//! a cold start, an agent handing over a task, task notices, and a bare
//! provider command. Each harness's own tests feed them through its session
//! and compare what reached the child with the file here, so a change to the
//! words or their framing shows up as a diff to a file a reviewer can read.
//!
//! `UPDATE_GOLDENS=1` rewrites the files instead of comparing them.

use std::path::PathBuf;

use crate::models::AgentProvider;
use crate::operation::OperationPayload;
use crate::thread::ThreadMessage;

/// Every harness, by the directory its golden files live in.
pub(crate) const HARNESSES: [(AgentProvider, &str); 5] = [
    (AgentProvider::ClaudeAdk, "adk"),
    (AgentProvider::Claude, "claude"),
    (AgentProvider::Codex, "codex"),
    (AgentProvider::CodexAppServer, "codex_app_server"),
    (AgentProvider::Pi, "pi"),
];

/// How the delivery reached the agent: an accepted operation, or the unread
/// thread a nudge reads.
enum Scope {
    Native(&'static str),
    Legacy,
}

/// One delivery, rendered the way the queue renders it for a provider.
pub(crate) struct Scenario {
    pub name: &'static str,
    payload: OperationPayload,
    scope: Scope,
    cold: bool,
}

impl Scenario {
    /// The prompt Build hands `provider` for this delivery.
    pub(crate) fn prompt(&self, provider: AgentProvider) -> String {
        match self.scope {
            Scope::Native(operation_id) => {
                self.payload
                    .delivery_prompt(operation_id, self.cold, provider)
            }
            Scope::Legacy => self.payload.legacy_delivery_prompt(self.cold, provider),
        }
    }
}

fn message(value: serde_json::Value) -> ThreadMessage {
    let mut base = serde_json::json!({
        "id": "message-7",
        "sequence": 7,
        "updated_sequence": 7,
        "role": "user",
        "created_at": "2026-11-02T12:00:00Z",
    });
    base.as_object_mut()
        .unwrap()
        .extend(value.as_object().unwrap().clone());
    serde_json::from_value(base).expect("a scenario message")
}

fn payload(messages: Vec<ThreadMessage>) -> OperationPayload {
    OperationPayload {
        start_sequence: 7,
        end_sequence: 7,
        messages,
        prior_context: "The previous revision changed the parser.".to_string(),
        ask_to_name: false,
        tells_sender_context: false,
    }
}

fn task_envelope() -> serde_json::Value {
    serde_json::json!({
        "task_id": "task-42",
        "number": 42,
        "title": "Fix the parser",
        "links": {"workspace_ids": ["ws-1"]},
    })
}

fn task_notice(actor: serde_json::Value, action: &str) -> serde_json::Value {
    let mut notice = serde_json::json!({"actor": actor, "action": action});
    if action == "commented" {
        notice["comment_id"] = serde_json::json!("tc-9");
    }
    notice
}

/// The user writing in the thread from a workspace, with a file attached.
fn user_in_thread() -> ThreadMessage {
    message(serde_json::json!({
        "body": "inspect the screenshot",
        "attachments": [{
            "name": "failure.png",
            "path": ".build/attachments/failure.png",
            "mime": "image/png",
            "size": 123
        }],
        "viewing_context": {
            "version": 1,
            "items": [{"kind": "workspace", "workspace_id": "ws-1", "name": "parser"}]
        },
    }))
}

/// The user writing in the thread while looking at a task.
fn user_looking_at_task() -> ThreadMessage {
    message(serde_json::json!({
        "body": "is this one done?",
        "viewing_context": {
            "version": 1,
            "items": [{"kind": "task", "task_id": "task-42", "number": 42, "title": "Fix the parser"}]
        },
    }))
}

/// Another agent handing over a task.
fn agent_assignment() -> ThreadMessage {
    message(serde_json::json!({
        "body": "The project agent assigned you Build task #42 — Fix the parser",
        "from_agent": {"id": "project-1", "name": "Coordinator"},
        "from_task": task_envelope(),
    }))
}

/// Build telling a tracker about a comment on a task.
fn comment_notice(actor: serde_json::Value, who: &str) -> ThreadMessage {
    message(serde_json::json!({
        "body": format!("New comment tc-9 on #42 from {who} — read_comment for their message, answer on the task with comment_task."),
        "from_build": true,
        "from_task": task_envelope(),
        "task_notice": task_notice(actor, "commented"),
    }))
}

/// Build telling a tracker a task moved.
fn moved_notice() -> ThreadMessage {
    message(serde_json::json!({
        "body": "#42 moved to In review by the Build project's agent.",
        "from_build": true,
        "from_task": task_envelope(),
        "task_notice": {
            "actor": {"kind": "agent", "agent_id": "project-1"},
            "action": "moved",
            "to": "in_review"
        },
    }))
}

/// A provider command, which must reach the provider untouched.
fn compact_command() -> ThreadMessage {
    message(serde_json::json!({"body": "/compact"}))
}

pub(crate) fn scenarios() -> Vec<Scenario> {
    let native = |name, messages, cold| Scenario {
        name,
        payload: payload(messages),
        scope: Scope::Native("op-1"),
        cold,
    };
    let legacy = |name, messages| Scenario {
        name,
        payload: payload(messages),
        scope: Scope::Legacy,
        cold: false,
    };
    vec![
        native("user_in_thread", vec![user_in_thread()], false),
        native("user_in_thread_cold", vec![user_in_thread()], true),
        native("user_looking_at_task", vec![user_looking_at_task()], false),
        native("agent_assignment", vec![agent_assignment()], false),
        legacy(
            "user_task_comment",
            vec![comment_notice(
                serde_json::json!({"kind": "user"}),
                "the user",
            )],
        ),
        legacy(
            "agent_task_comment",
            vec![comment_notice(
                serde_json::json!({"kind": "agent", "agent_id": "agent-3"}),
                "agent agent-3",
            )],
        ),
        legacy("task_moved", vec![moved_notice()]),
        native("compact_command", vec![compact_command()], false),
        Scenario {
            name: "first_delivery_asking_a_name",
            payload: OperationPayload {
                ask_to_name: true,
                tells_sender_context: true,
                ..payload(vec![user_in_thread()])
            },
            scope: Scope::Native("op-1"),
            cold: false,
        },
    ]
}

fn golden_path(harness: &str, scenario: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("src/agent_message/golden")
        .join(harness)
        .join(format!("{scenario}.txt"))
}

/// Compare what `harness` was handed for `scenario` with its golden file, or
/// rewrite the file under `UPDATE_GOLDENS=1`.
pub(crate) fn assert_golden(harness: &str, scenario: &str, actual: &str) {
    let path = golden_path(harness, scenario);
    if std::env::var_os("UPDATE_GOLDENS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    assert_eq!(
        actual,
        expected,
        "{harness}/{scenario} differs from {}",
        path.display()
    );
}

/// Every golden file belongs to a harness and a scenario that still exist,
/// so a renamed scenario cannot leave a stale file behind that pins nothing.
#[test]
fn every_golden_file_is_a_live_scenario() {
    if std::env::var_os("UPDATE_GOLDENS").is_some() {
        // The harness tests are rewriting the files alongside this one.
        return;
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/agent_message/golden");
    let names: Vec<&str> = scenarios().iter().map(|scenario| scenario.name).collect();
    for (_, harness) in HARNESSES {
        let mut found: Vec<String> = std::fs::read_dir(root.join(harness))
            .unwrap_or_else(|error| panic!("{harness}: {error}"))
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        found.sort();
        let mut expected: Vec<String> = names.iter().map(|name| format!("{name}.txt")).collect();
        expected.sort();
        assert_eq!(found, expected, "{harness}");
    }
}

/// The terminal harnesses: what lands on the child's stdin for each delivery,
/// paste frame and submit key included.
#[tokio::test]
async fn terminal_harnesses_are_handed_their_golden_bytes() {
    use crate::harness::{AgentSession, Turn};
    use crate::pty::{HarnessSpec, PtySession};

    let terminal = [
        (AgentProvider::Claude, "claude"),
        (AgentProvider::Codex, "codex"),
        (AgentProvider::Pi, "pi"),
    ];
    for (provider, harness) in terminal {
        for scenario in scenarios() {
            let directory = tempfile::tempdir().unwrap();
            let capture = directory.path().join("stdin.txt");
            let spec = HarnessSpec::new("sh")
                .arg("-c")
                .arg("stty raw -echo && printf '\\033[?2004h' && cat > \"$1\"")
                .arg("build-stdin-capture")
                .arg(capture.to_string_lossy());
            let size = portable_pty::PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            };
            let session = PtySession::spawn(&spec, None, size).unwrap();
            // The child turns paste mode on once its tty is raw.
            assert!(session.ready_within(std::time::Duration::from_secs(5)));
            session
                .send_turn(&Turn::new(scenario.prompt(provider)))
                .unwrap();
            let captured = captured_through_submit(&capture).await;
            assert_golden(harness, scenario.name, &captured);
            session.end();
        }
    }
}

/// What the child has read once its input ends in the submit key (raw, so
/// `\r` arrives as written) and has stopped growing.
async fn captured_through_submit(capture: &std::path::Path) -> String {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut last = String::new();
    loop {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let contents = std::fs::read_to_string(capture).unwrap_or_default();
        let settled = !contents.is_empty() && contents.ends_with('\r') && contents == last;
        if settled || std::time::Instant::now() > deadline {
            return contents;
        }
        last = contents;
    }
}

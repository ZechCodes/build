// Exact test bodies moved from the former inline test module.
use super::*;

/// A call reads as the thing it ran, and `Bash` runs a command. The
/// `description` beside it is the model's paraphrase of the same act, and a
/// row is one quiet line rather than two claims about it — so the command is
/// what survives.
#[test]
fn a_bash_call_reads_as_the_command_it_ran() {
    assert_eq!(
        tool_call_summary(
            "Bash",
            &json!({ "command": "cargo test", "description": "Run the test suite" }),
        ),
        "Bash cargo test",
    );
}
/// Every tool the table names mints its own one human argument, whatever
/// else the call carried beside it.
#[test]
fn each_named_tool_mints_the_argument_a_developer_would_read() {
    for (tool, input, want) in [
        (
            "Read",
            json!({ "file_path": "bridge/src/app.rs", "offset": 40 }),
            "Read bridge/src/app.rs",
        ),
        (
            "Write",
            json!({ "file_path": "spa/src/core/thread.js", "content": "export const x = 1;" }),
            "Write spa/src/core/thread.js",
        ),
        (
            "Edit",
            json!({ "file_path": "bridge/src/harness/adk.rs", "old_string": "a", "new_string": "b" }),
            "Edit bridge/src/harness/adk.rs",
        ),
        (
            "NotebookEdit",
            json!({ "notebook_path": "analysis.ipynb", "new_source": "print(1)" }),
            "NotebookEdit analysis.ipynb",
        ),
        (
            "Glob",
            json!({ "pattern": "**/*.rs", "path": "bridge" }),
            "Glob **/*.rs",
        ),
        (
            "Grep",
            json!({ "pattern": "tool_call_summary", "output_mode": "content" }),
            "Grep tool_call_summary",
        ),
        (
            "WebFetch",
            json!({ "url": "https://example.com/spec", "prompt": "what changed?" }),
            "WebFetch https://example.com/spec",
        ),
        (
            "WebSearch",
            json!({ "query": "stream-json protocol", "allowed_domains": ["example.com"] }),
            "WebSearch stream-json protocol",
        ),
        (
            "Task",
            json!({ "description": "audit the readers", "prompt": "read every reader" }),
            "Task audit the readers",
        ),
    ] {
        assert_eq!(tool_call_summary(tool, &input), want, "{tool}");
    }
}
/// Expandable tool text keeps its original shape through the activity limit,
/// then clips on a character boundary with an ellipsis.
#[test]
fn a_long_command_clips_at_the_activity_limit() {
    let command = format!("first\n{}", "λ".repeat(ACTIVITY_TEXT_LIMIT));
    let summary = tool_call_summary("Bash", &json!({ "command": command }));
    assert!(summary.contains('\n'));
    assert_eq!(
        summary.chars().count(),
        ACTIVITY_TEXT_LIMIT + 1,
        "the ellipsis sits beyond the activity text bound"
    );
    assert!(summary.ends_with('…'));
}

// Exact test bodies moved from the former inline test module.
use super::*;

/// The launch config is the interactive one with the TUI swapped for the
/// protocol: the same binary, the same permission grant, the same model
/// flags, and the stream-json argv that makes a turn a value.
#[test]
fn the_headless_spec_runs_claude_over_stream_json_on_both_ends() {
    let choice = ModelChoice {
        provider: AgentProvider::ClaudeAdk,
        model: Some("claude-fable-5-1".to_string()),
        effort: Some("high".to_string()),
    };
    let spec = AdkHarness
        .spec(&choice, &spawn_options(), &context())
        .unwrap();

    assert_eq!(spec.binary, "claude");
    let args = spec.args.join(" ");
    assert!(
        args.contains("-p --input-format stream-json --output-format stream-json --verbose"),
        "the protocol argv, whole and in order: {args}"
    );
    assert!(
        args.contains("--model claude-fable-5-1 --effort high"),
        "a model selection reaches the same flags claude has always taken: {args}"
    );
    assert!(args.contains("--dangerously-skip-permissions"), "{args}");
    assert!(
        !args.contains("--continue"),
        "nothing to continue was asked for: {args}"
    );
    assert_eq!(
        spec.submit_delay,
        Duration::ZERO,
        "a submit key is how a prompt is typed into a line editor, and there is none here"
    );

    let resumed = AdkHarness
        .spec(
            &choice,
            &SpawnOptions {
                continue_session: true,
                ..spawn_options()
            },
            &context(),
        )
        .unwrap();
    assert!(
        resumed.args.contains(&"--continue".to_string()),
        "a headless session picks the worktree's conversation back up: {:?}",
        resumed.args
    );
}
/// §2's dividend, held to: everything an agent says to Build arrives over
/// the MCP socket, so the from-agent half of the interface needs zero work
/// for a new provider — provided the wiring really is identical. This is
/// what checks that it is.
#[test]
fn the_headless_spec_carries_exactly_the_interactive_mcp_wiring() {
    let choice = ModelChoice {
        provider: AgentProvider::ClaudeAdk,
        ..ModelChoice::default()
    };
    let options = spawn_options();
    let headless = AdkHarness.spec(&choice, &options, &context()).unwrap();
    let interactive = ClaudeHarness
        .spec(&ModelChoice::default(), &options, &context())
        .unwrap();

    assert_eq!(
        headless.env, interactive.env,
        "the same socket and the same per-process capability"
    );
    assert_eq!(
        headless.unset, interactive.unset,
        "an agent Build spawns is its own session on either provider"
    );
    let mcp_args = |spec: &HarnessSpec| -> Vec<String> {
        spec.args
            .iter()
            .skip_while(|arg| *arg != "--mcp-config")
            .take(3)
            .cloned()
            .collect()
    };
    assert_eq!(
        mcp_args(&headless),
        mcp_args(&interactive),
        "the same per-agent config, loaded the same strict way"
    );
    assert!(mcp_args(&headless).contains(&crate::orchestrator::mcp_config_path(&options.owner_id)));
}
/// The catalog, the trust registry and the transcripts belong to the CLI,
/// not to the session protocol — it is the same claude, the same account and the
/// same `~/.claude/projects`. Only the terminal answer differs.
#[test]
fn the_headless_provider_is_claude_in_every_way_but_its_io() {
    assert_eq!(
        AdkHarness.models().len(),
        ClaudeHarness.models().len(),
        "one catalog, one place to add a model"
    );
    assert_eq!(AdkHarness.effort_levels(), ClaudeHarness.effort_levels());

    let home = tempfile::tempdir().expect("temp home");
    let cwd = std::path::Path::new("/Users/z/proj");
    assert!(!AdkHarness.has_transcript(home.path(), cwd));
    let encoded = home
        .path()
        .join(".claude/projects")
        .join(crate::harness::claude::encode_project_dir(cwd));
    std::fs::create_dir_all(&encoded).expect("the transcript dir");
    std::fs::write(encoded.join("session.jsonl"), "{}\n").expect("a transcript");
    assert!(
        AdkHarness.has_transcript(home.path(), cwd),
        "a headless session writes the transcripts the TUI does, so a resume finds them"
    );
}

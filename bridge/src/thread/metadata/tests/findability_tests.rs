use super::*;

/// A checkout holding exactly the files a test names, so "this path is real"
/// is decided by the same filesystem the bridge would ask.
fn checkout(files: &[&str]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("a temp checkout");
    for file in files {
        let path = dir.path().join(file);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("dirs");
        std::fs::write(path, "").expect("a file");
    }
    dir
}

fn thread_in(dir: &tempfile::TempDir) -> Thread {
    let mut thread = Thread::new("run-1");
    thread.set_worktree_root(dir.path());
    thread
}

fn message_metadata(thread: &Thread) -> ItemMetadata {
    thread
        .items
        .last()
        .expect("an item was posted")
        .metadata()
        .clone()
}

/// The rule that keeps prose out of the file index: a path is recorded only
/// when the worktree really has that file. "and/or" and a plausible-looking
/// path nobody created are prose, not files.
#[test]
fn a_path_counts_only_when_the_checkout_really_has_that_file() {
    let dir = checkout(&["src/parser.rs", "docs/design.md"]);
    let mut thread = thread_in(&dir);
    thread.post_user(
        "Rework `src/parser.rs` and/or docs/design.md, but not src/imaginary.rs. \
             See main.rs too.",
        None,
        "2026-08-13T09:00:00Z",
    );

    let metadata = message_metadata(&thread);
    assert_eq!(metadata.files, vec!["src/parser.rs", "docs/design.md"]);
}

/// Punctuation is how people write, not part of the path.
#[test]
fn a_path_wrapped_in_prose_punctuation_is_still_the_path() {
    let dir = checkout(&["src/parser.rs"]);
    let mut thread = thread_in(&dir);
    thread.post_agent(
        "I touched (src/parser.rs), then re-read \"src/parser.rs\".",
        None,
        "2026-08-13T09:00:00Z",
    );
    assert_eq!(message_metadata(&thread).files, vec!["src/parser.rs"]);
}

/// A conversation with no checkout behind it cannot tell a path from a
/// phrase, so it claims no files from prose — but a link the bridge already
/// validated is a file whatever the thread knows about disk.
#[test]
fn without_a_checkout_only_validated_links_name_files() {
    let mut thread = Thread::new("run-rootless");
    thread.post_agent_with_links(
        "Look at src/parser.rs and/or elsewhere.",
        None,
        vec![ThreadLink::File {
            path: "src/lexer.rs".to_string(),
            line_start: None,
            line_end: None,
        }],
        "2026-08-13T09:00:00Z",
    );
    assert_eq!(message_metadata(&thread).files, vec!["src/lexer.rs"]);
}

/// A commit sha is 7-40 hex characters. Requiring a digit is what keeps
/// all-hex English ("effaced", "deface") out of the commit index; a real
/// sha without a single digit is a one-in-a-thousand accident, and a link
/// carries the exact sha anyway.
#[test]
fn a_commit_is_hex_of_the_right_length_with_a_digit_in_it() {
    let mut thread = Thread::new("run-commits");
    thread.post_user(
        "a1b2c3d effaced deadbeef 0123456789abcdef0123456789abcdef01234567 \
             abc123 0123456789abcdef0123456789abcdef012345678",
        None,
        "2026-08-13T09:00:00Z",
    );
    assert_eq!(
        message_metadata(&thread).commits,
        vec!["a1b2c3d", "0123456789abcdef0123456789abcdef01234567"],
        "short, digit-free and over-long tokens are not shas"
    );
}

#[test]
fn a_commit_link_is_a_commit_however_the_body_reads() {
    let mut thread = Thread::new("run-commit-link");
    thread.push_event_with_links(
        ThreadEventKind::Committed,
        Some("committed the parser fix".to_string()),
        None,
        None,
        vec![ThreadLink::Commit {
            sha: "A".repeat(40),
        }],
        "2026-08-13T09:00:00Z",
    );
    assert_eq!(
        thread.items.last().unwrap().metadata().commits,
        vec!["a".repeat(40)],
        "a sha is indexed lowercase whatever case it arrived in"
    );
}

#[test]
fn a_stage_link_makes_the_item_findable_by_stage() {
    let mut thread = Thread::new("run-stages");
    thread.push_event_with_links(
        ThreadEventKind::StageStarted,
        Some("Started stage Parser".to_string()),
        None,
        None,
        vec![
            ThreadLink::TaskStage {
                task_id: "task-1".to_string(),
                stage_id: "parser".to_string(),
                path: ".build/plan/01-parser.md".to_string(),
            },
            ThreadLink::PlanStage {
                plan_id: "task-1".to_string(),
                stage_id: "lexer".to_string(),
                path: ".build/plan/02-lexer.md".to_string(),
            },
        ],
        "2026-08-13T09:00:00Z",
    );
    assert_eq!(
        thread.items.last().unwrap().metadata().stages,
        vec!["parser", "lexer"]
    );
}

/// The completion report is the densest statement of what a change touched,
/// so it is indexed like any other text the agent wrote.
#[test]
fn a_completion_report_makes_its_critical_files_findable() {
    let dir = checkout(&["src/parser.rs"]);
    let mut thread = thread_in(&dir);
    thread.post_outcome(
        MessageOutcome::Completed,
        "rewrote the parser",
        Some(&CompletionReport {
            critical_files: vec!["src/parser.rs — now streams tokens".to_string()],
            risk_notes: vec!["reverts cleanly at a1b2c3d".to_string()],
            decisions: Vec::new(),
            skips: Vec::new(),
        }),
        "2026-08-13T09:00:00Z",
    );
    let metadata = thread.items.last().unwrap().metadata().clone();
    assert_eq!(metadata.files, vec!["src/parser.rs"]);
    assert_eq!(metadata.commits, vec!["a1b2c3d"]);
}

#[test]
fn metadata_ships_on_the_wire_and_an_item_without_any_omits_it() {
    let dir = checkout(&["src/parser.rs"]);
    let mut thread = thread_in(&dir);
    thread.post_user("nothing to see here", None, "2026-08-13T09:00:00Z");
    thread.post_agent("fixed src/parser.rs", None, "2026-08-13T09:01:00Z");

    let wire = thread.wire_value();
    assert!(
        wire["items"][0]["data"].get("metadata").is_none(),
        "{wire:?}"
    );
    assert_eq!(
        wire["items"][1]["data"]["metadata"]["files"][0],
        "src/parser.rs"
    );

    let reloaded: Thread = serde_json::from_value(serde_json::to_value(&thread).unwrap())
        .expect("a thread with metadata reloads");
    assert_eq!(reloaded.items, thread.items);
}

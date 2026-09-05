//! The cognitive-complexity ratchet only goes down (CLAUDE.md "Complexity gates").
//!
//! `#[allow(clippy::cognitive_complexity)]` is how a function that was already
//! over the threshold when the gate landed stays in the tree. Clippy is happy
//! either way once the allow is written, so the count is asserted here: adding
//! one fails this test, and retiring one is a deliberate edit of the number
//! below.

use std::fs;
use std::path::{Path, PathBuf};

/// Measured when the gate landed: 19 in `src/app.rs`, 2 each in `src/bin/relay.rs`
/// and `src/harness/codex_app_server/tests.rs`, and one each in `src/mcp.rs`,
/// `src/rtc/testing.rs`, `src/thread.rs`, `src/worktree.rs`,
/// `tests/pi_extension.rs` and `tests/real_store_migration.rs`.
const RATCHETED_FUNCTIONS: usize = 29;

const ALLOW: &str = "#[allow(clippy::cognitive_complexity)]";

/// This file names the marker to look for, so it never counts itself.
const SELF: &str = "complexity_ratchet.rs";

fn rust_files(dir: &Path, into: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = fs::read_dir(dir)
        .expect("readable directory")
        .map(|entry| entry.expect("readable entry").path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            rust_files(&path, into);
        } else if path.extension().is_some_and(|ext| ext == "rs")
            && path.file_name().is_some_and(|name| name != SELF)
        {
            into.push(path);
        }
    }
}

#[test]
fn no_new_cognitive_complexity_suppressions() {
    let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    rust_files(&crate_root.join("src"), &mut files);
    rust_files(&crate_root.join("tests"), &mut files);

    let found: Vec<String> = files
        .iter()
        .flat_map(|path| {
            let text = fs::read_to_string(path).expect("readable source");
            let name = path.strip_prefix(crate_root).unwrap_or(path).to_owned();
            text.lines()
                .enumerate()
                .filter(|(_, line)| line.contains(ALLOW))
                .map(|(index, _)| format!("{}:{}", name.display(), index + 1))
                .collect::<Vec<_>>()
        })
        .collect();

    assert_eq!(
        found.len(),
        RATCHETED_FUNCTIONS,
        "the cognitive-complexity ratchet moved — split the function instead of \
         allowing it, or lower RATCHETED_FUNCTIONS when retiring one. Found: {found:?}"
    );
}

//! The cognitive-complexity ratchet only goes down (CLAUDE.md "Complexity gates").
//!
//! `#[allow(clippy::cognitive_complexity)]` is how a function that was already
//! over the threshold when the gate landed stays in the tree. Clippy is happy
//! either way once the allow is written, so the count is asserted here: adding
//! one fails this test, and retiring one is a deliberate edit of the number
//! below.

use std::fs;
use std::path::{Path, PathBuf};

/// The baseline includes 14 application functions, 2 each in the relay binary
/// and Codex app-server tests, and one each in MCP, RTC testing, thread paging,
/// Pi extension tests, and real-store migration tests. Module extraction does
/// not change this budget. Retiring the issue workflow removed five exemptions.
/// The earlier isolation refactor also removed one from the original baseline:
/// `worktree.rs`'s `parse_worktree_block` is a path-only porcelain parser now
/// that the isolation seam owns what it used to describe, and needs no allow.
const RATCHETED_FUNCTIONS: usize = 23;

const ALLOW: &str = "#[allow(clippy::cognitive_complexity)]";

/// A crate- or module-level allow (`#![allow(...)]`) would switch the lint
/// off for everything beneath it without touching the count: none may exist.
const BLANKET_ALLOW: &str = "#![allow(clippy::cognitive_complexity)]";

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

fn crate_sources() -> (PathBuf, Vec<PathBuf>) {
    let crate_root = Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf();
    let mut files = Vec::new();
    rust_files(&crate_root.join("src"), &mut files);
    rust_files(&crate_root.join("tests"), &mut files);
    (crate_root, files)
}

#[test]
fn the_lint_is_never_switched_off_for_a_whole_module_or_crate() {
    let (crate_root, files) = crate_sources();
    let blanket: Vec<String> = files
        .iter()
        .filter(|path| {
            fs::read_to_string(path)
                .expect("readable source")
                .contains(BLANKET_ALLOW)
        })
        .map(|path| {
            path.strip_prefix(&crate_root)
                .unwrap_or(path)
                .display()
                .to_string()
        })
        .collect();
    assert!(
        blanket.is_empty(),
        "a blanket allow for cognitive_complexity defeats the ratchet: {blanket:?}"
    );
}

#[test]
fn no_new_cognitive_complexity_suppressions() {
    let (crate_root, files) = crate_sources();

    let found: Vec<String> = files
        .iter()
        .flat_map(|path| {
            let text = fs::read_to_string(path).expect("readable source");
            let name = path.strip_prefix(&crate_root).unwrap_or(path).to_owned();
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

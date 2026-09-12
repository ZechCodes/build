use super::branches::checkout_refusal_message;
use super::history::is_valid_hash_prefix;
use super::mutations::stageable_paths;
use super::network::{pull_flag, run_with_timeout};
use super::patches::file_patches_capped;
use super::status::{
    file_status_json, is_untracked_status, map_repository_state, status_payload_with_file_cap,
};
use super::*;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

// --- git-state fixtures --------------------------------------------------

/// Run a git subcommand in a test repo with a deterministic identity and no
/// user/system config bleed-through. Returns the raw output; some setups
/// (a conflicting merge/cherry-pick/pop) exit non-zero on purpose.
fn git_run(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .unwrap()
}

/// [`git_run`] that asserts the subcommand succeeded.
fn git_ok(dir: &Path, args: &[&str]) {
    let out = git_run(dir, args);
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn write(dir: &Path, name: &str, contents: &str) {
    std::fs::write(dir.join(name), contents).unwrap();
}

/// A repo with one commit of `f.txt` == "base\n" on branch `main`.
fn init_repo(dir: &Path) {
    git_ok(dir, &["init", "-q"]);
    write(dir, "f.txt", "base\n");
    git_ok(dir, &["add", "."]);
    git_ok(dir, &["commit", "-q", "-m", "base"]);
    git_ok(dir, &["branch", "-m", "main"]);
}

/// `main` and `feature` each rewrite `f.txt`'s only line differently, so
/// merging or cherry-picking one onto the other conflicts. HEAD is `main`.
fn init_diverged(dir: &Path) {
    init_repo(dir);
    git_ok(dir, &["checkout", "-q", "-b", "feature"]);
    write(dir, "f.txt", "feature\n");
    git_ok(dir, &["commit", "-q", "-am", "feature"]);
    git_ok(dir, &["checkout", "-q", "main"]);
    write(dir, "f.txt", "mainline\n");
    git_ok(dir, &["commit", "-q", "-am", "mainline"]);
}

fn repo_state(dir: &Path) -> String {
    status_payload(dir)
        .unwrap()
        .get("repo_state")
        .unwrap()
        .as_str()
        .unwrap()
        .to_string()
}

/// A clone of a bare origin that carries `main` and `feature-x`, with no
/// local `feature-x` — the shape a fresh clone of a team's repository has,
/// down to the symbolic `origin/HEAD` every clone writes.
fn clone_of_an_origin_carrying_feature_x(dir: &Path) -> PathBuf {
    let source = dir.join("source");
    std::fs::create_dir(&source).unwrap();
    init_repo(&source);
    git_ok(&source, &["checkout", "-q", "-b", "feature-x"]);
    write(&source, "g.txt", "x\n");
    git_ok(&source, &["add", "."]);
    git_ok(&source, &["commit", "-q", "-m", "feature"]);
    git_ok(&source, &["checkout", "-q", "main"]);
    let origin = dir.join("origin.git");
    git_ok(
        dir,
        &[
            "clone",
            "-q",
            "--bare",
            source.to_str().unwrap(),
            origin.to_str().unwrap(),
        ],
    );
    let clone = dir.join("clone");
    git_ok(
        dir,
        &[
            "clone",
            "-q",
            origin.to_str().unwrap(),
            clone.to_str().unwrap(),
        ],
    );
    clone
}

mod branches;

mod history;
mod mutations;
mod network;
mod patches;
mod status;
mod unpushed;

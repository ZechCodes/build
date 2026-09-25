//! Candidate validation is part of the final deadline, and never holds the app mutex.

use super::workspace_reclaim::{
    build_output_in, finish, lifecycle, linked_workspace, now_ms, pruning, root_and_checkout,
};
use super::*;
use crate::git_fixture::git_in;
use crate::reclaim::GitProbe;
use std::time::{Duration, Instant};

#[test]
fn final_candidate_validation_times_out_off_lock_and_preserves_every_artifact() {
    let (tmp, state, _project, ws, issue) = linked_workspace();
    finish(&state, &issue);
    let (_root, checkout) = root_and_checkout(&state, &ws);
    let first = build_output_in(&checkout);
    let data = checkout.join(".build/data");
    std::fs::create_dir_all(&data).unwrap();
    for n in 0..5_000 {
        std::fs::write(data.join(format!("file-{n}")), "tracked\n").unwrap();
    }
    git_in(&checkout, &["add", "-f", ".build/data"]);
    git_in(&checkout, &["commit", "-m", "large index"]);
    git_in(&checkout, &["push", "origin", "HEAD"]);
    let outputs: Vec<_> = (0..128)
        .map(|n| {
            let path = checkout.join(format!("package-{n}/node_modules"));
            std::fs::create_dir_all(&path).unwrap();
            std::fs::write(path.join("blob"), "build output").unwrap();
            path
        })
        .collect();
    let marker = tmp.path().join("candidate-validation-started");
    let policy = crate::reclaim::ReclaimPolicy {
        final_check_time: Duration::from_secs(5),
        git: GitProbe::bridge().pause_during_candidate_validation(&marker),
        ..pruning()
    };
    let swept = state.clone();
    let sweep = std::thread::spawn(move || AppState::sweep_workspaces(&swept, &policy, now_ms()));
    let waiting = Instant::now();
    while !marker.exists() && !sweep.is_finished() && waiting.elapsed() < Duration::from_secs(60) {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(marker.exists(), "the child must reach candidate validation");
    let validating = Instant::now();
    let pid: i32 = std::fs::read_to_string(&marker)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    {
        let app = state
            .try_lock()
            .expect("candidate validation must leave app RPCs available");
        assert!(app.workspace_reserved(&ws));
    }
    sweep.join().unwrap();
    assert!(
        validating.elapsed() < Duration::from_secs(10),
        "final candidate deadline must terminate the sweep"
    );
    assert_eq!(
        unsafe { libc::kill(pid, 0) },
        -1,
        "timed-out candidate child was reaped"
    );
    assert!(first.exists());
    assert!(outputs.iter().all(|path| path.join("blob").exists()));
    let verdict = lifecycle(&state, &ws);
    assert_eq!(verdict["idle"], false, "{verdict}");
    assert_eq!(verdict["reclaimable"], false, "{verdict}");
    assert!(verdict["holds"]
        .as_array()
        .unwrap()
        .iter()
        .any(|hold| hold == "unmeasured"));
    assert!(!state.lock().unwrap().workspace_reserved(&ws));
}

/// Removing an ignore rule after the child validated it must keep the files.
#[test]
fn changing_ignore_rules_after_final_validation_preserves_the_candidate() {
    use crate::app::workspaces::PrunePhase;
    use std::cell::Cell;
    let (_tmp, state, _project, ws, issue) = linked_workspace();
    finish(&state, &issue);
    let (_root, checkout) = root_and_checkout(&state, &ws);
    let output = build_output_in(&checkout);
    let git_dir = git2::Repository::open(&checkout)
        .unwrap()
        .path()
        .to_path_buf();
    let common = std::fs::read_to_string(git_dir.join("commondir"))
        .map(|path| git_dir.join(path.trim()))
        .unwrap_or(git_dir);
    let changed = Cell::new(false);
    AppState::sweep_workspaces_racing(&state, &pruning(), now_ms(), &|phase| {
        if phase == PrunePhase::Validated {
            std::fs::write(common.join("info/exclude"), "").unwrap();
            changed.set(true);
        }
    });
    assert!(
        changed.get(),
        "the candidate must reach the final locked check"
    );
    assert!(output.join("pkg/index.js").exists());
    let verdict = lifecycle(&state, &ws);
    assert_eq!(verdict["pruned_bytes"], 0);
    assert_eq!(verdict["reclaimable"], false, "{verdict}");
}

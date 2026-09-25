use super::*;
use crate::git_fixture::{git_in, init_repo_named};
use crate::reclaim::Budget;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

fn budget(time: Duration) -> Budget {
    Budget::new(2_000_000, time, Arc::new(AtomicBool::new(false)))
}

fn pushed_checkout() -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let source = init_repo_named(temp.path(), "source");
    std::fs::write(source.join(".gitignore"), "node_modules/\n").unwrap();
    git_in(&source, &["add", ".gitignore"]);
    git_in(&source, &["commit", "-m", "ignore output"]);
    git_in(&source, &["config", "receive.denyCurrentBranch", "ignore"]);
    let checkout = temp.path().join("checkout");
    git_in(
        temp.path(),
        &[
            "clone",
            source.to_str().unwrap(),
            checkout.to_str().unwrap(),
        ],
    );
    (temp, checkout)
}

#[test]
fn committed_and_pushed_candidate_is_rejected_even_when_git_is_clean() {
    let (_temp, checkout) = pushed_checkout();
    let candidate = checkout.join("node_modules");
    std::fs::create_dir(&candidate).unwrap();
    std::fs::write(candidate.join("module.js"), "source").unwrap();
    git_in(&checkout, &["add", "-f", "node_modules/module.js"]);
    git_in(&checkout, &["commit", "-m", "vendor module"]);
    git_in(&checkout, &["push", "origin", "HEAD"]);

    let reading = GitProbe::bridge()
        .read_with_candidates(
            &checkout,
            &[PathBuf::from("node_modules")],
            &budget(Duration::from_secs(10)),
        )
        .unwrap();

    assert!(reading.read);
    assert!(!reading.dirty);
    assert_eq!(reading.pushes, 0);
    assert!(!reading.candidates_valid);
    assert!(candidate.join("module.js").exists());
}

#[test]
fn index_change_after_child_is_visible_to_cheap_final_check() {
    let (_temp, checkout) = pushed_checkout();
    let candidate = checkout.join("node_modules");
    std::fs::create_dir(&candidate).unwrap();
    std::fs::write(candidate.join("module.js"), "generated").unwrap();
    let reading = GitProbe::bridge()
        .read_with_candidates(
            &checkout,
            &[PathBuf::from("node_modules")],
            &budget(Duration::from_secs(10)),
        )
        .unwrap();
    assert!(reading.candidates_valid, "{reading:?}");
    let snapshot = reading.index_snapshot.unwrap();
    assert!(snapshot.unchanged());

    git_in(&checkout, &["add", "-f", "node_modules/module.js"]);

    assert!(!snapshot.unchanged());
}

#[test]
fn ignore_rule_change_after_child_invalidates_final_snapshot() {
    let (_temp, checkout) = pushed_checkout();
    let candidate = checkout.join("node_modules");
    std::fs::create_dir(&candidate).unwrap();
    std::fs::write(candidate.join("module.js"), "generated").unwrap();
    let reading = GitProbe::bridge()
        .read_with_candidates(
            &checkout,
            &[PathBuf::from("node_modules")],
            &budget(Duration::from_secs(10)),
        )
        .unwrap();
    assert!(reading.candidates_valid, "{reading:?}");
    assert!(reading
        .ignore_snapshots
        .iter()
        .all(IndexSnapshot::unchanged));

    std::fs::write(checkout.join(".gitignore"), "").unwrap();

    assert!(reading
        .ignore_snapshots
        .iter()
        .any(|snapshot| !snapshot.unchanged()));
    assert!(!git2::Repository::open(&checkout)
        .unwrap()
        .is_path_ignored(Path::new("node_modules"))
        .unwrap());
}

#[test]
fn common_exclude_and_config_selection_changes_invalidate_final_snapshot() {
    let (_temp, checkout) = pushed_checkout();
    let candidate = checkout.join("target");
    std::fs::create_dir(&candidate).unwrap();
    std::fs::write(candidate.join("blob"), "generated").unwrap();
    let exclude = checkout.join(".git/info/exclude");
    std::fs::write(&exclude, "target/\n").unwrap();
    let probe = GitProbe::bridge();
    let read = || {
        probe
            .read_with_candidates(
                &checkout,
                &[PathBuf::from("target")],
                &budget(Duration::from_secs(10)),
            )
            .unwrap()
    };

    let first = read();
    assert!(first.candidates_valid, "{first:?}");
    std::fs::write(&exclude, "").unwrap();
    assert!(first
        .ignore_snapshots
        .iter()
        .any(|snapshot| !snapshot.unchanged()));

    let custom = checkout.parent().unwrap().join("global-ignore");
    std::fs::write(&custom, "target/\n").unwrap();
    git_in(
        &checkout,
        &["config", "core.excludesFile", custom.to_str().unwrap()],
    );
    let second = read();
    assert!(second.candidates_valid, "{second:?}");
    let replacement = checkout.parent().unwrap().join("replacement-ignore");
    std::fs::write(&replacement, "target/\n").unwrap();
    git_in(
        &checkout,
        &["config", "core.excludesFile", replacement.to_str().unwrap()],
    );
    assert!(second
        .ignore_snapshots
        .iter()
        .any(|snapshot| !snapshot.unchanged()));
}

#[test]
fn many_candidate_snapshots_do_not_fill_the_child_pipe() {
    let (_temp, checkout) = pushed_checkout();
    let candidates = (0..128)
        .map(|index| {
            let relative = PathBuf::from(format!("package-{index}/node_modules"));
            std::fs::create_dir_all(checkout.join(&relative)).unwrap();
            relative
        })
        .collect::<Vec<_>>();

    let reading = GitProbe::bridge()
        .read_with_candidates(&checkout, &candidates, &budget(Duration::from_secs(5)))
        .unwrap();

    assert!(reading.candidates_valid, "{reading:?}");
    assert!(reading.ignore_snapshots.len() >= 128);
}

#[test]
fn candidate_validation_is_killed_at_final_check_deadline() {
    let (_temp, checkout) = pushed_checkout();
    let marker = checkout.parent().unwrap().join("candidate-started");
    let many = checkout.join(".build/many");
    std::fs::create_dir_all(&many).unwrap();
    for index in 0..5_000 {
        std::fs::write(many.join(format!("{index}.txt")), b"tracked").unwrap();
    }
    git_in(&checkout, &["add", ".build/many"]);
    git_in(&checkout, &["commit", "-q", "-m", "many tracked files"]);
    git_in(&checkout, &["push", "origin", "HEAD"]);
    for index in 0..128 {
        std::fs::create_dir_all(checkout.join(format!("{index}/node_modules"))).unwrap();
    }
    let candidates = (0..128)
        .map(|index| PathBuf::from(format!("{index}/node_modules")))
        .collect::<Vec<_>>();
    let probe = GitProbe::bridge().pause_during_candidate_validation(&marker);
    let started = std::time::Instant::now();

    let result =
        probe.read_with_candidates(&checkout, &candidates, &budget(Duration::from_secs(5)));

    assert_eq!(result, Err(Unfinished));
    assert!(
        marker.exists(),
        "the child must reach candidate validation before timing out"
    );
    assert!(started.elapsed() < Duration::from_secs(10));
    let pid: i32 = std::fs::read_to_string(&marker).unwrap().parse().unwrap();
    // SAFETY: signal zero only asks whether the child still exists.
    assert_ne!(
        unsafe { libc::kill(pid, 0) },
        0,
        "the timed-out child was reaped"
    );
    assert!(checkout.join("0/node_modules").exists());
}

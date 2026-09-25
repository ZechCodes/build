use super::*;
use crate::git_fixture::{git_in, init_repo_named};
use std::path::Path;
use std::sync::atomic::AtomicBool;

const HOUR_MS: i64 = 60 * 60 * 1000;

fn now_ms() -> i64 {
    i64::try_from(crate::agent::now_ms()).unwrap()
}

/// The default budget, which nothing here comes near.
fn budget() -> Budget {
    ReclaimPolicy::default().budget(Default::default())
}

/// A budget of `entries` directory entries.
fn tight(entries: u64) -> Budget {
    Budget::new(
        entries,
        std::time::Duration::from_secs(60),
        Default::default(),
    )
}

/// Everything `build_output` finds, by path.
fn found(subject: &Subject) -> Vec<PathBuf> {
    subject
        .build_output(&budget())
        .unwrap()
        .into_iter()
        .map(|artifact| artifact.path)
        .collect()
}

/// Measure, then drop what `build_output` finds the way the service does:
/// moved into the trash, the trash emptied.
fn prune(subject: &Subject) -> u64 {
    let Some(guard) = subject
        .boundary
        .as_ref()
        .and_then(|boundary| boundary.validate().ok())
    else {
        return 0;
    };
    let artifacts = subject.build_output(&budget()).unwrap();
    let moved = guard.move_to_trash(&artifacts, &budget());
    guard.empty_trash(&budget());
    moved.iter().map(|artifact| artifact.bytes).sum()
}

/// A workspace root holding one checkout cloned from an origin, so everything
/// committed is already pushed: `(tempdir, root, checkout)`.
fn pushed_workspace() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let source = init_repo_named(tmp.path(), "source");
    std::fs::write(source.join(".gitignore"), "node_modules/\ntarget\n").unwrap();
    git_in(&source, &["add", ".gitignore"]);
    git_in(&source, &["commit", "-m", "ignore build output"]);
    let root = tmp.path().join("managed/proj-1/ws");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join(crate::workspace::MANIFEST_FILE), b"{}").unwrap();
    let checkout = root.join("Build");
    git_in(
        tmp.path(),
        &[
            "clone",
            source.to_str().unwrap(),
            checkout.to_str().unwrap(),
        ],
    );
    (tmp, root, checkout)
}

fn fill(directory: &Path, bytes: usize) {
    std::fs::create_dir_all(directory).unwrap();
    std::fs::write(directory.join("blob"), vec![7u8; bytes]).unwrap();
}

fn subject(root: &Path, checkout: &Path) -> Subject {
    let anchor = containment::StorageAnchor::new(root.parent().unwrap().parent().unwrap());
    anchor.capture_workspace(root).unwrap();
    Subject {
        workspace_id: "ws-1".into(),
        project_id: "proj-1".into(),
        name: "ws".into(),
        root: root.to_path_buf(),
        boundary: Some(
            containment::WorkspaceBoundary::new(anchor, root, vec![checkout.to_path_buf()])
                .unwrap(),
        ),
        repositories: vec![(checkout.to_path_buf(), Some("main".into()))],
        holds: Vec::new(),
        issues: Vec::new(),
        conversation_activity_ms: None,
        previous: None,
    }
}

/// A day and more from now: everything on disk has been quiet for longer than
/// the idle threshold.
fn tomorrow() -> i64 {
    now_ms() + 25 * HOUR_MS
}

/// Only ignored, untracked directories with a build output name count, and a
/// walk never follows a symlink out of the checkout.
#[test]
fn build_output_is_ignored_untracked_and_conventionally_named() {
    let (_tmp, _root, checkout) = pushed_workspace();
    fill(&checkout.join("spa/node_modules/pkg"), 10);
    fill(&checkout.join("bridge/target/debug"), 10);
    fill(&checkout.join("dist"), 10);
    std::fs::write(checkout.join("dist/kept.txt"), "tracked").unwrap();
    git_in(&checkout, &["add", "-f", "dist/kept.txt"]);
    git_in(&checkout, &["commit", "-m", "a committed dist"]);
    fill(&checkout.join("tools/.venv"), 10);
    let elsewhere = tempfile::tempdir().unwrap();
    fill(&elsewhere.path().join("node_modules"), 10);
    #[cfg(unix)]
    std::os::unix::fs::symlink(elsewhere.path(), checkout.join("linked")).unwrap();

    let checkout = std::fs::canonicalize(checkout).unwrap();
    let found = artifacts::find(&checkout, &budget()).unwrap();

    assert_eq!(
        found,
        vec![
            checkout.join("bridge/target"),
            checkout.join("spa/node_modules"),
        ],
        "dist is tracked, .venv is not ignored here, and the link is not followed"
    );
}

/// Dropped build output goes into the workspace's trash in one rename, and
/// emptying the trash frees what the inspection counted.
#[test]
fn pruning_moves_the_directory_aside_then_empties_the_trash() {
    let (_tmp, root, checkout) = pushed_workspace();
    fill(&checkout.join("spa/node_modules"), 64 * 1024);
    let subject = subject(&root, &checkout);

    let freed = prune(&subject);

    assert!(freed >= 64 * 1024, "freed {freed}");
    assert!(!checkout.join("spa/node_modules").exists());
    let trash = root.join(".build/reclaim");
    assert_eq!(std::fs::read_dir(&trash).unwrap().count(), 0, "emptied");
    assert!(checkout.join("README.md").exists(), "the source stays");
}

/// A checkout replaced by a link to somewhere outside the workspace is not the
/// workspace's to prune: nothing of what it points at is touched.
#[cfg(unix)]
#[test]
fn a_checkout_linked_out_of_the_workspace_loses_nothing() {
    let (tmp, root, checkout) = pushed_workspace();
    fill(&checkout.join("bridge/target"), 1024);
    let outside = tmp.path().join("outside");
    std::fs::rename(&checkout, &outside).unwrap();
    std::os::unix::fs::symlink(&outside, &checkout).unwrap();

    assert!(found(&subject(&root, &checkout)).is_empty());
    assert_eq!(prune(&subject(&root, &checkout)), 0);
    assert!(outside.join("bridge/target/blob").exists());
}

/// A repository nested inside a checkout, ignored by it, keeps what it
/// commits: the outer index says nothing about its files.
#[test]
fn a_nested_repository_keeps_its_committed_build_output() {
    let (_tmp, root, checkout) = pushed_workspace();
    std::fs::write(checkout.join(".git/info/exclude"), "vendor/\n").unwrap();
    let nested = checkout.join("vendor/pkg");
    std::fs::create_dir_all(&nested).unwrap();
    git_in(&nested, &["init", "-q"]);
    fill(&nested.join("dist"), 1024);
    git_in(&nested, &["add", "-f", "dist/blob"]);
    git_in(
        &nested,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-qm",
            "dist",
        ],
    );
    fill(&checkout.join("spa/node_modules"), 1024);

    let pruned = prune(&subject(&root, &checkout));

    assert!(pruned > 0, "the outer checkout's own output still goes");
    assert!(nested.join("dist/blob").exists(), "the nested dist stays");
    assert!(!checkout.join("spa/node_modules").exists());
}

/// Build output with a repository somewhere inside it stays: whatever that
/// repository has not pushed would go with it.
#[test]
fn build_output_holding_a_repository_stays() {
    let (_tmp, root, checkout) = pushed_workspace();
    let package = checkout.join("spa/node_modules/linked-pkg");
    std::fs::create_dir_all(&package).unwrap();
    git_in(&package, &["init", "-q"]);

    assert!(found(&subject(&root, &checkout)).is_empty());
    assert!(package.join(".git").exists());
}

/// A measurement that runs out of budget is held as unmeasured, is not called
/// idle, and so is neither announced nor pruned.
#[test]
fn a_measurement_out_of_budget_is_held_and_not_idle() {
    let (_tmp, root, checkout) = pushed_workspace();
    for index in 0..20 {
        std::fs::write(checkout.join(format!("file-{index}.txt")), "x").unwrap();
    }

    let record =
        subject(&root, &checkout).measure(tomorrow(), &ReclaimPolicy::default(), &tight(5));

    assert!(!record.idle, "{record:?}");
    assert!(!record.reclaimable);
    assert!(
        record.holds.contains(&HOLD_UNMEASURED.to_string()),
        "{record:?}"
    );
    assert_eq!(
        record.notice_due(tomorrow(), &ReclaimPolicy::default()),
        NoticeDue::No
    );
    assert!(subject(&root, &checkout).build_output(&tight(3)).is_err());
}

/// Stands in for `build-bridge measure-git` under `cargo test`: the probe
/// runs this binary with only this test selected and the repository in the
/// environment. Run any other way, it does nothing.
#[test]
fn git_reading_child() {
    if std::env::var_os(git_probe::REPOSITORY_VAR).is_some() {
        println!("{}", git_reading_line());
    }
}

/// A probe that writes its pid to `pid` and then never finishes.
fn hanging_probe(pid: &Path) -> GitProbe {
    GitProbe::command(
        "/bin/sh",
        &[
            "-c",
            &format!("echo $$ > '{}'; exec sleep 60", pid.display()),
        ],
    )
}

/// Whether the process `pid` names is gone: killed, and reaped.
fn gone(pid: &Path) -> bool {
    let pid: i32 = std::fs::read_to_string(pid)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    // SAFETY: signal 0 only asks whether the process exists.
    unsafe { libc::kill(pid, 0) != 0 }
}

/// A clean, pushed checkout of 5,000 tracked files, measured on a 2 ms
/// budget: its Git reading cannot finish in time, so the workspace is held
/// as unmeasured, not idle and not reclaimable. With the default budget the
/// same workspace is idle and reclaimable.
#[test]
fn five_thousand_files_over_a_tiny_budget_are_unmeasured() {
    let tmp = tempfile::tempdir().unwrap();
    let source = init_repo_named(tmp.path(), "source");
    let many = source.join(".build/many");
    std::fs::create_dir_all(&many).unwrap();
    for index in 0..5_000 {
        std::fs::write(many.join(format!("{index}.txt")), "x").unwrap();
    }
    git_in(&source, &["add", "."]);
    git_in(&source, &["commit", "-q", "-m", "many files"]);
    let root = tmp.path().join("managed/proj-1/ws");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join(crate::workspace::MANIFEST_FILE), b"{}").unwrap();
    let checkout = root.join("Build");
    git_in(
        tmp.path(),
        &[
            "clone",
            "-q",
            source.to_str().unwrap(),
            checkout.to_str().unwrap(),
        ],
    );
    let policy = ReclaimPolicy::default();
    let two_ms = Budget::new(
        DEFAULT_MEASURE_ENTRIES,
        std::time::Duration::from_millis(2),
        Default::default(),
    );

    let starved = subject(&root, &checkout).measure(tomorrow(), &policy, &two_ms);

    assert!(!starved.idle, "{starved:?}");
    assert!(!starved.reclaimable, "{starved:?}");
    assert!(starved.holds.contains(&HOLD_UNMEASURED.to_string()));
    let fed = subject(&root, &checkout).measure(tomorrow(), &policy, &budget());
    assert!(fed.idle && fed.reclaimable, "{fed:?}");
}

/// A Git reading still running when the budget runs out is killed and
/// reaped, and the measurement says it did not finish.
#[test]
fn a_reading_past_its_deadline_is_killed() {
    let (tmp, _root, checkout) = pushed_workspace();
    let pid = tmp.path().join("reading.pid");
    let short = Budget::new(
        DEFAULT_MEASURE_ENTRIES,
        std::time::Duration::from_millis(500),
        Default::default(),
    );
    let started = std::time::Instant::now();

    let measured = measure_repositories(&[checkout], &short, &hanging_probe(&pid));

    assert!(measured.unfinished, "{measured:?}");
    assert!(started.elapsed() < std::time::Duration::from_secs(20));
    assert!(gone(&pid), "the reading was killed");
}

/// The daemon stopping kills a Git reading in flight.
#[test]
fn a_stop_kills_a_reading_in_flight() {
    let (tmp, _root, checkout) = pushed_workspace();
    let pid = tmp.path().join("reading.pid");
    let stop = Arc::new(AtomicBool::new(false));
    let stopper = {
        let (stop, pid) = (Arc::clone(&stop), pid.clone());
        std::thread::spawn(move || {
            while !pid.exists() {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            stop.store(true, std::sync::atomic::Ordering::Relaxed);
        })
    };
    let started = std::time::Instant::now();

    let measured = measure_repositories(
        &[checkout],
        &ReclaimPolicy::default().budget(stop),
        &hanging_probe(&pid),
    );

    stopper.join().unwrap();
    assert!(measured.unfinished, "{measured:?}");
    assert!(started.elapsed() < std::time::Duration::from_secs(20));
    assert!(gone(&pid), "the reading was killed");
}

/// A size walk that runs out holds the workspace as unmeasured.
#[test]
fn a_size_walk_out_of_budget_is_unmeasured() {
    let (_tmp, root, checkout) = pushed_workspace();
    fill(&checkout.join("node_modules/pkg"), 1024);
    // Enough for the activity walk, which skips build output, and not for
    // the size walk, which counts it.
    let entries = (1..10_000)
        .find(|entries| newest_change_ms(&root, &tight(*entries)).is_ok())
        .unwrap();

    let record =
        subject(&root, &checkout).measure(tomorrow(), &ReclaimPolicy::default(), &tight(entries));

    assert!(!record.idle, "{record:?}");
    assert!(record.holds.contains(&HOLD_UNMEASURED.to_string()));
    assert!(record.size_bytes.is_none());
}

/// A stopped daemon stops a walk at its next entry.
#[test]
fn a_stop_ends_a_walk() {
    let (_tmp, root, _checkout) = pushed_workspace();
    let stop = Arc::new(AtomicBool::new(true));
    let stopped = ReclaimPolicy::default().budget(stop);
    assert!(newest_change_ms(&root, &stopped).is_err());
    assert!(artifacts::size_on_disk(&root, &stopped).is_err());
}

/// Pruning is off unless the switch is set.
#[test]
fn pruning_is_off_unless_switched_on() {
    let unset = ReclaimPolicy::from_vars(|_| None);
    assert!(!unset.prune);
    for on in ["1", "true", "ON", " yes "] {
        let policy = ReclaimPolicy::from_vars(|name| {
            (name == "BRIDGE_WORKSPACE_PRUNE").then(|| on.to_string())
        });
        assert!(policy.prune, "{on:?}");
    }
    let off = ReclaimPolicy::from_vars(|name| {
        (name == "BRIDGE_WORKSPACE_PRUNE").then(|| "0".to_string())
    });
    assert!(!off.prune);
}

/// The device's settings (#167) move the idle threshold and the prune switch,
/// except where the environment set them: the variable pins its value.
#[test]
fn settings_move_the_policy_except_where_the_environment_pinned_it() {
    let chosen = ReclaimSettings {
        idle_after_secs: Some(7200),
        prune: Some(true),
    };
    let unpinned = ReclaimPolicy::from_vars(|_| None);
    assert_eq!(unpinned.pinned(), Vec::<&str>::new());
    let moved = unpinned.with_settings(&chosen);
    assert_eq!(moved.idle_after, std::time::Duration::from_secs(7200));
    assert!(moved.prune);
    assert_eq!(
        unpinned.with_settings(&ReclaimSettings::default()),
        unpinned,
        "nothing chosen changes nothing"
    );

    let pinned = ReclaimPolicy::from_vars(|name| match name {
        "BRIDGE_WORKSPACE_IDLE_SECS" => Some("3600".to_string()),
        "BRIDGE_WORKSPACE_PRUNE" => Some("0".to_string()),
        _ => None,
    });
    assert_eq!(
        pinned.pinned(),
        vec!["workspace_idle_secs", "workspace_prune"]
    );
    let kept = pinned.with_settings(&chosen);
    assert_eq!(kept.idle_after, std::time::Duration::from_secs(3600));
    assert!(!kept.prune);

    let unreadable = ReclaimPolicy::from_vars(|name| {
        (name == "BRIDGE_WORKSPACE_IDLE_SECS").then(|| "soon".to_string())
    });
    assert_eq!(
        unreadable.pinned(),
        Vec::<&str>::new(),
        "a value the bridge cannot read pins nothing"
    );
}

/// Activity is somebody's files. Git's own bookkeeping, Build's per-agent
/// configuration, the manifest and build output are not.
#[test]
fn only_the_work_itself_counts_as_a_change() {
    let (_tmp, root, checkout) = pushed_workspace();
    let before = newest_change_ms(&root, &budget()).unwrap().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    fill(&checkout.join("spa/node_modules"), 1);
    fill(&root.join(".build"), 1);
    std::fs::write(root.join(crate::workspace::MANIFEST_FILE), "{}").unwrap();
    git_in(&checkout, &["status"]);
    assert_eq!(newest_change_ms(&root, &budget()), Ok(Some(before)));

    std::fs::write(checkout.join("README.md"), "edited\n").unwrap();
    assert!(newest_change_ms(&root, &budget()).unwrap().unwrap() > before);
}

#[test]
fn a_notice_opens_repeats_after_a_quiet_period_and_starts_over_on_activity() {
    let policy = ReclaimPolicy::default();
    let mut record = LifecycleRecord {
        idle: true,
        last_activity_ms: Some(0),
        ..LifecycleRecord::default()
    };
    assert_eq!(record.notice_due(30 * HOUR_MS, &policy), NoticeDue::First);

    record.noticed_at_ms = Some(30 * HOUR_MS);
    assert_eq!(record.notice_due(40 * HOUR_MS, &policy), NoticeDue::No);
    assert_eq!(record.notice_due(54 * HOUR_MS, &policy), NoticeDue::Again);

    record.last_activity_ms = Some(35 * HOUR_MS);
    assert_eq!(
        record.notice_due(60 * HOUR_MS, &policy),
        NoticeDue::First,
        "work after the notice makes going quiet news again"
    );

    record.idle = false;
    assert_eq!(record.notice_due(90 * HOUR_MS, &policy), NoticeDue::No);
}

/// A quiet workspace nothing holds is reclaimable, and sized.
#[test]
fn a_quiet_workspace_nothing_holds_is_reclaimable() {
    let (_tmp, root, checkout) = pushed_workspace();
    fill(&checkout.join("spa/node_modules"), 128 * 1024);

    let record =
        subject(&root, &checkout).measure(tomorrow(), &ReclaimPolicy::default(), &budget());

    assert!(record.idle && record.reclaimable, "{record:?}");
    assert!(record.size_bytes.unwrap() >= 128 * 1024, "{record:?}");
    assert!(
        checkout.join("spa/node_modules").exists(),
        "measuring removes nothing"
    );
}

/// Work that is only here holds the workspace, and nothing is pruned.
#[test]
fn uncommitted_work_holds_the_workspace_and_nothing_is_pruned() {
    let (_tmp, root, checkout) = pushed_workspace();
    fill(&checkout.join("spa/node_modules"), 1024);
    std::fs::write(checkout.join("notes.md"), "mine\n").unwrap();
    std::fs::write(checkout.join("README.md"), "changed\n").unwrap();

    let record =
        subject(&root, &checkout).measure(tomorrow(), &ReclaimPolicy::default(), &budget());

    assert!(record.idle, "{record:?}");
    assert!(!record.reclaimable);
    assert_eq!(record.holds, vec!["dirty"]);
    assert_eq!(record.dirty_files, 2);
}

/// Commits no remote has hold it too, and are counted.
#[test]
fn unpushed_commits_hold_the_workspace() {
    let (_tmp, root, checkout) = pushed_workspace();
    std::fs::write(checkout.join("README.md"), "changed\n").unwrap();
    git_in(&checkout, &["commit", "-am", "local only"]);

    let record =
        subject(&root, &checkout).measure(tomorrow(), &ReclaimPolicy::default(), &budget());

    assert_eq!(record.holds, vec!["unpushed"]);
    assert_eq!(record.unpushed_commits, 1);
}

/// A workspace with recent activity is not idle, and is not sized.
#[test]
fn an_active_workspace_is_not_idle_or_sized() {
    let (_tmp, root, checkout) = pushed_workspace();
    fill(&checkout.join("spa/node_modules"), 1024);

    let active = subject(&root, &checkout).measure(now_ms(), &ReclaimPolicy::default(), &budget());
    assert!(!active.idle && active.reclaimable, "{active:?}");
    assert_eq!(active.size_bytes, None, "only an idle workspace is sized");
}

/// Holds read under the mutex join the measured ones, in one order.
#[test]
fn holds_read_under_the_mutex_are_kept_in_reading_order() {
    let (_tmp, root, checkout) = pushed_workspace();
    std::fs::write(checkout.join("notes.md"), "mine\n").unwrap();
    let mut held = subject(&root, &checkout);
    held.holds = vec![
        HOLD_ISSUE_OPEN,
        crate::workspace::FINISH_BLOCKER_AGENT_WORKING,
    ];

    let record = held.measure(tomorrow(), &ReclaimPolicy::default(), &budget());

    assert_eq!(record.holds, vec!["agent_working", "dirty", "issue_open"]);
}

/// A notice and a pruning carry over while the workspace stays quiet, and are
/// forgotten once somebody works in it again.
#[test]
fn the_previous_notice_and_pruning_carry_over_only_while_quiet() {
    let (_tmp, root, checkout) = pushed_workspace();
    let quiet_since = newest_change_ms(&root, &budget()).unwrap().unwrap();
    let mut carried = subject(&root, &checkout);
    carried.previous = Some(LifecycleRecord {
        noticed_at_ms: Some(quiet_since + HOUR_MS),
        pruned_bytes: 5,
        pruned_at_ms: Some(quiet_since + HOUR_MS),
        ..LifecycleRecord::default()
    });
    let record = carried.measure(tomorrow(), &ReclaimPolicy::default(), &budget());
    assert_eq!(record.noticed_at_ms, Some(quiet_since + HOUR_MS));
    assert_eq!(record.pruned_bytes, 5);

    carried.conversation_activity_ms = Some(quiet_since + 2 * HOUR_MS);
    let a_day_after_that = tomorrow() + 3 * HOUR_MS;
    let record = carried.measure(a_day_after_that, &ReclaimPolicy::default(), &budget());
    assert_eq!(record.pruned_bytes, 0, "{record:?}");
    assert_eq!(record.pruned_at_ms, None);
    assert_eq!(
        record.notice_due(a_day_after_that, &ReclaimPolicy::default()),
        NoticeDue::First
    );
}

#[test]
fn bytes_read_the_way_a_person_says_them() {
    assert_eq!(human_bytes(512), "512 B");
    assert_eq!(human_bytes(17_200_000_000), "17.2 GB");
    assert_eq!(human_bytes(640_000_000), "640 MB");
}

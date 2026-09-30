//! Everything a sync starts runs as nobody's command: no prompt, no hook,
//! no submodule fetch and no maintenance left behind (#268).

use super::*;

fn executable(path: &Path, script: &str) {
    std::fs::write(path, script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

/// A hook in the base checkout that leaves `marker` behind when git runs it.
fn hook(pair: &Pair, name: &str, marker: &Path) {
    executable(
        &pair.base.join(".git/hooks").join(name),
        &format!("#!/bin/sh\necho ran >> {}\n", marker.display()),
    );
}

/// The user did not start this merge, so nothing they set to run on one
/// runs: not `post-merge`, not `reference-transaction`.
#[test]
fn the_fast_forward_of_the_base_checkout_runs_no_hook() {
    let pair = pair();
    let marker = pair.base.parent().unwrap().join("hooked");
    hook(&pair, "post-merge", &marker);
    hook(&pair, "reference-transaction", &marker);
    commit(&pair.upstream, "theirs.txt", "theirs\n");

    let report = sync_base(&pair.base, "main", NOW);

    assert_eq!(report.outcome, SyncOutcome::FastForwarded { commits: 1 });
    assert!(!marker.exists(), "a hook ran");
}

#[test]
fn the_fast_forward_of_a_base_ref_runs_no_hook() {
    let pair = pair();
    commit(&pair.upstream, "theirs.txt", "theirs\n");
    git_in(&pair.base, &["switch", "-q", "-c", "feature"]);
    let marker = pair.base.parent().unwrap().join("hooked");
    hook(&pair, "reference-transaction", &marker);

    let report = sync_base(&pair.base, "main", NOW);

    assert_eq!(report.outcome, SyncOutcome::FastForwarded { commits: 1 });
    assert!(!marker.exists(), "a hook ran");
}

/// Filters still run, since a checkout without them (git-lfs) would write the
/// wrong files; but they run unattended, so one that reaches the network for
/// a credential cannot ask for it.
#[test]
fn a_filter_the_fast_forward_runs_cannot_prompt() {
    let pair = pair();
    let seen = pair.base.parent().unwrap().join("filter-env");
    let filter = pair.base.parent().unwrap().join("filter.sh");
    executable(
        &filter,
        &format!(
            "#!/bin/sh\nprintf '%s|%s|%s' \"${{GIT_ASKPASS-unset}}\" \"$SSH_ASKPASS_REQUIRE\" \"$GIT_TERMINAL_PROMPT\" > {}\nexec cat\n",
            seen.display()
        ),
    );
    commit(&pair.upstream, ".gitattributes", "*.dat filter=probe\n");
    git_in(&pair.base, &["pull", "-q", "--ff-only"]);
    git_in(
        &pair.base,
        &["config", "filter.probe.smudge", filter.to_str().unwrap()],
    );
    commit(&pair.upstream, "data.dat", "payload\n");

    let report = sync_base(&pair.base, "main", NOW);

    assert_eq!(report.outcome, SyncOutcome::FastForwarded { commits: 1 });
    assert_eq!(
        std::fs::read_to_string(pair.base.join("data.dat")).unwrap(),
        "payload\n"
    );
    assert_eq!(std::fs::read_to_string(&seen).unwrap(), "|never|0");
}

/// The fetch is of the base branch alone: a checkout set to fetch its
/// submodules too does not, so a submodule's remote cannot fail the sync or
/// be asked for anything.
#[test]
fn the_fetch_leaves_submodules_alone() {
    let pair = pair();
    let library = init_repo_named(pair.base.parent().unwrap(), "library");
    git_in(
        &pair.upstream,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "-q",
            library.to_str().unwrap(),
            "library",
        ],
    );
    git_in(&pair.upstream, &["commit", "-q", "-m", "library"]);
    git_in(&pair.base, &["pull", "-q", "--ff-only"]);
    git_in(
        &pair.base,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "update",
            "-q",
            "--init",
        ],
    );
    git_in(&pair.base, &["config", "fetch.recurseSubmodules", "true"]);
    let missing = pair.base.parent().unwrap().join("gone.git");
    git_in(
        &pair.base.join("library"),
        &["remote", "set-url", "origin", missing.to_str().unwrap()],
    );
    commit(&pair.upstream, "theirs.txt", "theirs\n");

    let report = sync_base(&pair.base, "main", NOW);

    assert_eq!(report.outcome, SyncOutcome::FastForwarded { commits: 1 });
}

fn loose_objects(repo: &Path) -> usize {
    let counted = git_command(repo, &["count-objects"]).output().unwrap();
    String::from_utf8_lossy(&counted.stdout)
        .split_whitespace()
        .next()
        .and_then(|count| count.parse().ok())
        .unwrap()
}

/// Fetch and merge each start `maintenance run --auto` when they are done,
/// which can detach and outlive the deadline's kill. A sync starts none: the
/// loose objects a repository set to pack at the first chance holds are still
/// loose after it.
#[test]
fn a_sync_starts_no_maintenance() {
    let pair = pair();
    let blobs = pair.base.parent().unwrap().join("blobs");
    std::fs::create_dir(&blobs).unwrap();
    let paths: Vec<String> = (0..3000)
        .map(|n| {
            let path = blobs.join(n.to_string());
            std::fs::write(&path, format!("blob {n}\n")).unwrap();
            path.display().to_string()
        })
        .collect();
    let mut hashing = git_command(&pair.base, &["hash-object", "-w", "--stdin-paths"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    use std::io::Write as _;
    hashing
        .stdin
        .take()
        .unwrap()
        .write_all(paths.join("\n").as_bytes())
        .unwrap();
    assert!(hashing.wait().unwrap().success());
    for (key, value) in [
        ("gc.auto", "1"),
        ("gc.autoDetach", "false"),
        ("maintenance.autoDetach", "false"),
    ] {
        git_in(&pair.base, &["config", key, value]);
    }
    commit(&pair.upstream, "theirs.txt", "theirs\n");
    let before = loose_objects(&pair.base);
    assert!(before >= 3000, "{before}");

    let report = sync_base(&pair.base, "main", NOW);

    assert_eq!(report.outcome, SyncOutcome::FastForwarded { commits: 1 });
    assert!(
        loose_objects(&pair.base) >= before,
        "the sync packed objects"
    );
}

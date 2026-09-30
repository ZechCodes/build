//! A fetch that runs out of time is not a fetch that needs a person (#268):
//! only a remote that asked for one — a password, a passphrase, a key touch —
//! is marked as needing you.

use super::*;

/// An ssh remote whose `ssh` says `said` on stderr and then waits, the way
/// ssh waits on a security key.
fn ssh_remote_that_says(pair: &Pair, said: &str) {
    let ssh = pair.base.parent().unwrap().join("ssh.sh");
    std::fs::write(
        &ssh,
        format!("#!/bin/sh\necho '{said}' >&2\nexec sleep 30\n"),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    git_in(
        &pair.base,
        &[
            "remote",
            "set-url",
            "origin",
            "ssh://git@127.0.0.1/private.git",
        ],
    );
    git_in(
        &pair.base,
        &["config", "core.sshCommand", ssh.to_str().unwrap()],
    );
    // Real ssh answers git's `ssh -G` variant probe at once; this stand-in
    // would wait in it, where git drops what it says.
    git_in(&pair.base, &["config", "ssh.variant", "ssh"]);
}

const SHORT: Fetch = Fetch::Within(Duration::from_millis(500));

#[test]
fn a_fetch_that_never_answers_times_out_without_needing_you() {
    let pair = pair();
    git_in(
        &pair.base,
        &["remote", "set-url", "origin", &stalling_remote()],
    );
    let before = rev(&pair.base, "main");

    let started = Instant::now();
    let report = sync_base(&pair.base, "main", SHORT);

    assert!(started.elapsed() < Duration::from_secs(5), "was not killed");
    let failure = failed(&report);
    assert!(failure.timed_out, "{failure:?}");
    assert!(!failure.needs_you, "{failure:?}");
    assert!(failure.reason.contains("did not answer"), "{failure:?}");
    assert_eq!(rev(&pair.base, "main"), before);
}

/// A security key blinking for a touch nobody is there to give: ssh says so
/// and waits. That is a person being asked, so it needs you.
#[test]
fn a_fetch_waiting_on_a_key_touch_needs_you() {
    let pair = pair();
    ssh_remote_that_says(
        &pair,
        "Confirm user presence for key ED25519-SK SHA256:0000000000000000000000000000000000000000000",
    );

    let started = Instant::now();
    let report = sync_base(&pair.base, "main", SHORT);

    assert!(started.elapsed() < Duration::from_secs(5), "was not killed");
    let failure = failed(&report);
    assert!(failure.needs_you, "{failure:?}");
    assert!(failure.reason.contains("user presence"), "{failure:?}");
}

#[test]
fn an_ssh_remote_that_is_only_slow_does_not_need_you() {
    let pair = pair();
    ssh_remote_that_says(&pair, "debug1: Connecting to 127.0.0.1 port 22.");

    let report = sync_base(&pair.base, "main", SHORT);

    let failure = failed(&report);
    assert!(failure.timed_out, "{failure:?}");
    assert!(!failure.needs_you, "{failure:?}");
}

// Exact test bodies moved from the former inline test module.
use super::*;

#[test]
fn pull_flag_maps_modes_and_rejects_unknown() {
    assert_eq!(pull_flag("ff").unwrap(), "--ff-only");
    assert_eq!(pull_flag("merge").unwrap(), "--no-rebase");
    assert_eq!(pull_flag("rebase").unwrap(), "--rebase");
    assert!(pull_flag("octopus").is_err());
}
#[test]
fn run_with_timeout_kills_a_child_that_overruns() {
    // A `sleep 5` under a 200 ms cap must be killed and reported, not
    // waited out — this is the network-op backstop's core mechanism.
    let mut command = Command::new("sleep");
    command.arg("5");
    let started = Instant::now();
    let result = run_with_timeout(command, Duration::from_millis(200), "git fetch");
    let elapsed = started.elapsed();
    let error = result.unwrap_err();
    assert!(error.contains("timed out after"), "{error}");
    assert!(error.starts_with("git fetch"), "{error}");
    assert!(
        elapsed < Duration::from_secs(2),
        "killed promptly: {elapsed:?}"
    );
}
#[test]
fn run_with_timeout_returns_stdout_on_a_fast_success() {
    let mut command = Command::new("echo");
    command.arg("hello");
    let out = run_with_timeout(command, Duration::from_secs(5), "echo").unwrap();
    assert_eq!(out.trim(), "hello");
}

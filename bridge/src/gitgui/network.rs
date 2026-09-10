use super::history::{current_branch, open_repo};
use super::status::upstream_status;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub const GIT_NETWORK_TIMEOUT_SECS: u64 = 60;

pub(super) fn run_with_timeout(
    mut command: Command,
    timeout: Duration,
    op_label: &str,
) -> Result<String, String> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|e| format!("could not run {op_label}: {e}"))?;
    let mut stdout_pipe = child.stdout.take().expect("stdout is piped");
    let mut stderr_pipe = child.stderr.take().expect("stderr is piped");
    let stdout_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout_pipe.read_to_end(&mut buf);
        buf
    });
    let stderr_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr_pipe.read_to_end(&mut buf);
        buf
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child
            .try_wait()
            .map_err(|e| format!("could not wait for {op_label}: {e}"))?
        {
            Some(status) => break status,
            None => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!("{op_label} timed out after {}s", timeout.as_secs()));
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    };
    let stdout = String::from_utf8_lossy(&stdout_reader.join().unwrap_or_default()).into_owned();
    let stderr = String::from_utf8_lossy(&stderr_reader.join().unwrap_or_default()).into_owned();
    if !status.success() {
        return Err(format!(
            "{op_label} failed: {}",
            format!("{} {}", stderr.trim(), stdout.trim()).trim()
        ));
    }
    Ok(stdout)
}

/// Run a network git subcommand (fetch/pull/push) with `GIT_TERMINAL_PROMPT=0`
/// (fail fast on a credential prompt, never block on stdin) under the
/// [`GIT_NETWORK_TIMEOUT_SECS`] wall-clock cap.
fn run_git_network(repo_path: &Path, args: &[&str]) -> Result<String, String> {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(repo_path)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0");
    let op_label = format!("git {}", args.first().unwrap_or(&""));
    run_with_timeout(
        command,
        Duration::from_secs(GIT_NETWORK_TIMEOUT_SECS),
        &op_label,
    )
}

/// `git.fetch`: `git fetch --prune`. Safe in every scope — it only updates
/// remote-tracking refs, never the working tree.
pub fn fetch(repo_path: &Path) -> Result<(), String> {
    run_git_network(repo_path, &["fetch", "--prune"]).map(|_| ())
}

/// `git.pull` mode → the matching git flag. Unknown modes fail fast rather
/// than silently defaulting.
pub(super) fn pull_flag(mode: &str) -> Result<&'static str, String> {
    match mode {
        "ff" => Ok("--ff-only"),
        "merge" => Ok("--no-rebase"),
        "rebase" => Ok("--rebase"),
        other => Err(format!("unknown pull mode: {other}")),
    }
}

/// `git.pull`: `git pull` in the requested integration mode. Git's own message
/// passes through verbatim on failure (ff-only refusal, conflict); a conflicted
/// pull deliberately leaves the repo in a merging/rebasing state, which the
/// next `git.status` surfaces.
pub fn pull(repo_path: &Path, mode: &str) -> Result<(), String> {
    let flag = pull_flag(mode)?;
    run_git_network(repo_path, &["pull", flag]).map(|_| ())
}

/// `git.push`: `git push` (or `git push -u origin -- <branch>` when the branch
/// has no upstream yet). `force` upgrades to `--force-with-lease` — never a
/// bare `--force`. A detached HEAD has no branch to push and is refused.
pub fn push(repo_path: &Path, force: bool) -> Result<(), String> {
    let repo = open_repo(repo_path)?;
    if repo
        .head_detached()
        .map_err(|e| format!("cannot read HEAD: {e}"))?
    {
        return Err("cannot push a detached HEAD".to_string());
    }
    let branch = current_branch(&repo)?;
    let (upstream, _, _) = upstream_status(&repo);
    drop(repo);
    let mut args: Vec<String> = vec!["push".to_string()];
    if force {
        args.push("--force-with-lease".to_string());
    }
    if upstream.is_none() {
        args.push("-u".to_string());
        args.push("origin".to_string());
        args.push("--".to_string());
        args.push(branch);
    }
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    run_git_network(repo_path, &arg_refs).map(|_| ())
}

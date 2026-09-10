use super::history::{commit_summary_json, head_commit_id, open_repo};
use super::status::{is_untracked_status, repo_state_label};
use serde_json::Value;
use std::path::Path;

pub(super) fn stageable_paths(paths: &[String]) -> Result<Vec<String>, String> {
    for path in paths {
        if !crate::plan::is_worktree_contained_path(path) {
            return Err(format!("path escapes the worktree: {path}"));
        }
    }
    Ok(paths
        .iter()
        .filter(|path| !crate::diff::is_mcp_config(path.as_str()))
        .map(|path| format!(":(literal){path}"))
        .collect())
}

/// Run a git subcommand in `repo_path` as plain argv (no shell), returning
/// stdout. A non-zero exit joins stderr and stdout into the error — git
/// splits its story across both streams.
pub(super) fn run_git(repo_path: &Path, args: &[&str]) -> Result<String, String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .args(args)
        .output()
        .map_err(|e| format!("could not run git: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(format!(
            "git {} failed: {}",
            args.first().unwrap_or(&""),
            format!("{} {}", stderr.trim(), stdout.trim()).trim()
        ));
    }
    Ok(stdout)
}

/// `git.stage`: `git add -- <paths…>`. An empty surviving list (everything
/// filtered) succeeds as a no-op.
pub fn stage_paths(repo_path: &Path, paths: &[String]) -> Result<(), String> {
    let surviving = stageable_paths(paths)?;
    if surviving.is_empty() {
        return Ok(());
    }
    let mut args = vec!["add", "--"];
    args.extend(surviving.iter().map(String::as_str));
    run_git(repo_path, &args).map(|_| ())
}

/// `git.unstage`: `git reset -q HEAD -- <paths…>`, or — when HEAD is unborn
/// and there is nothing to reset to — drop the index entries with
/// `git rm -f -r -q --cached -- <paths…>`. The `-f` is required whenever the
/// staged copy differs from the worktree copy (on an unborn HEAD "differs
/// from HEAD" is always true), and is safe: `--cached` never touches the
/// worktree file.
pub fn unstage_paths(repo_path: &Path, paths: &[String]) -> Result<(), String> {
    let surviving = stageable_paths(paths)?;
    if surviving.is_empty() {
        return Ok(());
    }
    let repo = open_repo(repo_path)?;
    let mut args = if head_commit_id(&repo)?.is_some() {
        vec!["reset", "-q", "HEAD", "--"]
    } else {
        vec!["rm", "-f", "-r", "-q", "--cached", "--"]
    };
    args.extend(surviving.iter().map(String::as_str));
    run_git(repo_path, &args).map(|_| ())
}

/// `git.commit`: commit exactly what is staged (no `-a`, no auto-stage) with
/// the trimmed message as a single argv element. Returns the new commit's
/// summary (`hash`/`short`/`subject`…).
pub fn commit_staged(repo_path: &Path, message: &str) -> Result<Value, String> {
    let message = message.trim();
    if message.is_empty() {
        return Err("commit message must not be empty".to_string());
    }
    let staged = run_git(repo_path, &["diff", "--cached", "--name-only"])?;
    if staged.trim().is_empty() {
        return Err("nothing staged to commit".to_string());
    }
    run_git(repo_path, &["commit", "-m", message])?;
    let repo = open_repo(repo_path)?;
    let head = repo
        .head()
        .map_err(|e| e.to_string())?
        .peel_to_commit()
        .map_err(|e| e.to_string())?;
    Ok(commit_summary_json(&head))
}
pub fn stash_push(repo_path: &Path) -> Result<(), String> {
    run_git(repo_path, &["stash", "push", "-u"]).map(|_| ())
}

/// `git.stash_pop`: `git stash pop`. A pop conflict is git's error; it leaves
/// unmerged (UU) index entries but no MERGE_HEAD, so `repo.state()` stays Clean
/// and the next status surfaces repo_state "conflicted" (not "merging").
pub fn stash_pop(repo_path: &Path) -> Result<(), String> {
    run_git(repo_path, &["stash", "pop"]).map(|_| ())
}

/// `git.merge_abort`: abort whatever operation is in progress with the matching
/// git command — `git merge --abort` while merging, `git rebase --abort` while
/// rebasing, `git cherry-pick --abort` / `git revert --abort` for those, and
/// `git bisect reset` while bisecting. A conflicted-but-idle tree (a stash-pop
/// conflict) or a clean/"other" repo has no operation to abort.
pub fn merge_abort(repo_path: &Path) -> Result<(), String> {
    let repo = open_repo(repo_path)?;
    let state = repo_state_label(&repo)?;
    drop(repo);
    match state {
        "merging" => run_git(repo_path, &["merge", "--abort"]).map(|_| ()),
        "rebasing" => run_git(repo_path, &["rebase", "--abort"]).map(|_| ()),
        "cherry-picking" => run_git(repo_path, &["cherry-pick", "--abort"]).map(|_| ()),
        "reverting" => run_git(repo_path, &["revert", "--abort"]).map(|_| ()),
        "bisecting" => run_git(repo_path, &["bisect", "reset"]).map(|_| ()),
        _ => Err("no abortable operation in progress".to_string()),
    }
}

/// `git.discard` (**destructive**): revert each path to HEAD. Tracked paths go
/// through `git restore --staged --worktree --source=HEAD` (reverting both the
/// index and the working copy); untracked paths are unlinked directly — but
/// only after the two-layer [`crate::fs_scope::fenced_scope_path`] guard
/// (lexical + canonical containment), since a symlinked path whose components
/// all look Normal could otherwise resolve outside the worktree. The scaffolded
/// `.build/mcp.json` is silently skipped.
pub fn discard_paths(repo_path: &Path, paths: &[String]) -> Result<(), String> {
    for path in paths {
        if !crate::plan::is_worktree_contained_path(path) {
            return Err(format!("path escapes the worktree: {path}"));
        }
    }
    let repo = open_repo(repo_path)?;
    let mut tracked: Vec<String> = Vec::new();
    let mut untracked: Vec<&String> = Vec::new();
    for path in paths {
        if crate::diff::is_mcp_config(path.as_str()) {
            continue;
        }
        match repo.status_file(Path::new(path)) {
            Ok(status) if is_untracked_status(status) => untracked.push(path),
            _ => tracked.push(format!(":(literal){path}")),
        }
    }
    drop(repo);
    // Untracked deletions first: each is fenced (lexical + canonical) before
    // the unlink, so a traversal or symlink escape can never reach outside.
    for path in untracked {
        let target = crate::fs_scope::fenced_scope_path(repo_path, path)?;
        std::fs::remove_file(&target).map_err(|e| format!("cannot delete {path}: {e}"))?;
    }
    if !tracked.is_empty() {
        let mut args = vec!["restore", "--staged", "--worktree", "--source=HEAD", "--"];
        args.extend(tracked.iter().map(String::as_str));
        run_git(repo_path, &args)?;
    }
    Ok(())
}

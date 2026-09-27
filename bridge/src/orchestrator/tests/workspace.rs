use super::runs::{dispatch_single_stage_run, orchestrator, split_store};
use crate::git_fixture::init_repo;
use crate::isolation::Isolation;
use crate::models::ModelChoice;
use crate::orchestrator::{mcp_config_path, Agent, Orchestrator};
use crate::pty::HarnessSpec;
use crate::run::RunState;
use crate::templates::Templates;
use crate::worktree::{ExternalWorktree, WorktreeManager};
use std::path::Path;
use std::process::Command;

/// A checkout whose directory a human already removed still says whose
/// branch it is: the answer lives beside the registration in the main
/// repository, not behind the pointer in the missing directory. Reading it
/// through the pointer left the registration and the branch behind.
#[test]
fn discarding_a_checkout_whose_directory_is_gone_still_takes_its_branch() {
    let (dir, repo) = init_repo();
    let orch = orchestrator(&dir, &repo);
    let worktree = orch
        .create_bare_worktree("vanished", "main", Isolation::Worktree)
        .unwrap()
        .worktree;
    std::fs::remove_dir_all(&worktree.path).unwrap();

    orch.discard_checkout(&worktree, false);

    let r = git2::Repository::open(&repo).unwrap();
    assert!(
        r.find_worktree("vanished")
            .err()
            .map(|error| error.code() == git2::ErrorCode::NotFound)
            .unwrap_or(false),
        "the stale registration is pruned"
    );
    assert!(
        r.find_branch("build/vanished", git2::BranchType::Local)
            .is_err(),
        "the branch Build cut goes with it"
    );
}
#[test]
fn agent_launch_prepares_scaffolding_spec_and_pty_size_as_one_value() {
    let (dir, repo) = init_repo();
    let worktree = dir.path().join("prepared-worktree");
    std::fs::create_dir(&worktree).unwrap();
    let agent = Agent::WarmBuilder(std::sync::Arc::new(|_, _, options| {
        assert!(
            options
                .cwd
                .join(mcp_config_path(&options.owner_id))
                .exists(),
            "the scaffold must exist before the fallible harness builder runs"
        );
        Ok(HarnessSpec::new("prepared-harness"))
    }));
    let orchestrator = Orchestrator::new(
        repo.clone(),
        dir.path().join("worktrees"),
        agent,
        Templates::default(),
        std::fs::canonicalize(repo.join("README.md")).unwrap(),
    );

    let prepared = orchestrator
        .agent_launch()
        .prepare(
            "agent-prepared",
            crate::orchestrator::LaunchDirs::at(&worktree),
            &ModelChoice::default(),
            false,
            None,
            "token",
        )
        .unwrap();

    assert_eq!(prepared.spec.binary, "prepared-harness");
    assert_eq!(prepared.pty_size.rows, 40);
    assert_eq!(prepared.pty_size.cols, 120);
}
/// Create a user worktree at `dir/<name>` on a new `branch` (cut from the
/// primary HEAD) and return its discovered summary — the same shape the
/// app layer resolves a `worktree_id` to.
pub(super) fn user_worktree(
    dir: &tempfile::TempDir,
    repo: &Path,
    name: &str,
    branch: &str,
) -> ExternalWorktree {
    let path = dir.path().join(name);
    assert!(Command::new("git")
        .args(["worktree", "add", "-b", branch, path.to_str().unwrap()])
        .current_dir(repo)
        .status()
        .unwrap()
        .success());
    WorktreeManager::new(repo, dir.path().join("worktrees"))
        .discover("main", &std::collections::HashSet::new())
        .unwrap()
        .into_iter()
        .find(|w| w.branch.as_deref() == Some(branch))
        .expect("the new worktree is discoverable")
}
pub(super) fn worktree_head(worktree: &Path) -> String {
    let out = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(worktree)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}
/// Every checkout git knows about for this repo, primary first.
pub(super) fn registered_checkouts(repo: &Path) -> Vec<String> {
    let out = Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(repo)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| line.strip_prefix("worktree ").map(str::to_string))
        .collect()
}
#[tokio::test]
async fn abandon_run_removes_the_worktree_but_keeps_the_branch() {
    let (dir, repo) = init_repo();
    let orch = orchestrator(&dir, &repo);
    let store = split_store(&dir);
    let mut run = dispatch_single_stage_run(&orch, &store, "run-1", "single stage work");
    let branch = run.worktree.branch();
    let path = run.worktree.path.clone();

    orch.abandon_run_keeping_checkout(&mut run).unwrap();
    orch.discard_checkout(&run.worktree, /* keep_branch */ true);
    assert_eq!(run.run.state, RunState::Abandoned);
    assert!(!path.exists(), "the worktree is removed");
    // The branch survives — a run's work outlives an abandon so it can be
    // re-attempted (the run entity's documented contract).
    let branches = Command::new("git")
        .args(["branch", "--list", &branch])
        .current_dir(&repo)
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&branches.stdout).contains(&branch),
        "the branch is kept on abandon"
    );
}

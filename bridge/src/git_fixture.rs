//! The repository every test that needs one starts from: one commit on
//! `main`, inside a temporary directory that lives as long as the test holds
//! it. One fixture, so a test about worktrees, isolation, orchestration, diffs
//! or the app begins from the same repository and none of them describes it
//! again.

use std::path::{Path, PathBuf};
use std::process::Command;

/// What the one committed file says unless a test needs it to say otherwise.
const README: &str = "# project\n";

pub mod environment;

/// A repository called `repo` on `main` with one commit, as the directory that
/// owns it and the repository's own path inside it.
pub fn init_repo() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let repo = init_repo_named(dir.path(), "repo");
    (dir, repo)
}

/// The same repository under `parent`, named. A project is named after its
/// repository directory, so a test about telling two projects apart says what
/// each one is called.
pub fn init_repo_named(parent: &Path, name: &str) -> PathBuf {
    init_repo_with_readme(parent, name, README)
}

/// The same repository with `README.md` saying `readme`, for a test whose
/// diffs are read against what that file held at the first commit.
pub fn init_repo_with_readme(parent: &Path, name: &str, readme: &str) -> PathBuf {
    let repo = parent.join(name);
    std::fs::create_dir(&repo).unwrap();
    git_in(&repo, &["init", "-b", "main"]);
    git_in(&repo, &["config", "user.email", "test@build.ing"]);
    git_in(&repo, &["config", "user.name", "Test"]);
    std::fs::write(repo.join("README.md"), readme).unwrap();
    git_in(&repo, &["add", "."]);
    git_in(&repo, &["commit", "-m", "initial"]);
    repo
}

/// Run one git command in `dir`, failing the test the moment git does.
pub fn git_in(dir: &Path, args: &[&str]) {
    let status = git_command(dir, args).status().unwrap();
    assert!(status.success(), "git {args:?} failed");
}

/// One git command in `dir` as the fixture runs it, for a test that needs
/// its output or expects it to fail.
///
/// Every command commits as the fixture's own identity, so a clone — which
/// does not copy the source's `user.*` config — still commits on a machine
/// with no global identity, a CI runner's. And none reads the machine's own
/// global or system config: a `commit.gpgsign` there would sign fixtures with
/// a real key, or stop at its passphrase.
pub fn git_command(dir: &Path, args: &[&str]) -> Command {
    let mut command = Command::new("git");
    command
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@build.ing")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@build.ing")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null");
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fixture's setup commands already ignore the host config. A commit
    /// made later by the product must be protected by the fixture too.
    #[test]
    fn product_commits_in_a_fixture_do_not_use_the_machines_signer() {
        let environment = environment::GitEnvironment::with_signing();
        if !environment.run_test() {
            assert!(!environment.signer_log().exists());
            return;
        }
        let (_dir, repo) = init_repo();
        std::fs::write(repo.join("change.txt"), "change\n").unwrap();
        crate::gitgui::stage_paths(&repo, &["change.txt".into()]).unwrap();
        crate::gitgui::commit_staged(&repo, "product commit").unwrap();
    }

    /// Exercise the unmodified product against an explicitly supplied user
    /// config. The fake signer fails instead of accessing any real key.
    #[test]
    fn product_commits_honor_explicit_signing_config() {
        let environment = environment::GitEnvironment::with_signing();
        if !environment.run_test() {
            assert_eq!(
                std::fs::read_to_string(environment.signer_log()).unwrap(),
                "sign\n"
            );
            return;
        }
        let (_dir, repo) = init_repo();
        // Discard the fixture's local signing policy, if present: this test
        // stands for a user's repository, not an unsigned fixture.
        let _ = git_command(
            &repo,
            &["config", "--local", "--unset-all", "commit.gpgsign"],
        )
        .status()
        .unwrap();
        std::fs::write(repo.join("change.txt"), "change\n").unwrap();
        crate::gitgui::stage_paths(&repo, &["change.txt".into()]).unwrap();
        let error = crate::gitgui::commit_staged(&repo, "signed product commit").unwrap_err();
        assert!(error.contains("failed to write commit object"), "{error}");
    }

    /// A machine that signs every commit with a key only its owner can use
    /// must not reach a fixture's commits: the fixture neither signs with the
    /// owner's real key nor prompts for its passphrase. `$HOME` stands in for
    /// the machine's own config, set on the one command so no other test sees it.
    #[test]
    fn a_fixture_commit_reads_none_of_the_machines_git_config() {
        let (dir, repo) = init_repo();
        let home = dir.path().join("home");
        std::fs::create_dir(&home).unwrap();
        std::fs::write(
            home.join(".gitconfig"),
            "[commit]\n\tgpgsign = true\n[user]\n\tsigningkey = unusable\n[gpg]\n\tprogram = false\n",
        )
        .unwrap();
        let output = git_command(&repo, &["commit", "--allow-empty", "-m", "unsigned"])
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", &home)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

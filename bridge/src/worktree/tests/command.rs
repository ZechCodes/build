use crate::git_fixture::git_in;
use std::path::Path;
use std::process::Command;

pub(super) fn git_output(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?} failed");
    String::from_utf8_lossy(&out.stdout).into_owned()
}
/// Commit `name` in `dir` as a new file of the same name.
pub(super) fn commit_file(dir: &Path, name: &str) {
    std::fs::write(dir.join(format!("{name}.txt")), "x\n").unwrap();
    git_in(dir, &["add", "."]);
    git_in(dir, &["commit", "-m", name]);
}
/// The sha a ref points at, as git spells it.
pub(super) fn tip_of(repo: &Path, reference: &str) -> String {
    git2::Repository::open(repo)
        .unwrap()
        .find_reference(reference)
        .unwrap()
        .target()
        .unwrap()
        .to_string()
}

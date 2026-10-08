use crate::git_fixture::{git_in, init_repo};
use crate::reviews::git_actions::{merge_expected, GitActionError};
use crate::reviews::model::{ReviewDirectory, ReviewDirectoryStatus};
use std::path::Path;

fn feature(repository: &Path) -> ReviewDirectory {
    git_in(repository, &["checkout", "-b", "feature"]);
    std::fs::write(repository.join("feature.txt"), "feature\n").unwrap();
    git_in(repository, &["add", "feature.txt"]);
    git_in(repository, &["commit", "-m", "feature"]);
    let repo = git2::Repository::open(repository).unwrap();
    let directory = ReviewDirectory {
        id: "directory".into(),
        source_id: "source".into(),
        name: "repo".into(),
        path: repository.into(),
        source_path: repository.into(),
        is_git: true,
        status: ReviewDirectoryStatus::Git,
        reason: None,
        common_git_dir: Some(repo.commondir().canonicalize().unwrap()),
        branch: Some("feature".into()),
        base: None,
        head: Some(repo.head().unwrap().target().unwrap().to_string()),
        uncommitted_files: Some(0),
    };
    git_in(repository, &["checkout", "main"]);
    directory
}

fn target(repository: &Path) -> String {
    git2::Repository::open(repository)
        .unwrap()
        .refname_to_id("refs/heads/main")
        .unwrap()
        .to_string()
}

fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

fn native_commands_supported(repository: &Path) -> bool {
    let supported = super::supports_commands(repository).expect("identify native Git hook support");
    if !supported {
        eprintln!("native Git predates configured hook commands; skipping configured dispatch");
    }
    supported
}

#[test]
fn native_configured_reference_hook_preserves_nested_routing_args_and_stdin() {
    let (_home, repository) = init_repo();
    if !native_commands_supported(&repository) {
        return;
    }
    let (_foreign_home, foreign) = init_repo();
    let saved = feature(&repository);
    let before = target(&repository);
    let hook_path = "hooks with 'quotes'";
    git_in(&repository, &["config", "core.hooksPath", hook_path]);
    let expected = quote(repository.join(hook_path).to_str().unwrap());
    let foreign_path = quote(foreign.to_str().unwrap());
    let foreign_hooks = quote(foreign.join(".git/hooks").to_str().unwrap());
    let git_dir = quote(repository.join(".git").to_str().unwrap());
    let script = format!(
        "test \"$#\" -eq 1 || exit 90\n\
         test \"$(git rev-parse --path-format=absolute --git-path hooks)\" = {expected} || exit 91\n\
         test \"$(unset GIT_DIR GIT_WORK_TREE GIT_COMMON_DIR GIT_INDEX_FILE; \
         git -C {foreign_path} rev-parse --path-format=absolute --git-path hooks)\" = {foreign_hooks} || exit 92\n\
         printf '%s\\n' \"$1\" >> {git_dir}/configured-phases\n\
         cat >> {git_dir}/configured-input-\"$1\""
    );
    let command = format!("sh -c {} --", quote(&script));
    git_in(
        &repository,
        &["config", "hook.routing.event", "reference-transaction"],
    );
    git_in(
        &repository,
        &["config", "--add", "hook.routing.command", "exit 97"],
    );
    git_in(
        &repository,
        &["config", "--add", "hook.routing.command", &command],
    );

    let result = merge_expected(&saved, &repository, "refs/heads/main", &before)
        .expect("configured hook keeps original Git routing inside the fenced merge");
    assert_eq!(target(&repository), result.head);
    let phases = std::fs::read_to_string(repository.join(".git/configured-phases")).unwrap();
    assert!(phases.lines().any(|phase| phase == "prepared"));
    assert!(phases.lines().any(|phase| phase == "committed"));
    let prepared = std::fs::read(repository.join(".git/configured-input-prepared")).unwrap();
    let committed = std::fs::read(repository.join(".git/configured-input-committed")).unwrap();
    assert_eq!(
        prepared, committed,
        "native hook stdin survives unchanged across phases"
    );
    assert!(prepared.ends_with(b"\n"));
    let input = String::from_utf8(prepared).unwrap();
    assert!(
        input.contains(&format!("{before} {} refs/heads/main\n", result.head))
            || input.contains(&format!("{before} {} HEAD\n", result.head)),
        "native transaction contains the selected merge update: {input}"
    );
}

#[test]
fn plain_configured_command_does_not_become_a_shell_builtin() {
    let (home, _repository) = init_repo();
    let path = home.path().join("shim");
    let script = super::script("exit", "unset GIT_CONFIG_PARAMETERS\n", None).unwrap();
    super::super::write_hook(&path, &script).unwrap();
    let result = std::process::Command::new(path).output().unwrap();
    assert!(
        !result.status.success(),
        "plain exit must remain an executable name"
    );
}

#[test]
#[cfg(unix)]
fn non_utf8_configured_command_is_rejected_before_emitting_shims() {
    use std::os::unix::ffi::OsStringExt;
    let (home, repository) = init_repo();
    let mut command =
        crate::git_fixture::git_command(&repository, &["config", "hook.invalid.command"]);
    command.arg(std::ffi::OsString::from_vec(b"exit \xff".to_vec()));
    assert!(command.status().unwrap().success());
    let destination = home.path().join("shims");
    std::fs::create_dir(&destination).unwrap();

    let result =
        super::configured_overrides(&repository, &destination, "unset GIT_CONFIG_PARAMETERS\n");
    assert!(
        result.is_err(),
        "invalid bytes must not become a different executable policy"
    );
    assert_eq!(std::fs::read_dir(&destination).unwrap().count(), 0);
}

#[test]
fn native_configured_hook_preserves_enabled_flag_and_veto() {
    for enabled in [false, true] {
        let (_home, repository) = init_repo();
        if !native_commands_supported(&repository) {
            return;
        }
        let saved = feature(&repository);
        let before = target(&repository);
        git_in(
            &repository,
            &["config", "hook.policy.event", "pre-merge-commit"],
        );
        git_in(&repository, &["config", "hook.policy.command", "exit 37"]);
        git_in(
            &repository,
            &[
                "config",
                "hook.policy.enabled",
                if enabled { "true" } else { "false" },
            ],
        );

        let result = merge_expected(&saved, &repository, "refs/heads/main", &before);
        if enabled {
            assert!(
                matches!(result, Err(GitActionError::Failed(_))),
                "{result:?}"
            );
            assert_eq!(target(&repository), before);
            assert!(!repository.join("feature.txt").exists());
        } else {
            assert!(result.is_ok(), "{result:?}");
            assert_ne!(target(&repository), before);
        }
    }
}

#[test]
fn ambiguous_command_key_is_rejected_before_emitting_shims() {
    let (home, repository) = init_repo();
    git_in(&repository, &["config", "hook.foo=bar.command", "exit 37"]);
    let destination = home.path().join("shims");
    std::fs::create_dir(&destination).unwrap();

    let result =
        super::configured_overrides(&repository, &destination, "unset GIT_CONFIG_PARAMETERS\n");
    assert!(
        result.is_err(),
        "a key that -c cannot encode must not leave the original hook unwrapped"
    );
    assert_eq!(std::fs::read_dir(&destination).unwrap().count(), 0);
}

#[test]
fn configured_hook_support_uses_the_known_native_git_boundary() {
    for version in ["git version 2.39.5\n", "git version 2.53.4\n"] {
        assert_eq!(super::version_supports_commands(version), Ok(false));
    }
    for version in [
        "git version 2.54.0\n",
        "git version 2.55.0.apple.1\n",
        "git version 3.0.0\n",
    ] {
        assert_eq!(super::version_supports_commands(version), Ok(true));
    }
    for version in ["unknown", "git version two\n", "git version 2.invalid\n"] {
        assert!(super::version_supports_commands(version).is_err());
    }
}

//! A separate process for tests whose product path creates the repository
//! and commits before a fixture can configure it. Never mutate the parallel
//! test harness's environment or change git's production command builders.

use std::path::{Path, PathBuf};
use std::process::Command;

const CHILD_TEST: &str = "BUILD_GIT_FIXTURE_TEST";
const CHILD_MARKER: &str = "BUILD_GIT_FIXTURE_MARKER";

/// An unsigned isolated process for a test that creates its own repository.
macro_rules! isolated_git_test {
    () => {
        if $crate::git_fixture::environment::run_test(
            $crate::git_fixture::environment::GitEnvironment::unsigned,
        )
        .is_some()
        {
            return;
        }
    };
}
pub(crate) use isolated_git_test;

pub struct GitEnvironment {
    home: tempfile::TempDir,
}

impl GitEnvironment {
    pub fn unsigned() -> Self {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(
            home.path().join(".gitconfig"),
            "[user]\nname = Test\nemail = test@build.ing\n[commit]\ngpgsign = false\n",
        )
        .unwrap();
        Self { home }
    }

    pub fn with_signing() -> Self {
        use std::os::unix::fs::PermissionsExt;

        let environment = Self::unsigned();
        let signer = environment.home.path().join("fake-gpg");
        std::fs::write(
            &signer,
            "#!/bin/sh\nprintf 'sign\\n' >> \"$HOME/signer.log\"\nexit 1\n",
        )
        .unwrap();
        std::fs::set_permissions(&signer, std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut config = git2::Config::open(&environment.home.path().join(".gitconfig")).unwrap();
        config.set_bool("commit.gpgsign", true).unwrap();
        config
            .set_str("user.signingkey", "unused-test-key")
            .unwrap();
        config.set_str("gpg.format", "openpgp").unwrap();
        config
            .set_str("gpg.program", signer.to_str().unwrap())
            .unwrap();
        environment
    }

    pub fn signer_log(&self) -> PathBuf {
        self.home.path().join("signer.log")
    }

    fn run_named_test(&self, name: &str) {
        let output = test_command(self.home.path(), name).output().unwrap();
        assert!(
            output.status.success(),
            "isolated {name}: {}\n{}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            std::fs::read_to_string(self.home.path().join("entered-test"))
                .ok()
                .as_deref(),
            Some(name),
            "isolated {name} did not enter its test body"
        );
    }
}

/// Run this exact test in a child. The parent gets its environment back for
/// assertions on artifacts; the child returns None without constructing
/// another HOME. Call before starting any test work.
pub fn run_test(make_environment: fn() -> GitEnvironment) -> Option<GitEnvironment> {
    let thread = std::thread::current();
    let name = thread.name().expect("a named test thread");
    if std::env::var(CHILD_TEST).as_deref() == Ok(name) {
        record_entry(name);
        return None;
    }
    let environment = make_environment();
    environment.run_named_test(name);
    Some(environment)
}

fn record_entry(name: &str) {
    use std::io::Write;

    let marker = std::env::var_os(CHILD_MARKER).expect("the parent's execution marker");
    // create_new also rejects a second entry: one successful child exit must
    // represent exactly one execution of the requested test body.
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(marker)
        .unwrap()
        .write_all(name.as_bytes())
        .unwrap();
}

fn test_command(home: &Path, name: &str) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", name, "--nocapture"])
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home)
        .env("GNUPGHOME", home)
        .env("GIT_CONFIG_GLOBAL", home.join(".gitconfig"))
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("BRIDGE_IDENTITY_FILE", home.join("identity.json"))
        .env(CHILD_MARKER, home.join("entered-test"))
        .env(CHILD_TEST, name);
    for variable in ["RUST_BACKTRACE", "TMPDIR", "LLVM_PROFILE_FILE"] {
        if let Some(value) = std::env::var_os(variable) {
            command.env(variable, value);
        }
    }
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_child_with_no_matching_test_is_rejected() {
        let result = std::thread::Builder::new()
            .name("no such isolated git fixture test".into())
            .spawn(|| run_test(GitEnvironment::unsigned).is_some())
            .unwrap()
            .join();
        let panic = result.expect_err("a child running zero tests must not pass");
        let message = panic.downcast_ref::<String>().unwrap();
        assert!(message.contains("did not enter"), "{message}");
    }

    #[test]
    fn a_matching_child_reuses_the_prepared_environment() {
        fn only_in_parent() -> GitEnvironment {
            assert!(std::env::var_os(CHILD_TEST).is_none());
            GitEnvironment::unsigned()
        }

        if run_test(only_in_parent).is_some() {
            return;
        }
        let home = PathBuf::from(std::env::var_os("HOME").unwrap());
        assert!(home.join(".gitconfig").is_file());
    }
}

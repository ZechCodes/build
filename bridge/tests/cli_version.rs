//! `build-bridge --version` is the release pipeline's checkpoint.
//!
//! The workflow that builds a `bridge-vX.Y.Z` tag runs the freshly built binary
//! and compares its output to the tag, so the line it prints is a contract:
//! exactly `build-bridge <version from Cargo.toml>`, nothing else on stdout.

use std::process::Command;

fn stdout_of(flag: &str) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_build-bridge"))
        .arg(flag)
        .output()
        .expect("the binary runs");
    assert!(output.status.success(), "`build-bridge {flag}` exits 0");
    String::from_utf8(output.stdout).expect("stdout is utf-8")
}

#[test]
fn version_flag_prints_the_crate_version_and_nothing_else() {
    assert_eq!(
        stdout_of("--version"),
        format!("build-bridge {}\n", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn short_version_flag_prints_the_same_line() {
    assert_eq!(stdout_of("-V"), stdout_of("--version"));
}

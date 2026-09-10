use super::*;

#[test]
fn resolve_term_shell_yields_an_absolute_shell_path() {
    // Whatever the source (override, $SHELL, passwd, fallback), the result
    // must be an executable path, never empty.
    let shell = resolve_term_shell();
    assert!(shell.starts_with('/'), "{shell}");
}

#[test]
fn capture_login_path_returns_the_shells_path() {
    let path =
        capture_login_path("/bin/bash", Duration::from_secs(10)).expect("bash must yield a PATH");
    assert!(path.contains('/'), "{path}");
    assert!(path.contains("bin"), "{path}");
}

#[test]
fn capture_login_path_rejects_a_bare_launchd_path() {
    // A shell whose rc files never widen PATH leaves us with launchd's bare
    // default. Adopting it is worse than useless: it *looks* like the fix
    // worked while `claude` still can't be found. Reject it so the daemon
    // logs the truth and keeps whatever it inherited.
    let shell = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(
        shell.path(),
        "#!/bin/sh\nprintf %s /usr/bin:/bin:/usr/sbin:/sbin\n",
    )
    .unwrap();
    std::fs::set_permissions(
        shell.path(),
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )
    .unwrap();

    assert_eq!(
        capture_login_path(&shell.path().to_string_lossy(), Duration::from_secs(5)),
        None
    );
}

#[test]
fn capture_login_path_survives_a_missing_shell() {
    assert_eq!(
        capture_login_path("/nonexistent-shell-for-test", Duration::from_secs(5)),
        None
    );
}

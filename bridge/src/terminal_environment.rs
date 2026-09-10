//! Login-shell and environment discovery for user terminals and the daemon.

/// The shell user terminals run: `BRIDGE_TERM_SHELL` override → the daemon
/// env's `SHELL` → the account's passwd shell → bash. Terminals are windows
/// onto the user's machine — they get the user's own shell and rc files, not
/// a sanitized bash.
pub fn resolve_term_shell() -> String {
    for var in ["BRIDGE_TERM_SHELL", "SHELL"] {
        if let Ok(shell) = std::env::var(var) {
            if !shell.trim().is_empty() {
                return shell;
            }
        }
    }
    passwd_shell().unwrap_or_else(|| "/bin/bash".to_string())
}

/// The account's login shell from the passwd database.
fn passwd_shell() -> Option<String> {
    // SAFETY: getpwuid returns a pointer to static storage owned by libc; we
    // only read pw_shell out of it, on this thread, immediately.
    unsafe {
        let pw = libc::getpwuid(libc::getuid());
        if pw.is_null() {
            return None;
        }
        let shell = (*pw).pw_shell;
        if shell.is_null() {
            return None;
        }
        let shell = std::ffi::CStr::from_ptr(shell)
            .to_string_lossy()
            .into_owned();
        (!shell.trim().is_empty()).then_some(shell)
    }
}

/// Ask a login shell what PATH looks like — the terminal-emulator trick.
/// launchd starts agents with a bare PATH, so user-installed coding-agent
/// harnesses don't resolve until we adopt the login PATH.
/// Bounded by `timeout`; a hung rc file just means we keep the inherited PATH.
pub fn capture_login_path(shell: &str, timeout: std::time::Duration) -> Option<String> {
    let (tx, rx) = std::sync::mpsc::channel();
    let shell = shell.to_string();
    std::thread::spawn(move || {
        let out = std::process::Command::new(&shell)
            .args(["-ilc", "printf %s \"$PATH\""])
            .stdin(std::process::Stdio::null())
            .output();
        let _ = tx.send(out);
    });
    match rx.recv_timeout(timeout) {
        Ok(Ok(out)) if out.status.success() => {
            let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
            path_widens_launchd_default(&path).then_some(path)
        }
        _ => None,
    }
}

/// launchd's own PATH. A captured PATH that adds nothing to it is not worth
/// adopting — it would mask the real problem behind a "PATH adopted" log line.
const LAUNCHD_BARE_PATH: [&str; 4] = ["/usr/bin", "/bin", "/usr/sbin", "/sbin"];

fn path_widens_launchd_default(path: &str) -> bool {
    path.split(':')
        .any(|dir| !dir.is_empty() && !LAUNCHD_BARE_PATH.contains(&dir))
}

//! Asking an installed CLI what it is.
//!
//! Every probe runs the program by name, as a spawn would find it on `PATH`,
//! with no shell, a closed stdin where it takes none, and a deadline after
//! which its whole process group is killed. The CLIs sit behind wrapper scripts
//! (mise) that start children of their own, so a group kill is what leaves
//! nothing running.

use semver::Version;

use super::CliReading;

/// How long one probe may run. `claude --version` answers in about 0.13 s and
/// codex's model list in about 0.3 s (#203); a wrapper that hangs is cut off
/// here rather than holding an answer that nothing waits on anyway.
pub(crate) const PROBE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(3);

/// A way of asking one CLI what it is.
pub trait CliProbe: Send + Sync {
    /// Whatever `binary` says. Never an error: a CLI that cannot be asked, or
    /// answers in words Build cannot read, reads as knowing nothing.
    fn read(&self, binary: &str) -> CliReading;
}

/// A CLI nobody asks: its harness offers its catalog whole.
pub struct NoProbe;

pub static NO_PROBE: NoProbe = NoProbe;

impl CliProbe for NoProbe {
    fn read(&self, _binary: &str) -> CliReading {
        CliReading::default()
    }
}

/// `<binary> --version`, and the first word of the answer that is a version.
pub struct VersionFlag;

pub static VERSION_FLAG: VersionFlag = VersionFlag;

impl CliProbe for VersionFlag {
    fn read(&self, binary: &str) -> CliReading {
        let version = run_version_flag(binary)
            .map_err(|why| eprintln!("cli probe: {binary} --version: {why}"))
            .ok()
            .and_then(|said| version_in(&said));
        CliReading {
            version,
            listed: None,
        }
    }
}

fn run_version_flag(binary: &str) -> std::io::Result<String> {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    // The home directory, not the bridge's: a version manager reads the
    // directory it is started in for a project-local pin, and the answer
    // wanted is the machine's.
    let dir = home.unwrap_or_else(std::env::temp_dir);
    let output = crate::git_process::run_command_with_deadline(
        std::ffi::OsStr::new(binary),
        &dir,
        &[std::ffi::OsStr::new("--version")],
        PROBE_DEADLINE,
    )?;
    if !output.status.success() {
        return Err(std::io::Error::other(format!("exited {}", output.status)));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The first whitespace-separated word of `said` that is a version, or the
/// version after a `name/` in one: `2.1.280 (Claude Code)`, `codex-cli
/// 0.155.1`, `build_bridge/0.155.1 (Linux …)`.
pub fn version_in(said: &str) -> Option<Version> {
    said.split_whitespace().find_map(|word| {
        let word = word.rsplit('/').next().unwrap_or(word);
        Version::parse(word.trim_start_matches('v')).ok()
    })
}

mod codex_list;

pub use codex_list::CODEX_MODEL_LIST;

#[cfg(test)]
mod tests;

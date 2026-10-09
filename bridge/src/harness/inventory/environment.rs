//! The device environment the inventory observes: the home and variables a
//! harness spawns with, captured once per sweep, so every adapter reads the
//! same facts and a test can point all of them at a temporary home.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::harness::installed::executable::{self, Executable};
use crate::harness::installed::probe::child::{withheld_from_probes, OFFLINE_SWITCHES};

/// Where Claude Code reads policy settings on this platform, which may name
/// helper commands as well as the user's own settings can.
#[cfg(target_os = "macos")]
const CLAUDE_MANAGED_SETTINGS: &[&str] =
    &["/Library/Application Support/ClaudeCode/managed-settings.json"];
#[cfg(not(target_os = "macos"))]
const CLAUDE_MANAGED_SETTINGS: &[&str] = &["/etc/claude-code/managed-settings.json"];

#[derive(Debug, Clone)]
pub struct DeviceEnvironment {
    home: PathBuf,
    vars: BTreeMap<OsString, OsString>,
    claude_managed_settings: Vec<PathBuf>,
}

impl DeviceEnvironment {
    /// This daemon's own: what its harnesses inherit.
    pub fn of_this_process() -> Self {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        Self::new(home, std::env::vars_os())
    }

    /// `home`, with exactly `vars` set.
    pub fn new<K, V>(home: impl Into<PathBuf>, vars: impl IntoIterator<Item = (K, V)>) -> Self
    where
        K: Into<OsString>,
        V: Into<OsString>,
    {
        DeviceEnvironment {
            home: home.into(),
            vars: vars
                .into_iter()
                .map(|(name, value)| (name.into(), value.into()))
                .collect(),
            claude_managed_settings: CLAUDE_MANAGED_SETTINGS.iter().map(PathBuf::from).collect(),
        }
    }

    /// Read Claude Code's policy settings from `paths` instead.
    pub fn with_claude_managed_settings(mut self, paths: Vec<PathBuf>) -> Self {
        self.claude_managed_settings = paths;
        self
    }

    pub fn home(&self) -> &Path {
        &self.home
    }

    pub(super) fn claude_managed_settings(&self) -> &[PathBuf] {
        &self.claude_managed_settings
    }

    /// Whether `name` is set to something nonempty. Only presence is ever
    /// asked: a credential variable's value is not Build's to read.
    pub(super) fn has(&self, name: &str) -> bool {
        self.vars
            .get(OsStr::new(name))
            .is_some_and(|value| !value.is_empty())
    }

    /// The directory `variable` names, or `relative` under the home.
    pub(super) fn dir_or_home(&self, variable: &str, relative: &str) -> PathBuf {
        match self.vars.get(OsStr::new(variable)) {
            Some(dir) if !dir.is_empty() => self.home.join(dir),
            _ => self.home.join(relative),
        }
    }

    /// The executable a spawn of `binary` would run here.
    pub(super) fn executable(&self, binary: &str) -> Option<Executable> {
        let path = self
            .vars
            .get(OsStr::new("PATH"))
            .cloned()
            .unwrap_or_else(|| "/usr/bin:/bin".into());
        executable::identify_on_path(binary, &path, &self.home)
    }

    /// `program args…` with fixed argv, no shell, from the home directory,
    /// with this environment less everything withheld from probes, offline,
    /// stdin closed, in a process group of its own.
    pub(super) fn probe_command(&self, program: &Path, args: &[&str]) -> Command {
        let mut command = Command::new(program);
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        command
            .env_clear()
            .envs(
                self.vars
                    .iter()
                    .filter(|(name, _)| !withheld_from_probes(name)),
            )
            .env("HOME", &self.home)
            .envs(OFFLINE_SWITCHES)
            .args(args)
            .current_dir(&self.home)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }
}

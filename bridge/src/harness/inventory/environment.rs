//! The device environment the inventory observes: the home and variables a
//! harness spawns with, captured once per sweep, so every adapter reads the
//! same facts and a test can point all of them at a temporary home. Nothing
//! is run in it: the inventory starts no CLI (#466).

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use crate::harness::installed::executable::{self, Executable};

/// Where Claude Code reads managed policy on this platform:
/// `managed-settings.json` and the `managed-settings.d/` drop-ins beside it.
#[cfg(target_os = "macos")]
const CLAUDE_MANAGED_ROOT: &str = "/Library/Application Support/ClaudeCode";
#[cfg(not(target_os = "macos"))]
const CLAUDE_MANAGED_ROOT: &str = "/etc/claude-code";

#[derive(Debug, Clone)]
pub struct DeviceEnvironment {
    home: PathBuf,
    vars: BTreeMap<OsString, OsString>,
    claude_managed_root: PathBuf,
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
            claude_managed_root: PathBuf::from(CLAUDE_MANAGED_ROOT),
        }
    }

    /// Read Claude Code's managed policy from `root` instead.
    pub fn with_claude_managed_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.claude_managed_root = root.into();
        self
    }

    pub fn home(&self) -> &Path {
        &self.home
    }

    pub(super) fn claude_managed_root(&self) -> &Path {
        &self.claude_managed_root
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
}

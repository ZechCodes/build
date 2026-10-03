//! The executable a probe would find, without running it. Relative PATH
//! entries are resolved from HOME, just like the probe's child process.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Executable {
    path: PathBuf,
    resolved: PathBuf,
    modified: Option<SystemTime>,
}

pub(super) fn identify(binary: &str) -> Option<Executable> {
    let path = std::env::var_os("PATH").unwrap_or_else(|| "/usr/bin:/bin".into());
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    identify_on_path(binary, &path, &home)
}

fn identify_on_path(binary: &str, path: &std::ffi::OsStr, home: &Path) -> Option<Executable> {
    if binary.contains('/') {
        return at(home.join(binary));
    }
    std::env::split_paths(path).find_map(|directory| at(home.join(directory).join(binary)))
}

fn at(path: PathBuf) -> Option<Executable> {
    let metadata = std::fs::metadata(&path).ok()?;
    if !metadata.is_file() {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return None;
        }
    }
    Some(Executable {
        resolved: std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone()),
        path,
        modified: metadata.modified().ok(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_lookup_follows_the_probe_home_and_skips_nonexecutables() {
        let home = tempfile::tempdir().unwrap();
        let bin = home.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        std::fs::write(home.path().join("codex"), "not executable").unwrap();
        let binary = bin.join("codex");
        crate::isolation::test_fixture::write_executable(&binary, "#!/bin/sh\nexit 0\n");

        let found = identify_on_path("codex", std::ffi::OsStr::new(":bin"), home.path())
            .expect("the executable in the relative bin directory");
        assert_eq!(found.path, binary);
        assert!(identify_on_path("missing", std::ffi::OsStr::new("bin"), home.path()).is_none());
    }
}

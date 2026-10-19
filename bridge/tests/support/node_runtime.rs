use std::ffi::{OsStr, OsString};
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

const NODE_HINT: &str =
    "Set BUILD_TEST_NODE to an absolute executable Node path outside mise/asdf shims";

fn is_shim(path: &Path) -> bool {
    path.ancestors().any(|directory| {
        directory.file_name() == Some(OsStr::new("shims"))
            && directory
                .parent()
                .and_then(Path::file_name)
                .is_some_and(|name| {
                    ["mise", ".mise", "asdf", ".asdf"]
                        .iter()
                        .any(|manager| name == OsStr::new(manager))
                })
    })
}

fn executable_node(candidate: &Path) -> Option<PathBuf> {
    if !candidate.is_absolute() || is_shim(candidate) {
        return None;
    }
    let canonical = candidate.canonicalize().ok()?;
    let metadata = canonical.metadata().ok()?;
    (metadata.is_file() && metadata.permissions().mode() & 0o111 != 0 && !is_shim(&canonical))
        .then_some(canonical)
}

fn resolve_node(path: &OsStr, override_path: Option<&OsStr>) -> Result<PathBuf, String> {
    if let Some(candidate) = override_path {
        return executable_node(Path::new(candidate))
            .ok_or_else(|| format!("Invalid BUILD_TEST_NODE. {NODE_HINT}"));
    }
    std::env::split_paths(path)
        .find_map(|directory| executable_node(&directory.join("node")))
        .ok_or_else(|| NODE_HINT.into())
}

fn command_with_node(home: &Path, node: &Path, path: &OsStr) -> Command {
    // The owned alias also handles overrides named node22: MCP's env-node
    // shebang must use the very same executable as the extension driver.
    let bin = home.join("test-node-bin");
    std::fs::create_dir_all(&bin).unwrap();
    let alias = bin.join("node");
    if !alias.exists() {
        symlink(node, &alias).unwrap();
    }
    let child_path =
        std::env::join_paths(std::iter::once(bin).chain(std::env::split_paths(path))).unwrap();
    let mut command = Command::new(node);
    command
        .env_clear()
        .env("HOME", home)
        .env("PATH", child_path);
    command
}

pub fn isolated_node(home: &Path) -> Command {
    static RUNTIME: OnceLock<(PathBuf, OsString)> = OnceLock::new();
    let (node, path) = RUNTIME.get_or_init(|| {
        let path = std::env::var_os("PATH").unwrap_or_default();
        let requested = std::env::var_os("BUILD_TEST_NODE");
        let node =
            resolve_node(&path, requested.as_deref()).unwrap_or_else(|error| panic!("{error}"));
        (node, path)
    });
    command_with_node(home, node, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{symlink, PermissionsExt};
    use std::path::Path;

    fn executable(root: &Path, name: &str) -> PathBuf {
        let path = root.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "#!/bin/sh\nprintf 'native-node\\n'\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        path
    }

    #[test]
    fn skips_mise_asdf_and_symlinked_shims_without_executing_them() {
        let temp = tempfile::tempdir().unwrap();
        let mise = executable(temp.path(), "mise/shims/node");
        let asdf = executable(temp.path(), ".asdf/shims/node");
        let marker = temp.path().join("shim-ran");
        for shim in [&mise, &asdf] {
            std::fs::write(
                shim,
                format!("#!/bin/sh\ntouch '{}'\nexit 78\n", marker.display()),
            )
            .unwrap();
        }
        let alias = temp.path().join("alias");
        std::fs::create_dir(&alias).unwrap();
        symlink(&mise, alias.join("node")).unwrap();
        let native = executable(temp.path(), "native/bin/node");
        let path = std::env::join_paths([
            mise.parent().unwrap(),
            asdf.parent().unwrap(),
            &alias,
            native.parent().unwrap(),
        ])
        .unwrap();
        assert_eq!(resolve_node(&path, None).unwrap(), native);
        assert!(!marker.exists());
    }

    #[test]
    fn absolute_override_wins_and_invalid_overrides_do_not_fall_back() {
        let temp = tempfile::tempdir().unwrap();
        let native = executable(temp.path(), "native/node");
        let renamed = executable(temp.path(), "other/node22");
        let path = native.parent().unwrap().as_os_str();
        assert_eq!(
            resolve_node(path, Some(renamed.as_os_str())).unwrap(),
            renamed
        );
        let nonexec = temp.path().join("not-executable");
        std::fs::write(&nonexec, "data").unwrap();
        let missing = temp.path().join("missing");
        let shim = executable(temp.path(), "asdf/shims/node");
        for invalid in [Path::new("node"), &missing, temp.path(), &nonexec, &shim] {
            let error = resolve_node(path, Some(invalid.as_os_str())).unwrap_err();
            assert!(error.contains("BUILD_TEST_NODE"), "{error}");
        }
    }

    #[test]
    fn native_symlink_is_accepted_and_missing_node_is_actionable() {
        let temp = tempfile::tempdir().unwrap();
        let native = executable(temp.path(), "native/node22");
        let alias = temp.path().join("alias");
        std::fs::create_dir(&alias).unwrap();
        symlink(&native, alias.join("node")).unwrap();
        assert_eq!(resolve_node(alias.as_os_str(), None).unwrap(), native);
        assert!(resolve_node(temp.path().join("missing").as_os_str(), None)
            .unwrap_err()
            .contains("BUILD_TEST_NODE"));
    }

    #[test]
    fn driver_and_mcp_env_node_use_the_same_renamed_override() {
        let temp = tempfile::tempdir().unwrap();
        let node = executable(temp.path(), "native/node22");
        let shim = executable(temp.path(), "mise/shims/node");
        let marker = temp.path().join("shim-ran");
        std::fs::write(
            &shim,
            format!("#!/bin/sh\ntouch '{}'\nexit 78\n", marker.display()),
        )
        .unwrap();
        let path = std::env::join_paths([
            shim.parent().unwrap(),
            Path::new("/usr/bin"),
            Path::new("/bin"),
        ])
        .unwrap();
        let mut driver = command_with_node(temp.path(), &node, &path);
        let output = driver.output().unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"native-node\n");
        let mcp = executable(temp.path(), "mcp");
        std::fs::write(&mcp, "#!/usr/bin/env node\n").unwrap();
        let mut child = Command::new(mcp);
        child.env_clear();
        for (key, value) in driver.get_envs() {
            child.env(key, value.unwrap());
        }
        let output = child.output().unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"native-node\n");
        assert!(!marker.exists());
    }
}

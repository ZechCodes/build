use std::ffi::{OsStr, OsString};
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

const NODE_HINT: &str =
    "Set BUILD_TEST_NODE to an absolute executable Node path outside shims directories";

fn is_shim(path: &Path) -> bool {
    path.ancestors()
        .any(|directory| directory.file_name() == Some(OsStr::new("shims")))
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

fn resolve_node_with_probe(
    path: &OsStr,
    override_path: Option<&OsStr>,
    probe: impl FnOnce() -> std::io::Result<std::process::Output>,
) -> Result<PathBuf, String> {
    let resolved = resolve_node(path, override_path);
    if override_path.is_some() || resolved.is_ok() {
        return resolved;
    }
    let failure = || format!("Node process.execPath discovery failed. {NODE_HINT}");
    let output = probe().map_err(|_| failure())?;
    if !output.status.success() {
        return Err(failure());
    }
    let stdout = std::str::from_utf8(&output.stdout).map_err(|_| failure())?;
    executable_node(Path::new(stdout.trim()))
        .ok_or_else(|| format!("Node process.execPath returned an invalid executable. {NODE_HINT}"))
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
        // A shim-only PATH needs its parent environment to locate Node. Probe
        // once here, before constructing any env-cleared fixture children.
        let node = resolve_node_with_probe(&path, requested.as_deref(), || {
            Command::new("node")
                .args(["-p", "process.execPath"])
                .output()
        })
        .unwrap_or_else(|error| panic!("{error}"));
        (node, path)
    });
    command_with_node(home, node, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{symlink, PermissionsExt};
    use std::path::Path;

    #[test]
    fn shim_only_path_resolves_once_with_parent_environment_before_isolation() {
        let temp = tempfile::tempdir().unwrap();
        let native = executable(temp.path(), "native/node22");
        let shim = executable(temp.path(), "mise/shims/node");
        std::fs::write(&shim, "#!/bin/sh\n[ \"$1\" = -p ] && [ \"$2\" = process.execPath ] || exit 9\nprintf '%s\\n' \"$TEST_PARENT_NODE\"\n").unwrap();
        let path = shim.parent().unwrap().as_os_str();
        let calls = std::cell::Cell::new(0);
        let resolved = resolve_node_with_probe(path, None, || {
            calls.set(calls.get() + 1);
            Command::new("node")
                .args(["-p", "process.execPath"])
                .env_clear()
                .env("HOME", temp.path())
                .env("PATH", path)
                .env("TEST_PARENT_NODE", &native)
                .output()
        })
        .unwrap();
        assert_eq!(resolved, native);
        assert_eq!(calls.get(), 1);
        let child = command_with_node(temp.path(), &resolved, path);
        assert!(!child.get_envs().any(|(name, _)| name == "TEST_PARENT_NODE"));
    }

    #[test]
    fn fallback_rejects_failure_invalid_output_and_all_shim_targets() {
        use std::os::unix::process::ExitStatusExt;
        let temp = tempfile::tempdir().unwrap();
        let shim = executable(temp.path(), "custom/shims/node");
        let alias = temp.path().join("alias-node");
        symlink(&shim, &alias).unwrap();
        let path = temp.path().join("missing");
        for stdout in [
            b"relative/node\n".to_vec(),
            Vec::new(),
            vec![255],
            shim.as_os_str().as_encoded_bytes().to_vec(),
            alias.as_os_str().as_encoded_bytes().to_vec(),
        ] {
            let error = resolve_node_with_probe(path.as_os_str(), None, || {
                Ok(std::process::Output {
                    status: std::process::ExitStatus::from_raw(0),
                    stdout,
                    stderr: Vec::new(),
                })
            })
            .unwrap_err();
            assert!(
                error.contains("process.execPath") && error.contains("BUILD_TEST_NODE"),
                "{error}"
            );
        }
        let error = resolve_node_with_probe(path.as_os_str(), None, || {
            Ok(std::process::Output {
                status: std::process::ExitStatus::from_raw(7 << 8),
                stdout: Vec::new(),
                stderr: b"private diagnostic".to_vec(),
            })
        })
        .unwrap_err();
        assert!(
            error.contains("failed") && !error.contains("private diagnostic"),
            "{error}"
        );
        assert!(
            resolve_node_with_probe(path.as_os_str(), None, || Err(std::io::Error::other(
                "spawn failed"
            )))
            .unwrap_err()
            .contains("process.execPath")
        );
    }

    #[test]
    fn overrides_and_native_path_never_invoke_fallback() {
        let temp = tempfile::tempdir().unwrap();
        let native = executable(temp.path(), "native/node");
        let path = native.parent().unwrap().as_os_str();
        let never = || panic!("fallback must not run");
        assert_eq!(resolve_node_with_probe(path, None, never).unwrap(), native);
        assert_eq!(
            resolve_node_with_probe(OsStr::new(""), Some(native.as_os_str()), never).unwrap(),
            native
        );
        assert!(resolve_node_with_probe(path, Some(OsStr::new("relative")), never).is_err());
    }

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

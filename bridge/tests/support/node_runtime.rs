use std::ffi::OsStr;
use std::path::PathBuf;

fn resolve_node(path: &OsStr, _override_path: Option<&OsStr>) -> Result<PathBuf, String> {
    std::env::split_paths(path)
        .map(|directory| directory.join("node"))
        .find(|candidate| candidate.is_file())
        .ok_or_else(|| "Set BUILD_TEST_NODE to an absolute executable Node path".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};
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
            std::fs::write(shim, format!("#!/bin/sh\ntouch '{}'\nexit 78\n", marker.display())).unwrap();
        }
        let alias = temp.path().join("alias");
        std::fs::create_dir(&alias).unwrap();
        symlink(&mise, alias.join("node")).unwrap();
        let native = executable(temp.path(), "native/bin/node");
        let path = std::env::join_paths([mise.parent().unwrap(), asdf.parent().unwrap(), &alias, native.parent().unwrap()]).unwrap();
        assert_eq!(resolve_node(&path, None).unwrap(), native);
        assert!(!marker.exists());
    }

    #[test]
    fn absolute_override_wins_and_invalid_overrides_do_not_fall_back() {
        let temp = tempfile::tempdir().unwrap();
        let native = executable(temp.path(), "native/node");
        let renamed = executable(temp.path(), "other/node22");
        let path = native.parent().unwrap().as_os_str();
        assert_eq!(resolve_node(path, Some(renamed.as_os_str())).unwrap(), renamed);
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
        assert!(resolve_node(temp.path().join("missing").as_os_str(), None).unwrap_err().contains("BUILD_TEST_NODE"));
    }
}

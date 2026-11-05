//! Codex TUI reads its narrow shell capability through an owned launch profile.
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::pty::HarnessSpec;

const PREFIX: &str = "build-secret-";
const SUFFIX: &str = ".config.toml";

pub(crate) struct SecretProfile {
    path: PathBuf,
    _file: File,
}

impl SecretProfile {
    pub(crate) fn remove(&self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

impl Drop for SecretProfile {
    fn drop(&mut self) {
        self.remove();
    }
}

fn codex_home() -> io::Result<PathBuf> {
    let path = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".codex")))
        .ok_or_else(|| io::Error::other("Codex home unavailable"))?;
    if !path.is_absolute() {
        return Err(io::Error::other("Codex home is not absolute"));
    }
    Ok(path)
}

pub(crate) fn prepare(spec: HarnessSpec) -> Result<HarnessSpec, super::HarnessError> {
    if spec.binary != "codex" || spec.args.iter().any(|arg| arg == "app-server") {
        return Ok(spec);
    }
    if super::secret_launch::codex_thread_config(&spec, None).is_none() {
        return Ok(spec);
    }
    install(
        spec,
        &codex_home()
            .map_err(|_| super::HarnessError::Setup("secret profile unavailable".into()))?,
    )
    .map_err(|_| super::HarnessError::Setup("secret profile unavailable".into()))
}

fn install(mut spec: HarnessSpec, home: &Path) -> io::Result<HarnessSpec> {
    let config = super::secret_launch::codex_thread_config(&spec, None)
        .ok_or_else(|| io::Error::other("secret launch unavailable"))?;
    let mut contents = String::from("[shell_environment_policy]\nignore_default_excludes = false\nexclude = [\"BRIDGE_*\"]\ninclude_only = []\n\n[shell_environment_policy.set]\n");
    for (key, value) in config["shell_environment_policy"]["set"]
        .as_object()
        .unwrap()
    {
        // Keys are our fixed allowlist; JSON quoting also protects TOML strings.
        contents.push_str(&format!("{key} = {value}\n"));
    }
    let name = format!("{PREFIX}{}", uuid::Uuid::new_v4());
    let path = home.join(format!("{name}{SUFFIX}"));
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)?;
    let mut profile = SecretProfile { path, _file: file };
    lock(&profile._file)?;
    profile._file.write_all(contents.as_bytes())?;
    profile._file.sync_all()?;
    spec.args.splice(0..0, ["--profile".into(), name]);
    spec.secret_profile = Some(Arc::new(profile));
    Ok(spec)
}

fn lock(file: &File) -> io::Result<()> {
    // SAFETY: the live File owns this fd throughout the nonblocking flock call.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Remove only our unlocked, owned profiles. A live bridge of another channel
/// keeps its file lock, so starting one daemon never retires another's launch.
pub fn sweep_codex_secret_profiles() -> io::Result<()> {
    sweep(&codex_home()?)
}

fn sweep(home: &Path) -> io::Result<()> {
    let entries = match std::fs::read_dir(home) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let Some(id) = name
            .to_str()
            .and_then(|name| name.strip_prefix(PREFIX))
            .and_then(|name| name.strip_suffix(SUFFIX))
        else {
            continue;
        };
        if !uuid::Uuid::parse_str(id).is_ok_and(|uuid| uuid.to_string() == id) {
            continue;
        }
        let file = match OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(entry.path())
        {
            Ok(file) => file,
            Err(_) => continue,
        };
        let metadata = file.metadata()?;
        // SAFETY: geteuid has no pointer arguments or preconditions.
        if metadata.is_file() && metadata.uid() == unsafe { libc::geteuid() } && lock(&file).is_ok()
        {
            std::fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn launch() -> HarnessSpec {
        super::super::secret_launch::runtime_path(
            super::super::secret_launch::apply(
                HarnessSpec::new("codex"),
                Some(super::super::SecretLaunchEnvironment::new(
                    "/fixture.sock".into(),
                    "dev".into(),
                    "agent".into(),
                    "owner".into(),
                    "fixture-capability".into(),
                    "/installed/bin/build-bridge".into(),
                )),
            ),
            Path::new("/installed/bin/build-bridge"),
        )
        .unwrap()
        .env("BRIDGE_MCP_TOKEN", "unrelated-mcp-token")
    }

    #[test]
    fn private_profile_preserves_user_files_and_contains_only_narrow_set_values() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join("config.toml"), "# unchanged user config\n").unwrap();
        let spec = install(launch(), home.path()).unwrap();
        let profile = spec.secret_profile.as_ref().unwrap();
        assert_eq!(profile._file.metadata().unwrap().mode() & 0o777, 0o600);
        assert!(!spec
            .args
            .iter()
            .any(|arg| arg.contains("fixture-capability")));
        let contents = std::fs::read_to_string(&profile.path).unwrap();
        assert!(!contents.contains("unrelated-mcp-token"));
        let parsed: toml::Value = toml::from_str(&contents).unwrap();
        assert_eq!(
            parsed["shell_environment_policy"]["set"]
                .as_table()
                .unwrap()
                .len(),
            7
        );
        assert_eq!(
            parsed["shell_environment_policy"]["ignore_default_excludes"].as_bool(),
            Some(false)
        );
        let path = profile.path.clone();
        drop(spec);
        assert!(!path.exists());
        assert_eq!(
            std::fs::read_to_string(home.path().join("config.toml")).unwrap(),
            "# unchanged user config\n"
        );
    }

    #[test]
    fn startup_sweep_removes_owned_orphans_and_preserves_live_and_non_build_profiles() {
        let home = tempfile::tempdir().unwrap();
        let orphan = home
            .path()
            .join(format!("{PREFIX}{}{SUFFIX}", uuid::Uuid::new_v4()));
        std::fs::write(&orphan, "orphan").unwrap();
        let user = home.path().join("work.config.toml");
        std::fs::write(&user, "user").unwrap();
        let malformed = home.path().join("build-secret-user.config.toml");
        std::fs::write(&malformed, "not ours").unwrap();
        let noncanonical = home
            .path()
            .join(format!("{PREFIX}{}{SUFFIX}", uuid::Uuid::new_v4().simple()));
        std::fs::write(&noncanonical, "not canonical").unwrap();
        let symlink = home
            .path()
            .join(format!("{PREFIX}{}{SUFFIX}", uuid::Uuid::new_v4()));
        std::os::unix::fs::symlink(&user, &symlink).unwrap();
        let spec = install(launch(), home.path()).unwrap();
        let live = spec.secret_profile.as_ref().unwrap().path.clone();
        sweep(home.path()).unwrap();
        assert!(!orphan.exists());
        assert!(live.exists() && user.exists() && malformed.exists());
        assert!(symlink.symlink_metadata().unwrap().file_type().is_symlink());
        assert!(noncanonical.exists());
    }

    #[tokio::test]
    async fn profile_lives_through_spawn_and_is_removed_when_session_ends() {
        use crate::harness::AgentSession;
        let home = tempfile::tempdir().unwrap();
        let mut spec = install(launch(), home.path()).unwrap();
        let path = spec.secret_profile.as_ref().unwrap().path.clone();
        spec.binary = "/bin/sh".into();
        spec.args = vec!["-c".into(), "exec cat >/dev/null".into()];
        let session = crate::pty::PtySession::spawn(
            &spec,
            None,
            portable_pty::PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            },
        )
        .unwrap();
        drop(spec);
        assert!(path.exists());
        session.end();
        assert!(!path.exists());
    }

    #[test]
    fn failed_spawn_releases_the_profile_with_its_spec() {
        let home = tempfile::tempdir().unwrap();
        let mut spec = install(launch(), home.path()).unwrap();
        let path = spec.secret_profile.as_ref().unwrap().path.clone();
        spec.binary = "/nonexistent-build-secret-fixture".into();
        assert!(crate::pty::PtySession::spawn(
            &spec,
            None,
            portable_pty::PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            }
        )
        .is_err());
        drop(spec);
        assert!(!path.exists());
    }
}

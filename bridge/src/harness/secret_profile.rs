//! Codex TUI reads its narrow shell capability through an owned launch profile.
mod directory;
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
    directory: File,
}

impl SecretProfile {
    pub(crate) fn remove(&self) {
        if let Some(name) = self.path.file_name() {
            let _ = rustix::fs::unlinkat(&self.directory, name, rustix::fs::AtFlags::empty());
        }
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

pub(crate) fn prepare(
    mut spec: HarnessSpec,
    cwd: &Path,
) -> Result<HarnessSpec, super::HarnessError> {
    if spec.binary != "codex" {
        return Ok(spec);
    }
    if super::secret_launch::secret_values(&spec).is_none() {
        return Ok(spec);
    }
    let home = spec
        .env
        .iter()
        .rev()
        .find(|(key, _)| key == "CODEX_HOME")
        .map(|(_, value)| PathBuf::from(value))
        .map(Ok)
        .unwrap_or_else(codex_home)
        .map_err(|_| unavailable())?;
    // Codex must not create a fresh 0755 home before Build validates its path.
    directory::open_or_create(&home).map_err(|_| unavailable())?;
    spec.env.retain(|(key, _)| key != "CODEX_HOME");
    spec = spec.env("CODEX_HOME", home.to_str().ok_or_else(unavailable)?);
    let policy =
        super::installed::probe::codex_shell_policy(&spec, cwd).map_err(|_| unavailable())?;
    spec.codex_shell_policy = Some(Box::new(policy));
    if spec.args.iter().any(|arg| arg == "app-server") {
        return Ok(spec);
    }
    install(spec, &home).map_err(|_| unavailable())
}

fn unavailable() -> super::HarnessError {
    super::HarnessError::Setup("secret runtime policy unavailable".into())
}

fn install(mut spec: HarnessSpec, home: &Path) -> io::Result<HarnessSpec> {
    let config = super::secret_launch::codex_thread_config(&spec, None)
        .ok_or_else(|| io::Error::other("secret launch unavailable"))?;
    let contents =
        toml::to_string(&config).map_err(|_| io::Error::other("secret profile unavailable"))?;
    let name = format!("{PREFIX}{}", uuid::Uuid::new_v4());
    let file_name = format!("{name}{SUFFIX}");
    let path = home.join(&file_name);
    let directory = directory::open_or_create(home)?;
    let file = rustix::fs::openat(
        &directory,
        file_name.as_str(),
        rustix::fs::OFlags::WRONLY
            | rustix::fs::OFlags::CREATE
            | rustix::fs::OFlags::EXCL
            | rustix::fs::OFlags::NOFOLLOW
            | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::from_bits_truncate(0o600),
    )?;
    let mut profile = SecretProfile {
        path,
        _file: file.into(),
        directory,
    };
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
    fn first_launch_creates_a_private_owned_codex_home() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("fresh").join("codex");
        let spec = install(launch(), &home).unwrap();
        let metadata = std::fs::symlink_metadata(&home).unwrap();
        assert!(metadata.is_dir());
        assert_eq!(metadata.mode() & 0o777, 0o700);
        // SAFETY: geteuid has no pointer arguments or preconditions.
        assert_eq!(metadata.uid(), unsafe { libc::geteuid() });
        let profile = spec.secret_profile.as_ref().unwrap().path.clone();
        assert_eq!(std::fs::metadata(&profile).unwrap().mode() & 0o777, 0o600);
        drop(spec);
        assert!(!profile.exists());
    }

    #[test]
    fn profile_creation_never_follows_home_or_parent_symlinks() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("target");
        std::fs::create_dir(&target).unwrap();
        let link = root.path().join("linked-home");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(install(launch(), &link).is_err());
        assert!(install(launch(), &link.join("child")).is_err());
        assert_eq!(std::fs::read_dir(&target).unwrap().count(), 0);
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
            6
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

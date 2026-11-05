//! Optional Codex shell authority; ordinary harness startup never depends on it.
#[cfg(test)]
mod degraded_tests;
mod directory;
mod home;
mod registry;

use std::fs::File;
use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::pty::HarnessSpec;

const PREFIX: &str = "build-secret-";
const SUFFIX: &str = ".config.toml";

pub(crate) struct SecretProfile {
    path: PathBuf,
    _file: File,
    registration: registry::RegistryGuard,
}

impl SecretProfile {
    pub(crate) fn remove(&self) {
        let _ = self.registration.remove();
    }
}

pub fn registry_root(state_root: &Path) -> PathBuf {
    state_root.join("secret-profiles")
}

fn codex_home() -> io::Result<PathBuf> {
    let path = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".codex")))
        .ok_or_else(|| io::Error::other("Codex home unavailable"))?;
    if !path.is_absolute() {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    Ok(path)
}

pub(crate) fn prepare(spec: HarnessSpec, cwd: &Path) -> HarnessSpec {
    if spec.binary != "codex" || super::secret_launch::secret_values(&spec).is_none() {
        return spec;
    }
    let ordinary = spec.clone();
    match prepare_enabled(spec, cwd) {
        Ok(spec) => spec,
        Err(reason) => {
            let _ = writeln!(
                std::io::stderr().lock(),
                "secret runtime unavailable: {}",
                reason.code()
            );
            disable(ordinary)
        }
    }
}

#[derive(Clone, Copy)]
enum SetupFailure {
    HomeSymlink,
    Profile,
    Policy(super::installed::probe::PolicyFailure),
}

impl SetupFailure {
    fn code(self) -> &'static str {
        match self {
            Self::HomeSymlink => "home_symlink",
            Self::Profile => "profile_failure",
            Self::Policy(reason) => reason.code(),
        }
    }
}

fn prepare_enabled(mut spec: HarnessSpec, cwd: &Path) -> Result<HarnessSpec, SetupFailure> {
    let home = spec
        .env
        .iter()
        .rev()
        .find(|(key, _)| key == "CODEX_HOME")
        .map(|(_, value)| PathBuf::from(value))
        .map(Ok)
        .unwrap_or_else(codex_home)
        .map_err(|_| SetupFailure::Profile)?;
    // Resolve user paths normally. Symlink layouts keep ordinary Codex working,
    // while this optional authority stage declines to create a profile there.
    home::open_or_create(&home).map_err(|error| match error {
        home::HomeError::Symlink => SetupFailure::HomeSymlink,
        home::HomeError::Unavailable => SetupFailure::Profile,
    })?;
    spec.env.retain(|(key, _)| key != "CODEX_HOME");
    spec = spec.env("CODEX_HOME", home.to_str().ok_or(SetupFailure::Profile)?);
    let policy =
        super::installed::probe::codex_shell_policy(&spec, cwd).map_err(SetupFailure::Policy)?;
    spec.codex_shell_policy = Some(Box::new(policy));
    if spec.args.iter().any(|arg| arg == "app-server") {
        return Ok(spec);
    }
    install(spec, &home).map_err(|_| SetupFailure::Profile)
}

fn disable(mut spec: HarnessSpec) -> HarnessSpec {
    if let Some(profile) = spec.secret_profile.take() {
        profile.remove();
        let name = profile
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_suffix(SUFFIX));
        if let Some(name) = name {
            remove_profile_selection(&mut spec.args, name);
        }
    }
    spec.env
        .retain(|(key, _)| !key.starts_with("BRIDGE_SECRET_"));
    spec.codex_shell_policy = None;
    super::secret_launch::remove_runtime_path(&mut spec);
    spec
}

fn remove_profile_selection(args: &mut Vec<String>, name: &str) {
    let mut index = 0;
    while index + 1 < args.len() {
        if args[index] == "--profile" && args[index + 1] == name {
            args.drain(index..index + 2);
        } else {
            index += 1;
        }
    }
}

fn install(mut spec: HarnessSpec, home: &Path) -> io::Result<HarnessSpec> {
    let config = super::secret_launch::codex_thread_config(&spec, None)
        .ok_or_else(|| io::Error::other("secret launch unavailable"))?;
    let contents =
        toml::to_string(&config).map_err(|_| io::Error::other("secret profile unavailable"))?;
    let directory = home::open_or_create(home).map_err(|_| io::ErrorKind::PermissionDenied)?;
    let root = spec
        .secret_profile_registry_root
        .as_deref()
        .ok_or(io::ErrorKind::NotFound)?;
    let registry = registry::Registry::open(root)?;
    let id = uuid::Uuid::new_v4();
    let name = format!("{PREFIX}{id}");
    let file_name = format!("{name}{SUFFIX}");
    let pending = format!(".{PREFIX}{id}.pending");
    let mut file: File = rustix::fs::openat(
        &directory,
        pending.as_str(),
        rustix::fs::OFlags::WRONLY
            | rustix::fs::OFlags::CREATE
            | rustix::fs::OFlags::EXCL
            | rustix::fs::OFlags::NOFOLLOW
            | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::from_bits_truncate(0o600),
    )?
    .into();
    let published: io::Result<_> = (|| {
        registry::lock_owned_file(&file)?;
        // The durable registry owns the exact staged inode before any authority
        // bytes are written. A crash cannot leave an unregistered capability.
        let registration = registry.record(home, &directory, &file_name, &file, Some(&pending))?;
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
        publish_profile(&directory, &pending, &file_name)?;
        Ok(registration)
    })();
    if published.is_err() {
        cleanup_pending(&directory, &pending, &file);
    }
    let registration = published?;
    let profile = SecretProfile {
        path: home.join(&file_name),
        _file: file,
        registration,
    };
    spec.args.splice(0..0, ["--profile".into(), name]);
    spec.secret_profile = Some(Arc::new(profile));
    Ok(spec)
}

fn publish_profile(directory: &File, pending: &str, final_name: &str) -> io::Result<()> {
    // Both names are registered to one inode. Linking publishes atomically
    // without replacing an existing user entry; either name can be crash-cleaned.
    rustix::fs::linkat(
        directory,
        pending,
        directory,
        final_name,
        rustix::fs::AtFlags::empty(),
    )?;
    rustix::fs::unlinkat(directory, pending, rustix::fs::AtFlags::empty())?;
    directory.sync_all()
}

fn cleanup_pending(directory: &File, name: &str, file: &File) {
    let metadata = file.metadata();
    let stat = rustix::fs::statat(directory, name, rustix::fs::AtFlags::SYMLINK_NOFOLLOW);
    if let (Ok(metadata), Ok(stat)) = (metadata, stat) {
        if same_number(stat.st_dev, metadata.dev()) && same_number(stat.st_ino, metadata.ino()) {
            let _ = rustix::fs::unlinkat(directory, name, rustix::fs::AtFlags::empty());
        }
    }
}

fn same_number<T: TryInto<u64>>(raw: T, expected: u64) -> bool {
    raw.try_into().ok() == Some(expected)
}

fn lock(file: &File) -> io::Result<()> {
    // SAFETY: the live File owns this fd throughout the nonblocking flock call.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Sweep only exact profiles recorded in this bridge's private registry.
pub fn sweep_codex_secret_profiles(root: &Path) -> io::Result<()> {
    registry::Registry::open(root)?.sweep()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        _root: tempfile::TempDir,
        home: PathBuf,
        registry: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let root = tempfile::tempdir().unwrap();
            let home = root.path().join("codex-home");
            std::fs::create_dir(&home).unwrap();
            let registry = registry_root(root.path());
            Self {
                _root: root,
                home,
                registry,
            }
        }

        fn launch(&self) -> HarnessSpec {
            launch(&self.registry)
        }
    }

    fn launch(root: &Path) -> HarnessSpec {
        let mut spec = super::super::secret_launch::apply(
            HarnessSpec::new("codex"),
            Some(super::super::SecretLaunchEnvironment::new(
                "/fixture.sock".into(),
                "dev".into(),
                "agent".into(),
                "owner".into(),
                "fixture-capability".into(),
                "/installed/bin/build-bridge".into(),
            )),
        )
        .env("BRIDGE_MCP_TOKEN", "unrelated-mcp-token");
        spec.secret_profile_registry_root = Some(root.to_owned());
        spec
    }

    #[test]
    fn first_launch_creates_only_a_missing_private_home_leaf() {
        let fixture = Fixture::new();
        std::fs::remove_dir(&fixture.home).unwrap();
        let spec = install(fixture.launch(), &fixture.home).unwrap();
        let metadata = std::fs::metadata(&fixture.home).unwrap();
        assert_eq!(metadata.mode() & 0o777, 0o700);
        // SAFETY: geteuid has no pointer arguments or preconditions.
        assert_eq!(metadata.uid(), unsafe { libc::geteuid() });
        let path = spec.secret_profile.as_ref().unwrap().path.clone();
        assert_eq!(std::fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
        drop(spec);
        assert!(!path.exists());
        let missing = fixture._root.path().join("missing-parent/codex");
        assert!(install(fixture.launch(), &missing).is_err());
        assert!(!missing.parent().unwrap().exists());
    }

    #[test]
    fn profile_creation_declines_home_and_parent_symlinks_without_writing() {
        let fixture = Fixture::new();
        let link = fixture._root.path().join("linked-home");
        std::os::unix::fs::symlink(&fixture.home, &link).unwrap();
        assert!(install(fixture.launch(), &link).is_err());
        let parent = fixture._root.path().join("linked-parent");
        std::os::unix::fs::symlink(fixture._root.path(), &parent).unwrap();
        assert!(install(fixture.launch(), &parent.join("codex-home")).is_err());
        assert_eq!(std::fs::read_dir(&fixture.home).unwrap().count(), 0);
    }

    #[test]
    fn private_profile_preserves_user_files_and_contains_only_launch_values() {
        let fixture = Fixture::new();
        let user = fixture.home.join("config.toml");
        std::fs::write(&user, "# unchanged user config\n").unwrap();
        let spec = install(fixture.launch(), &fixture.home).unwrap();
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
        let path = profile.path.clone();
        drop(spec);
        assert!(!path.exists());
        assert_eq!(
            std::fs::read_to_string(user).unwrap(),
            "# unchanged user config\n"
        );
    }

    #[test]
    fn registry_sweep_preserves_live_and_unregistered_same_prefix_profiles() {
        let fixture = Fixture::new();
        let foreign = fixture
            .home
            .join(format!("{PREFIX}{}{SUFFIX}", uuid::Uuid::new_v4()));
        std::fs::write(&foreign, "not registered by Build").unwrap();
        let spec = install(fixture.launch(), &fixture.home).unwrap();
        let live = spec.secret_profile.as_ref().unwrap().path.clone();
        sweep_codex_secret_profiles(&fixture.registry).unwrap();
        assert!(live.exists());
        assert_eq!(
            std::fs::read_to_string(&foreign).unwrap(),
            "not registered by Build"
        );
        drop(spec);
        assert!(!live.exists());
        assert!(foreign.exists());
    }

    #[test]
    fn startup_registry_sweep_never_scans_a_symlinked_configured_home() {
        let fixture = Fixture::new();
        let foreign = fixture
            .home
            .join(format!("{PREFIX}{}{SUFFIX}", uuid::Uuid::new_v4()));
        std::fs::write(&foreign, "target must remain").unwrap();
        let alias = fixture._root.path().join("linked-home");
        std::os::unix::fs::symlink(&fixture.home, &alias).unwrap();
        sweep_codex_secret_profiles(&fixture.registry).unwrap();
        assert_eq!(
            std::fs::read_to_string(alias.join(foreign.file_name().unwrap())).unwrap(),
            "target must remain"
        );
    }

    #[test]
    fn startup_registry_sweep_never_scans_a_symlinked_configured_parent() {
        let fixture = Fixture::new();
        let foreign = fixture
            .home
            .join(format!("{PREFIX}{}{SUFFIX}", uuid::Uuid::new_v4()));
        std::fs::write(&foreign, "target must remain").unwrap();
        let alias = fixture._root.path().join("linked-parent");
        std::os::unix::fs::symlink(fixture._root.path(), &alias).unwrap();
        sweep_codex_secret_profiles(&fixture.registry).unwrap();
        assert_eq!(
            std::fs::read_to_string(alias.join("codex-home").join(foreign.file_name().unwrap()))
                .unwrap(),
            "target must remain"
        );
    }

    #[test]
    fn crashed_profile_is_reaped_from_its_record_even_with_a_different_home() {
        const CHILD: &str = "BUILD_PROFILE_REGISTRY_CRASH_CHILD";
        if std::env::var_os(CHILD).is_some() {
            let home = PathBuf::from(std::env::var_os("BUILD_PROFILE_TEST_HOME").unwrap());
            let registry = PathBuf::from(std::env::var_os("BUILD_PROFILE_TEST_REGISTRY").unwrap());
            let spec = install(launch(&registry), &home).unwrap();
            std::fs::write(
                home.join("crashed-profile-name"),
                spec.secret_profile
                    .as_ref()
                    .unwrap()
                    .path
                    .file_name()
                    .unwrap()
                    .as_encoded_bytes(),
            )
            .unwrap();
            std::process::exit(0);
        }
        let fixture = Fixture::new();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "harness::secret_profile::tests::crashed_profile_is_reaped_from_its_record_even_with_a_different_home", "--nocapture"])
            .env(CHILD, "1").env("BUILD_PROFILE_TEST_HOME", &fixture.home)
            .env("BUILD_PROFILE_TEST_REGISTRY", &fixture.registry).env("CODEX_HOME", "/different-user-home")
            .output().unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let name = std::fs::read_to_string(fixture.home.join("crashed-profile-name")).unwrap();
        let path = fixture.home.join(name);
        assert!(path.exists());
        sweep_codex_secret_profiles(&fixture.registry).unwrap();
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn profile_lives_through_spawn_and_is_removed_when_session_ends() {
        use crate::harness::AgentSession;
        let fixture = Fixture::new();
        let mut spec = install(fixture.launch(), &fixture.home).unwrap();
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
    fn failed_spawn_releases_only_its_registered_profile() {
        let fixture = Fixture::new();
        let mut spec = install(fixture.launch(), &fixture.home).unwrap();
        let path = spec.secret_profile.as_ref().unwrap().path.clone();
        spec.binary = "/nonexistent-build-secret-fixture".into();
        assert!(crate::pty::PtySession::spawn(
            &spec,
            None,
            portable_pty::PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0
            }
        )
        .is_err());
        drop(spec);
        assert!(!path.exists());
    }

    #[test]
    fn profile_publication_never_replaces_an_existing_user_entry() {
        let fixture = Fixture::new();
        let directory = home::open_or_create(&fixture.home).unwrap();
        std::fs::write(fixture.home.join("pending"), "owned staging file").unwrap();
        std::fs::write(fixture.home.join("existing"), "user-owned file").unwrap();
        assert!(publish_profile(&directory, "pending", "existing").is_err());
        assert_eq!(
            std::fs::read_to_string(fixture.home.join("existing")).unwrap(),
            "user-owned file"
        );
        assert_eq!(
            std::fs::read_to_string(fixture.home.join("pending")).unwrap(),
            "owned staging file"
        );
    }
}

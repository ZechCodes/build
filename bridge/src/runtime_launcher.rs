//! Checkout-independent `buildctl` in a private directory beside the bridge.

use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub const SOURCE: &str = include_str!("../runtime/buildctl");
const HEADER: &str = "#!/bin/sh\n# Agent runtime facade.";

/// Only an installed Build wrapper may be prepended to an agent's PATH.
/// Foreign files, symlinks and absent launchers provide no runtime directory.
pub fn bin_directory(binary: &Path) -> Option<PathBuf> {
    if !binary.is_absolute() {
        return None;
    }
    let directory = binary.parent()?.join(".build-runtime");
    let metadata = directory.symlink_metadata().ok()?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return None;
    }
    let launcher = directory.join("buildctl");
    if !is_runtime_launcher(&launcher)
        || launcher.metadata().ok()?.permissions().mode() & 0o111 == 0
    {
        return None;
    }
    Some(directory)
}

fn is_runtime_launcher(path: &Path) -> bool {
    let Ok(metadata) = path.symlink_metadata() else {
        return false;
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return false;
    }
    let Ok(file) = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    else {
        return false;
    };
    let mut header = String::new();
    file.take(64).read_to_string(&mut header).is_ok() && header.starts_with(HEADER)
}

fn warn_occupied() {
    eprintln!("runtime launcher destination is occupied; skipping");
}

/// Publish a launcher atomically without writing a global `buildctl` command.
/// Foreign private destinations are left intact and do not block installation.
pub fn install_sibling(binary: &Path) -> io::Result<()> {
    install_with_hooks(binary, |_| {}, || {}, sync_directory)
}

fn install_with_hooks(
    binary: &Path,
    after_write: impl FnOnce(&fs::File),
    on_busy: impl FnMut(),
    sync_parent: impl FnOnce(&Path) -> io::Result<()>,
) -> io::Result<()> {
    let directory = binary
        .parent()
        .ok_or(io::ErrorKind::InvalidInput)?
        .join(".build-runtime");
    match directory.symlink_metadata() {
        Ok(metadata) if !metadata.is_dir() || metadata.file_type().is_symlink() => {
            warn_occupied();
            return Ok(());
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            DirBuilder::new().mode(0o700).create(&directory)?;
        }
        Err(error) => return Err(error),
    }
    let destination = directory.join("buildctl");
    if destination.symlink_metadata().is_ok() && !is_runtime_launcher(&destination) {
        warn_occupied();
        return Ok(());
    }
    let temporary = directory.join(format!(".buildctl-{}", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o755)
            .open(&temporary)?;
        file.write_all(SOURCE.as_bytes())?;
        file.sync_all()?;
        after_write(&file);
        drop(file);
        // Closing this writer is not enough: a concurrent fork may still
        // retain its writable description until exec. Probe before publishing
        // so that no reader can observe an ETXTBSY launcher at the final path.
        ready_to_execute(&temporary, on_busy)?;
        fs::rename(&temporary, &destination)?;
        // Rename is the publication point. A later sync error affects
        // durability, not which launcher is installed; report it as a warning.
        if sync_parent(&directory).is_err() {
            let _ = writeln!(
                io::stderr().lock(),
                "runtime launcher published; directory sync failed"
            );
        }
        Ok(())
    })();
    let _ = fs::remove_file(temporary);
    result
}

fn sync_directory(path: &Path) -> io::Result<()> {
    fs::File::open(path)?.sync_all()
}

fn ready_to_execute(path: &Path, mut on_busy: impl FnMut()) -> io::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(1);
    let mut probe = Command::new(path);
    // With no arguments SOURCE exits 2 before resolving a bridge or reading
    // launch authority. A successful exec proves no writable description of
    // this inode remains, including descriptions retained by fork children.
    probe
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    loop {
        match probe.status() {
            Ok(status) if status.code() == Some(2) => return Ok(()),
            Ok(_) => return Err(io::Error::other("runtime launcher readiness probe failed")),
            Err(error) if error.raw_os_error() == Some(libc::ETXTBSY) => {
                if Instant::now() >= deadline {
                    return Err(error);
                }
                on_busy();
                std::thread::sleep(
                    Duration::from_millis(5)
                        .min(deadline.saturating_duration_since(Instant::now())),
                );
            }
            Err(error) => return Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::fs::MetadataExt;

    struct RetainedWriter {
        child: libc::pid_t,
        release: fs::File,
    }

    impl RetainedWriter {
        fn fork(file: &fs::File) -> Self {
            let mut pipe = [0; 2];
            // SAFETY: pipe points to two writable descriptor slots.
            assert_eq!(unsafe { libc::pipe(pipe.as_mut_ptr()) }, 0);
            for fd in pipe {
                // SAFETY: both descriptors were just returned by pipe.
                assert_eq!(
                    unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) },
                    0
                );
            }
            let writer = file.as_raw_fd();
            // SAFETY: this re-exec isolated actor uses only async signal safe
            // operations in the child, then _exit, without touching Rust locks.
            let child = unsafe { libc::fork() };
            assert!(child >= 0);
            if child == 0 {
                // SAFETY: the descriptors remain valid copies after fork, and
                // byte points to one writable byte. Only libc operations run.
                unsafe {
                    libc::close(pipe[1]);
                    let mut byte = 0u8;
                    while libc::read(pipe[0], (&mut byte as *mut u8).cast(), 1) < 0 {}
                    libc::close(writer);
                    libc::_exit(0);
                }
            }
            // SAFETY: the parent owns this descriptor and no longer reads it.
            unsafe {
                libc::close(pipe[0]);
            }
            Self {
                child,
                // SAFETY: pipe[1] is open and has no other owning File wrapper.
                release: unsafe { fs::File::from_raw_fd(pipe[1]) },
            }
        }
    }

    impl Drop for RetainedWriter {
        fn drop(&mut self) {
            let _ = self.release.write_all(&[1]);
            // SAFETY: child is our positive fork result, and waitpid permits a
            // null status pointer. Retry only interrupted waits.
            while unsafe { libc::waitpid(self.child, std::ptr::null_mut(), 0) } < 0 {
                if io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
                    break;
                }
            }
        }
    }

    #[test]
    fn retained_writer_is_not_published_as_an_executable_launcher() {
        let mode = std::env::var("BUILD_RUNTIME_WRITER_ACTOR").unwrap_or_default();
        if mode == "timeout" || mode == "release" {
            retained_writer_child_actor(&mode);
            return;
        }
        for mode in ["timeout", "release"] {
            let output = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "runtime_launcher::tests::retained_writer_is_not_published_as_an_executable_launcher",
                    "--nocapture",
                ])
                .env("BUILD_RUNTIME_WRITER_ACTOR", mode)
                .env("RUST_TEST_THREADS", "1")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    fn retained_writer_child_actor(mode: &str) {
        let directory = tempfile::tempdir().unwrap();
        let binary = directory.path().join("build-bridge");
        let runtime = directory.path().join(".build-runtime");
        fs::create_dir(&runtime).unwrap();
        let launcher = runtime.join("buildctl");
        let previous = format!("{HEADER} previous\nexit 47\n");
        fs::write(&launcher, &previous).unwrap();
        fs::set_permissions(&launcher, fs::Permissions::from_mode(0o700)).unwrap();
        let metadata = fs::metadata(&launcher).unwrap();
        let identity = (metadata.dev(), metadata.ino());
        let retained = RefCell::new(None);
        let observed_busy = Cell::new(false);
        let result = install_with_hooks(
            &binary,
            |file| {
                *retained.borrow_mut() = Some(RetainedWriter::fork(file));
            },
            || {
                observed_busy.set(true);
                if mode == "release" {
                    drop(retained.borrow_mut().take());
                }
            },
            sync_directory,
        );
        if mode == "timeout" {
            assert_eq!(result.unwrap_err().raw_os_error(), Some(libc::ETXTBSY));
            assert_eq!(fs::read_to_string(&launcher).unwrap(), previous);
            let metadata = fs::metadata(&launcher).unwrap();
            assert_eq!((metadata.dev(), metadata.ino()), identity);
        } else {
            result.unwrap();
            assert_eq!(Command::new(&launcher).status().unwrap().code(), Some(2));
            assert_eq!(fs::read_to_string(&launcher).unwrap(), SOURCE);
        }
        assert!(observed_busy.get());
        assert_eq!(fs::read_dir(runtime).unwrap().count(), 1);
        drop(retained.into_inner());
    }

    #[test]
    fn published_launcher_survives_directory_sync_and_warning_failures() {
        let mode = std::env::var("BUILD_RUNTIME_SYNC_ACTOR").unwrap_or_default();
        if mode == "warning" || mode == "broken-stderr" {
            directory_sync_child_actor(&mode);
            return;
        }
        for mode in ["warning", "broken-stderr"] {
            let output = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "runtime_launcher::tests::published_launcher_survives_directory_sync_and_warning_failures",
                    "--nocapture",
                ])
                .env("BUILD_RUNTIME_SYNC_ACTOR", mode)
                .env("RUST_TEST_THREADS", "1")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let expected = if mode == "warning" {
                b"runtime launcher published; directory sync failed\n".as_slice()
            } else {
                b"".as_slice()
            };
            assert_eq!(output.stderr, expected);
        }
    }

    fn directory_sync_child_actor(mode: &str) {
        let directory = tempfile::tempdir().unwrap();
        let binary = directory.path().join("build-bridge");
        let runtime = directory.path().join(".build-runtime");
        fs::create_dir(&runtime).unwrap();
        let launcher = runtime.join("buildctl");
        let previous = format!("{HEADER} previous\nexit 47\n");
        fs::write(&launcher, previous).unwrap();
        let metadata = fs::metadata(&launcher).unwrap();
        let identity = (metadata.dev(), metadata.ino());
        let result = install_with_hooks(
            &binary,
            |_| {},
            || {},
            |_| {
                if mode == "broken-stderr" {
                    // SAFETY: this is an isolated re-exec test, after all child
                    // spawning. Closing stderr simulates a failed diagnostic sink.
                    unsafe {
                        libc::close(libc::STDERR_FILENO);
                    }
                }
                Err(io::Error::from_raw_os_error(libc::EIO))
            },
        );
        result.unwrap();
        assert_eq!(fs::read_to_string(launcher).unwrap(), SOURCE);
        let metadata = fs::metadata(runtime.join("buildctl")).unwrap();
        assert_ne!((metadata.dev(), metadata.ino()), identity);
        assert_eq!(fs::read_dir(runtime).unwrap().count(), 1);
    }

    #[test]
    fn installs_executable_sibling_and_replaces_existing_launcher() {
        let directory = tempfile::tempdir().unwrap();
        let binary = directory.path().join("build-bridge");
        install_sibling(&binary).unwrap();
        let launcher = directory.path().join(".build-runtime/buildctl");
        assert_eq!(
            bin_directory(&binary),
            Some(directory.path().join(".build-runtime"))
        );
        assert_eq!(fs::read_to_string(&launcher).unwrap(), SOURCE);
        assert_ne!(
            fs::metadata(&launcher).unwrap().permissions().mode() & 0o111,
            0
        );
        fs::write(&launcher, "#!/bin/sh\n# Agent runtime facade. old\n").unwrap();
        install_sibling(&binary).unwrap();
        assert_eq!(fs::read_to_string(launcher).unwrap(), SOURCE);
        assert_eq!(
            fs::read_dir(directory.path().join(".build-runtime"))
                .unwrap()
                .count(),
            1
        );
    }

    #[test]
    fn foreign_global_buildctl_coexists_with_private_runtime() {
        let directory = tempfile::tempdir().unwrap();
        let global = directory.path().join("buildctl");
        fs::write(&global, "management command").unwrap();
        install_sibling(&directory.path().join("build-bridge")).unwrap();
        assert_eq!(fs::read_to_string(global).unwrap(), "management command");
        assert_eq!(
            fs::read_to_string(directory.path().join(".build-runtime/buildctl")).unwrap(),
            SOURCE
        );
    }

    #[test]
    fn foreign_private_runtime_is_skipped_without_replacement() {
        let directory = tempfile::tempdir().unwrap();
        let runtime = directory.path().join(".build-runtime");
        fs::create_dir(&runtime).unwrap();
        let launcher = runtime.join("buildctl");
        fs::write(&launcher, "foreign command").unwrap();
        install_sibling(&directory.path().join("build-bridge")).unwrap();
        assert_eq!(fs::read_to_string(launcher).unwrap(), "foreign command");
        assert_eq!(bin_directory(&directory.path().join("build-bridge")), None);
        assert!(!directory.path().join("buildctl").exists());
    }

    #[test]
    fn symlink_private_runtime_is_skipped_without_following_it() {
        let directory = tempfile::tempdir().unwrap();
        let foreign = directory.path().join("foreign");
        fs::create_dir(&foreign).unwrap();
        std::os::unix::fs::symlink(&foreign, directory.path().join(".build-runtime")).unwrap();
        install_sibling(&directory.path().join("build-bridge")).unwrap();
        assert_eq!(fs::read_dir(foreign).unwrap().count(), 0);
        assert_eq!(bin_directory(&directory.path().join("build-bridge")), None);
        assert!(!directory.path().join("buildctl").exists());
    }

    #[test]
    fn symlink_private_launcher_is_skipped_without_following_it() {
        let directory = tempfile::tempdir().unwrap();
        let runtime = directory.path().join(".build-runtime");
        fs::create_dir(&runtime).unwrap();
        let foreign = directory.path().join("foreign");
        fs::write(&foreign, "foreign command").unwrap();
        std::os::unix::fs::symlink(&foreign, runtime.join("buildctl")).unwrap();
        install_sibling(&directory.path().join("build-bridge")).unwrap();
        assert!(runtime.join("buildctl").is_symlink());
        assert_eq!(fs::read_to_string(foreign).unwrap(), "foreign command");
        assert_eq!(bin_directory(&directory.path().join("build-bridge")), None);
        assert!(!directory.path().join("buildctl").exists());
    }

    #[test]
    fn foreign_file_at_private_directory_is_skipped() {
        let directory = tempfile::tempdir().unwrap();
        let runtime = directory.path().join(".build-runtime");
        fs::write(&runtime, "foreign file").unwrap();
        install_sibling(&directory.path().join("build-bridge")).unwrap();
        assert_eq!(fs::read_to_string(runtime).unwrap(), "foreign file");
        assert_eq!(bin_directory(&directory.path().join("build-bridge")), None);
        assert!(!directory.path().join("buildctl").exists());
    }

    #[test]
    fn missing_relative_and_nonexecutable_wrappers_provide_no_path() {
        let directory = tempfile::tempdir().unwrap();
        let binary = directory.path().join("build-bridge");
        assert_eq!(bin_directory(&binary), None);
        assert_eq!(bin_directory(Path::new("build-bridge")), None);
        install_sibling(&binary).unwrap();
        let launcher = directory.path().join(".build-runtime/buildctl");
        fs::set_permissions(launcher, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(bin_directory(&binary), None);
    }
}

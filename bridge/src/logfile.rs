//! One rotating file for the daemon's stderr, where there was one that only
//! grew: `bridge.err.log` reached 16 MB in a day (issue #131).
//!
//! The service manager opens the file — `StandardError=append:` in the
//! systemd unit, `StandardErrorPath` in the launchd agent — and hands the
//! daemon a descriptor it cannot reopen. So the daemon rotates by copy and
//! cut: the file's contents go to `<file>.1` (the previous `.1` to `.2`, and
//! the oldest is dropped), then the file is cut back to nothing. The
//! descriptor was opened for append, so the next line lands at the new end;
//! a descriptor that was not is left alone, since cutting under it would
//! leave a hole the size of everything written so far. A line written
//! between the copy and the cut is lost: the price of not owning the file.

use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How big the file may grow before it is rotated.
pub const ROTATE_AT: u64 = 8 * 1024 * 1024;

/// How many rotated files are kept beside it: `.1` and `.2`, 24 MiB in all.
pub const KEEP: usize = 2;

/// How often the daemon looks at the file's size.
const CHECK_EVERY: Duration = Duration::from_secs(60);

/// Rotate `file`, open for append at `path`, if it has grown past `limit`,
/// keeping `keep` rotated files. Whether it rotated.
pub fn rotate_if_past(file: &File, path: &Path, limit: u64, keep: usize) -> io::Result<bool> {
    if file.metadata()?.len() <= limit || keep == 0 {
        return Ok(false);
    }
    for older in (1..keep).rev() {
        let from = rotated(path, older);
        if from.exists() {
            std::fs::rename(&from, rotated(path, older + 1))?;
        }
    }
    std::fs::copy(path, rotated(path, 1))?;
    file.set_len(0)?;
    Ok(true)
}

fn rotated(path: &Path, generation: usize) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(format!(".{generation}"));
    PathBuf::from(name)
}

/// The daemon's stderr as a file it can rotate: a regular file, opened for
/// append, and the path it was opened at. `None` for a terminal, a pipe, the
/// journal, or a file opened without append.
#[cfg(unix)]
pub fn stderr_file() -> Option<(File, PathBuf)> {
    use std::os::fd::FromRawFd;
    // A duplicate, so the `File` closes its own descriptor and never fd 2.
    let duplicate = unsafe { libc::dup(libc::STDERR_FILENO) };
    if duplicate < 0 {
        return None;
    }
    let file = unsafe { File::from_raw_fd(duplicate) };
    let appending = unsafe { libc::fcntl(duplicate, libc::F_GETFL) } & libc::O_APPEND != 0;
    if !appending || !file.metadata().ok()?.is_file() {
        return None;
    }
    Some((file, path_of(duplicate)?))
}

#[cfg(target_os = "linux")]
fn path_of(fd: std::os::fd::RawFd) -> Option<PathBuf> {
    std::fs::read_link(format!("/proc/self/fd/{fd}")).ok()
}

#[cfg(target_os = "macos")]
fn path_of(fd: std::os::fd::RawFd) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    let mut buffer = vec![0u8; libc::PATH_MAX as usize];
    if unsafe { libc::fcntl(fd, libc::F_GETPATH, buffer.as_mut_ptr()) } < 0 {
        return None;
    }
    let length = buffer.iter().position(|byte| *byte == 0)?;
    Some(PathBuf::from(std::ffi::OsStr::from_bytes(
        &buffer[..length],
    )))
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
fn path_of(_fd: std::os::fd::RawFd) -> Option<PathBuf> {
    None
}

/// Look at stderr's file every minute and rotate it past [`ROTATE_AT`], on
/// the blocking pool of the runtime this is called on. Says once which file
/// it rotates, or nothing when stderr is not a file it can rotate.
#[cfg(unix)]
pub fn spawn_rotation() {
    let Some((file, path)) = stderr_file() else {
        return;
    };
    crate::logline::say(format!(
        "log: {} rotates past {} MiB, {KEEP} kept",
        path.display(),
        ROTATE_AT / (1024 * 1024)
    ));
    let file = std::sync::Arc::new(file);
    tokio::spawn(async move {
        let mut every = tokio::time::interval(CHECK_EVERY);
        loop {
            every.tick().await;
            let (file, path) = (std::sync::Arc::clone(&file), path.clone());
            let rotated =
                tokio::task::spawn_blocking(move || rotate_if_past(&file, &path, ROTATE_AT, KEEP))
                    .await;
            if let Ok(Err(error)) = rotated {
                crate::logline::say(format!("log: rotation failed: {error}"));
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn appending(path: &Path) -> File {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap()
    }

    /// Past the limit the file's lines move to `.1`, the file is cut back to
    /// nothing, and the next line the same descriptor writes lands at its
    /// start. Two rotations later the oldest is gone and two are kept.
    #[test]
    fn a_file_past_its_limit_rotates_and_the_next_line_lands_at_its_start() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bridge.err.log");
        let mut log = appending(&path);

        log.write_all(&[b'a'; 100]).unwrap();
        assert!(
            !rotate_if_past(&log, &path, 100, 2).unwrap(),
            "not past it yet"
        );
        log.write_all(b"a").unwrap();
        assert!(rotate_if_past(&log, &path, 100, 2).unwrap());
        assert_eq!(std::fs::read(&path).unwrap(), b"");
        assert_eq!(std::fs::read(rotated(&path, 1)).unwrap(), vec![b'a'; 101]);

        log.write_all(&[b'b'; 120]).unwrap();
        assert_eq!(
            std::fs::read(&path).unwrap(),
            vec![b'b'; 120],
            "appended at the new end, no hole before it"
        );
        assert!(rotate_if_past(&log, &path, 100, 2).unwrap());
        log.write_all(&[b'c'; 130]).unwrap();
        assert!(rotate_if_past(&log, &path, 100, 2).unwrap());

        assert_eq!(std::fs::read(rotated(&path, 1)).unwrap(), vec![b'c'; 130]);
        assert_eq!(std::fs::read(rotated(&path, 2)).unwrap(), vec![b'b'; 120]);
        assert!(!rotated(&path, 3).exists(), "the oldest was dropped");
    }

    /// The path a descriptor was opened at is how the daemon finds the file
    /// the service manager gave it.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn a_descriptor_names_the_file_it_was_opened_at() {
        use std::os::fd::AsRawFd;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bridge.err.log");
        let log = appending(&path);
        assert_eq!(
            path_of(log.as_raw_fd()).map(|found| found.canonicalize().unwrap()),
            Some(path.canonicalize().unwrap())
        );
    }
}

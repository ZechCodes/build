//! One rotating file for the daemon's stderr, where there was one that only
//! grew: `bridge.err.log` reached 16 MB in a day (issue #131).
//!
//! The service manager opens the file — `StandardError=append:` in the
//! systemd unit, `StandardErrorPath` in the launchd agent — and hands the
//! daemon descriptor 2 on it. The daemon rotates by moving the file aside
//! and putting a fresh one in its place under the same descriptor: `<file>`
//! is renamed to `<file>.1` (the previous `.1` to `.2`, and the oldest is
//! dropped), a new `<file>` is opened for append, and `dup2` swaps it onto
//! descriptor 2 in one step. Every line is written whole through one
//! descriptor or the other, so none falls between them: a line written
//! before the swap lands at the end of `.1`, and every line after it in the
//! new file. Nothing is copied and nothing is cut.
//!
//! A child that inherited descriptor 2 before a rotation keeps writing to
//! the file it was given, which is `.1` by then, until it exits.

use std::io;
use std::os::fd::RawFd;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How big the file may grow before it is rotated.
pub const ROTATE_AT: u64 = 8 * 1024 * 1024;

/// How many rotated files are kept beside it: `.1` and `.2`, 24 MiB in all.
pub const KEEP: usize = 2;

/// How often the daemon looks at the file's size.
const CHECK_EVERY: Duration = Duration::from_secs(60);

/// Rotate the file under descriptor `fd`, opened at `path`, if it has grown
/// past `limit`, keeping `keep` rotated files: move it aside and swap a fresh
/// file at `path` onto `fd`. Whether it rotated.
pub fn rotate_if_past(fd: RawFd, path: &Path, limit: u64, keep: usize) -> io::Result<bool> {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    let current = size_and_mode(fd)?;
    if current.0 <= limit || keep == 0 {
        return Ok(false);
    }
    for older in (1..keep).rev() {
        let from = rotated(path, older);
        if from.exists() {
            std::fs::rename(&from, rotated(path, older + 1))?;
        }
    }
    std::fs::rename(path, rotated(path, 1))?;
    let fresh = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(current.1)
        .open(path);
    let swapped = fresh.and_then(|fresh| {
        // The mode asked for is filtered by the umask; the file the service
        // manager made is who may read the log, so it is copied exactly.
        fresh.set_permissions(std::fs::Permissions::from_mode(current.1))?;
        if unsafe { libc::dup2(fresh.as_raw_fd(), fd) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    });
    if let Err(error) = swapped {
        // The descriptor still writes to the file it had; put it back where
        // it was, so the next look finds it there rather than at `.1`.
        let _ = std::fs::rename(rotated(path, 1), path);
        return Err(error);
    }
    Ok(true)
}

/// The size of the file under `fd`, and its permission bits.
fn size_and_mode(fd: RawFd) -> io::Result<(u64, u32)> {
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    if unsafe { libc::fstat(fd, stat.as_mut_ptr()) } < 0 {
        return Err(io::Error::last_os_error());
    }
    let stat = unsafe { stat.assume_init() };
    // `st_mode` is a u16 on macOS and a u32 on Linux.
    #[allow(clippy::useless_conversion)]
    let mode = u32::from(stat.st_mode) & 0o7777;
    Ok((stat.st_size as u64, mode))
}

fn rotated(path: &Path, generation: usize) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(format!(".{generation}"));
    PathBuf::from(name)
}

/// The path of the regular file the daemon's stderr was opened at. `None`
/// for a terminal, a pipe or the journal.
pub fn stderr_path() -> Option<PathBuf> {
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    if unsafe { libc::fstat(libc::STDERR_FILENO, stat.as_mut_ptr()) } < 0 {
        return None;
    }
    let stat = unsafe { stat.assume_init() };
    if stat.st_mode & libc::S_IFMT != libc::S_IFREG {
        return None;
    }
    path_of(libc::STDERR_FILENO)
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
/// it rotates, or nothing when stderr is not a file.
pub fn spawn_rotation() {
    let Some(path) = stderr_path() else {
        return;
    };
    crate::logline::say(format!(
        "log: {} rotates past {} MiB, {KEEP} kept",
        path.display(),
        ROTATE_AT / (1024 * 1024)
    ));
    tokio::spawn(async move {
        let mut every = tokio::time::interval(CHECK_EVERY);
        loop {
            every.tick().await;
            let path = path.clone();
            let rotated = tokio::task::spawn_blocking(move || {
                rotate_if_past(libc::STDERR_FILENO, &path, ROTATE_AT, KEEP)
            })
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
    use std::fs::File;
    use std::io::Write;
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::PermissionsExt;

    fn appending(path: &Path) -> File {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap()
    }

    /// Past the limit the file moves to `.1`, and the next line the same
    /// descriptor writes lands in a fresh file at the path, made with the
    /// old one's permissions. Two rotations later the oldest is gone and two
    /// are kept.
    #[test]
    fn a_file_past_its_limit_rotates_and_the_next_line_lands_in_a_fresh_one() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bridge.err.log");
        let mut log = appending(&path);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
        let fd = log.as_raw_fd();

        log.write_all(&[b'a'; 100]).unwrap();
        assert!(
            !rotate_if_past(fd, &path, 100, 2).unwrap(),
            "not past it yet"
        );
        log.write_all(b"a").unwrap();
        assert!(rotate_if_past(fd, &path, 100, 2).unwrap());
        assert_eq!(std::fs::read(&path).unwrap(), b"");
        assert_eq!(std::fs::read(rotated(&path, 1)).unwrap(), vec![b'a'; 101]);
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o640,
            "the fresh file is as readable as the one it replaced"
        );

        log.write_all(&[b'b'; 120]).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), vec![b'b'; 120]);
        assert!(rotate_if_past(fd, &path, 100, 2).unwrap());
        log.write_all(&[b'c'; 130]).unwrap();
        assert!(rotate_if_past(fd, &path, 100, 2).unwrap());

        assert_eq!(std::fs::read(rotated(&path, 1)).unwrap(), vec![b'c'; 130]);
        assert_eq!(std::fs::read(rotated(&path, 2)).unwrap(), vec![b'b'; 120]);
        assert!(!rotated(&path, 3).exists(), "the oldest was dropped");
    }

    /// A writer that never stops, through the descriptor being rotated under
    /// it, loses no line and splits none: every line it wrote is whole in
    /// exactly one of the files. Cutting the file after copying it lost the
    /// lines written in between (#131 review).
    #[test]
    fn a_line_written_while_the_file_rotates_is_never_lost() {
        const ROTATIONS: usize = 200;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bridge.err.log");
        let log = appending(&path);
        let fd = log.as_raw_fd();
        let writing = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));

        let writer = {
            let writing = std::sync::Arc::clone(&writing);
            std::thread::spawn(move || {
                // One write(2) per line on the raw descriptor, the way
                // eprintln! writes to stderr, until the rotations are done.
                let mut lines = 0;
                while writing.load(std::sync::atomic::Ordering::SeqCst) {
                    let text = format!("line {lines:09} of the writer\n");
                    let wrote = unsafe { libc::write(fd, text.as_ptr().cast(), text.len()) };
                    assert_eq!(wrote, text.len() as isize);
                    lines += 1;
                }
                lines
            })
        };
        let mut rotations = 0;
        while rotations < ROTATIONS {
            if rotate_if_past(fd, &path, 1024, ROTATIONS + 1).unwrap() {
                rotations += 1;
            }
        }
        writing.store(false, std::sync::atomic::Ordering::SeqCst);
        let lines = writer.join().unwrap();

        let mut seen = vec![0usize; lines];
        let files = std::iter::once(path.clone()).chain((1..=rotations).map(|g| rotated(&path, g)));
        for file in files {
            for line in std::fs::read_to_string(&file).unwrap().lines() {
                let number: usize = line
                    .strip_prefix("line ")
                    .and_then(|rest| rest.strip_suffix(" of the writer"))
                    .and_then(|number| number.parse().ok())
                    .unwrap_or_else(|| panic!("a split or mangled line in {file:?}: {line:?}"));
                seen[number] += 1;
            }
        }
        let lost: Vec<_> = (0..lines).filter(|line| seen[*line] != 1).collect();
        assert!(
            lost.is_empty(),
            "{} of {lines} lines lost or doubled over {rotations} rotations, first {:?}",
            lost.len(),
            lost.first()
        );
        drop(log);
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

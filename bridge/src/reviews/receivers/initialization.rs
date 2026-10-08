//! Build receivers in isolated attempts so a dead Git child cannot block retry.

use super::{
    git, ownership, read_ownership, validate_placement, validate_receiver,
    validate_receiver_contents, write_owned_json, ReviewReceiver, OWNERSHIP_FILE,
};
use std::ffi::OsStr;
use std::fs::{self, File};
use std::io;
use std::path::Path;

pub(super) fn ensure_with(
    receiver: &ReviewReceiver,
    initialize: impl FnOnce(&ReviewReceiver) -> Result<(), String>,
) -> Result<(), String> {
    validate_placement(receiver)?;
    let parent = receiver
        .path
        .parent()
        .ok_or("invalid review receiver path")?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let reservation = parent.join(format!(
        ".{}.receiver-reservation.json",
        receiver.repository_id
    ));
    if receiver.path.exists() && !reservation.exists() {
        return validate_receiver(receiver);
    }
    write_owned_json(&reservation, &ownership(receiver))?;
    validate_reservation(&reservation, receiver)?;
    if !receiver.path.exists() {
        let attempt = create_attempt(receiver)?;
        initialize(&attempt)?;
        validate_receiver_contents(&attempt)?;
        sync_tree(&attempt.path)?;
        validate_placement(receiver)?;
        validate_reservation(&reservation, receiver)?;
        publish_attempt(&attempt, receiver)?;
    }
    validate_receiver(receiver)?;
    finish_reservation(&reservation, receiver)
}

fn validate_reservation(path: &Path, receiver: &ReviewReceiver) -> Result<(), String> {
    if read_ownership(path)? != ownership(receiver) {
        return Err("review receiver initialization ownership changed".into());
    }
    Ok(())
}

fn finish_reservation(path: &Path, receiver: &ReviewReceiver) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    validate_reservation(path, receiver)?;
    match fs::remove_file(path) {
        Ok(()) => File::open(path.parent().ok_or("invalid receiver reservation path")?)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| error.to_string()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

fn create_attempt(receiver: &ReviewReceiver) -> Result<ReviewReceiver, String> {
    let parent = receiver
        .path
        .parent()
        .ok_or("invalid review receiver path")?;
    let attempt = ReviewReceiver {
        path: parent.join(format!(
            ".{}.receiver-initialization-{}.git",
            receiver.repository_id,
            uuid::Uuid::new_v4()
        )),
        ..receiver.clone()
    };
    fs::create_dir(&attempt.path).map_err(|error| error.to_string())?;
    write_owned_json(&attempt.path.join(OWNERSHIP_FILE), &ownership(&attempt))?;
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| error.to_string())?;
    // Failed attempts remain untouched. In particular, retry never removes a
    // previous child's config.lock or any files changed after that child died.
    Ok(attempt)
}

pub(super) fn initialize_receiver(receiver: &ReviewReceiver) -> Result<(), String> {
    git(&receiver.path, &["init", "--bare"])?;
    for (key, value) in [
        ("core.hooksPath", "/dev/null"),
        ("gc.auto", "0"),
        ("maintenance.auto", "false"),
    ] {
        git(&receiver.path, &["config", "--local", key, value])?;
    }
    Ok(())
}

fn sync_tree(path: &Path) -> Result<(), String> {
    for entry in fs::read_dir(path).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let kind = entry.file_type().map_err(|error| error.to_string())?;
        if kind.is_dir() {
            sync_tree(&entry.path())?;
        } else if kind.is_file() {
            File::open(entry.path())
                .and_then(|file| file.sync_all())
                .map_err(|error| error.to_string())?;
        } else {
            return Err("review receiver initialization contains a nonregular file".into());
        }
    }
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| error.to_string())
}

fn publish_attempt(attempt: &ReviewReceiver, receiver: &ReviewReceiver) -> Result<(), String> {
    let parent = receiver
        .path
        .parent()
        .ok_or("invalid review receiver path")?;
    let directory = File::open(parent).map_err(|error| error.to_string())?;
    let source = attempt
        .path
        .file_name()
        .ok_or("invalid receiver attempt path")?;
    let target = receiver
        .path
        .file_name()
        .ok_or("invalid review receiver path")?;
    match rename_no_replace(&directory, source, target) {
        Ok(()) => directory.sync_all().map_err(|error| error.to_string()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => validate_receiver(receiver),
        Err(error) => Err(error.to_string()),
    }
}

#[cfg(target_os = "linux")]
fn rename_no_replace(directory: &File, source: &OsStr, target: &OsStr) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::fd::AsRawFd;
    let source = CString::new(source.as_encoded_bytes())?;
    let target = CString::new(target.as_encoded_bytes())?;
    // SAFETY: names are valid C strings relative to the live parent descriptor;
    // RENAME_NOREPLACE atomically refuses even an empty existing directory.
    // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage
    let result = unsafe {
        libc::renameat2(
            directory.as_raw_fd(),
            source.as_ptr(),
            directory.as_raw_fd(),
            target.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn rename_no_replace(directory: &File, source: &OsStr, target: &OsStr) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::fd::AsRawFd;
    let source = CString::new(source.as_encoded_bytes())?;
    let target = CString::new(target.as_encoded_bytes())?;
    // SAFETY: names are valid C strings relative to the live parent descriptor;
    // RENAME_EXCL atomically refuses even an empty existing directory.
    // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage
    let result = unsafe {
        libc::renameatx_np(
            directory.as_raw_fd(),
            source.as_ptr(),
            directory.as_raw_fd(),
            target.as_ptr(),
            libc::RENAME_EXCL,
        )
    };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn rename_no_replace(_directory: &File, _source: &OsStr, _target: &OsStr) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "atomic review receiver directory publication is unsupported on this platform",
    ))
}

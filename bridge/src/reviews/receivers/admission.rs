//! Close native Git admission durably, then drain on later maintenance turns.
use super::{locks::RecoveryWait, write_owned_json};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::Path;

pub(crate) const ADMISSION_DRAIN_SECS: u64 = 60 * 60;
pub(crate) const MARKER: &str = ".build-review-closed.json";
const HOOKS: &str = "hooks";
const REFERENCE_HOOK: &[u8] = b"#!/bin/sh\nif [ \"$1\" = prepared ] && [ -f \"${0%/*}/../.build-review-closed.json\" ]; then\n echo 'Build receiver is closed for reclamation' >&2\n exit 1\nfi\nexit 0\n";
const RECEIVE_HOOK: &[u8] = b"#!/bin/sh\nif [ -f \"${0%/*}/../.build-review-closed.json\" ]; then\n echo 'Build receiver is closed for reclamation' >&2\n exit 1\nfi\nexit 0\n";

#[derive(Serialize, Deserialize)]
struct Closed {
    format: u32,
    closed_at: u64,
    references: BTreeMap<String, String>,
}

pub(crate) fn references(receiver: &Path) -> Result<BTreeMap<String, String>, String> {
    let repository = git2::Repository::open_bare(receiver).map_err(|error| error.to_string())?;
    let references = repository
        .references()
        .map_err(|error| error.to_string())?
        .map(|reference| {
            let reference = reference.map_err(|error| error.to_string())?;
            let name = reference
                .name()
                .ok_or("receiver ref name is not UTF-8")?
                .to_owned();
            let target = reference
                .target()
                .map(|oid| oid.to_string())
                .or_else(|| reference.symbolic_target().map(str::to_owned))
                .ok_or("receiver ref target is not UTF-8")?;
            Ok((name, target))
        })
        .collect();
    references
}

pub(crate) fn check_closed_state(receiver: &Path) -> Result<(), String> {
    if let Some(closed) = read(receiver)? {
        if closed.references != references(receiver)? {
            return Err(format!(
                "preserved receiver with late refs after admission closed: {}",
                receiver.display()
            ));
        }
    }
    Ok(())
}

/// No sleep or blocking admission: grace and live locks defer to later turns.
pub(crate) fn ready(receiver: &Path, now: u64) -> Result<bool, String> {
    install_hooks(receiver)?;
    let closed = match read(receiver)? {
        Some(closed) => closed,
        None => {
            let closed = Closed {
                format: 1,
                closed_at: now,
                references: references(receiver)?,
            };
            write_owned_json(&receiver.join(MARKER), &closed)?;
            return Ok(false);
        }
    };
    check_closed_state(receiver)?;
    Ok(now.saturating_sub(closed.closed_at) >= ADMISSION_DRAIN_SECS && !has_locks(receiver)?)
}

/// A new opening holds opening.lock and cancels any pending reclamation.
pub(crate) fn reopen(receiver: &Path) -> Result<(), String> {
    if read(receiver)?.is_some() {
        fs::remove_file(receiver.join(MARKER)).map_err(|error| error.to_string())?;
        File::open(receiver)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn read(receiver: &Path) -> Result<Option<Closed>, String> {
    let path = receiver.join(MARKER);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    if !metadata.is_file() {
        return Err("receiver close marker is not a regular file".into());
    }
    let closed: Closed =
        serde_json::from_slice(&fs::read(path).map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())?;
    if closed.format != 1 {
        return Err("unknown receiver close marker format".into());
    }
    Ok(Some(closed))
}

fn install_hooks(receiver: &Path) -> Result<(), String> {
    let hooks = receiver.join(HOOKS);
    fs::create_dir_all(&hooks).map_err(|error| error.to_string())?;
    if !fs::symlink_metadata(&hooks)
        .map_err(|error| error.to_string())?
        .is_dir()
    {
        return Err("receiver admission hook directory changed".into());
    }
    install_hook(&hooks.join("reference-transaction"), REFERENCE_HOOK)?;
    install_hook(&hooks.join("pre-receive"), RECEIVE_HOOK)?;
    super::with_local_config_locked_with_recovery_wait(
        receiver,
        RecoveryWait::TryOnly,
        |current, staged| {
            if let Some(configured) = hooks_path(current)? {
                if Path::new(&configured) == receiver.join(".build-review-hooks") {
                    verify_hooks(&receiver.join(".build-review-hooks"))?;
                } else if configured != "/dev/null" {
                    return Err("preserved receiver with foreign Git hooks".into());
                }
            }
            match staged.remove_multivar("core.hooksPath", ".*") {
                Ok(()) => Ok(()),
                Err(error) if error.code() == git2::ErrorCode::NotFound => Ok(()),
                Err(error) => Err(error.to_string()),
            }
        },
    )?;
    // Removing the local setting can expose a global or included override.
    verify_default_hooks(receiver)
}

fn hooks_path(config: &git2::Config) -> Result<Option<String>, String> {
    match config.get_string("core.hooksPath") {
        Ok(path) => Ok(Some(path)),
        Err(error) if error.code() == git2::ErrorCode::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

fn verify_default_hooks(receiver: &Path) -> Result<(), String> {
    let repository = git2::Repository::open_bare(receiver).map_err(|error| error.to_string())?;
    if hooks_path(&repository.config().map_err(|error| error.to_string())?)?.is_some() {
        return Err("preserved receiver with overridden admission hooks".into());
    }
    verify_hooks(&receiver.join(HOOKS))
}

/// Read-only recheck after quarantine, before taking the final inventory.
pub(crate) fn verify_closed(receiver: &Path) -> Result<(), String> {
    read(receiver)?.ok_or("preserved receiver without admission close marker")?;
    verify_default_hooks(receiver)?;
    check_closed_state(receiver)
}

fn verify_hooks(hooks: &Path) -> Result<(), String> {
    if !fs::symlink_metadata(hooks)
        .map_err(|error| error.to_string())?
        .is_dir()
    {
        return Err("receiver admission hook directory changed".into());
    }
    verify_hook(&hooks.join("reference-transaction"), REFERENCE_HOOK)?;
    verify_hook(&hooks.join("pre-receive"), RECEIVE_HOOK)
}

fn install_hook(path: &Path, bytes: &[u8]) -> Result<(), String> {
    #[cfg(unix)]
    use std::os::unix::fs::OpenOptionsExt;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o755);
    match options.open(path) {
        Ok(mut file) => {
            file.write_all(bytes)
                .and_then(|()| file.sync_all())
                .map_err(|error| error.to_string())?;
            File::open(path.parent().ok_or("invalid admission hook path")?)
                .and_then(|directory| directory.sync_all())
                .map_err(|error| error.to_string())?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            verify_hook(path, bytes)?;
        }
        Err(error) => return Err(error.to_string()),
    }
    Ok(())
}

fn verify_hook(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if !metadata.is_file() || fs::read(path).map_err(|error| error.to_string())? != bytes {
        return Err("preserved receiver with foreign admission hook".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err("receiver admission hook is not executable".into());
        }
    }
    Ok(())
}

fn has_locks(receiver: &Path) -> Result<bool, String> {
    locks_below(receiver)
}

fn locks_below(path: &Path) -> Result<bool, String> {
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.to_string()),
    };
    for entry in entries {
        let entry = entry.map_err(|error| error.to_string())?;
        if entry.file_name().to_string_lossy().ends_with(".lock") {
            return Ok(true);
        }
        let kind = entry.file_type().map_err(|error| error.to_string())?;
        if kind.is_symlink() {
            return Err("receiver ref lock directory contains a symlink".into());
        }
        if kind.is_dir() && locks_below(&entry.path())? {
            return Ok(true);
        }
    }
    Ok(false)
}

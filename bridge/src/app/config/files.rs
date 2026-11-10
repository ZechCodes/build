//! Atomic config replacement and the two distinct recovery snapshots.
use crate::app::AppState;
use serde_json::Value;
use std::io::Write;
use std::path::{Path, PathBuf};

pub(super) fn adjacent(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

fn io_error(path: &Path, error: std::io::Error) -> String {
    format!("write config {}: {error}", path.display())
}

/// Publish a complete snapshot without ever overwriting the first one. Linking
/// the synced temporary file makes the destination appear atomically; a crash
/// while writing the temporary file cannot leave a truncated upgrade backup.
pub(super) fn backup_before_environments(path: &Path) -> Result<(), String> {
    let backup = adjacent(path, ".pre-environments");
    match std::fs::symlink_metadata(&backup) {
        Ok(metadata) if metadata.is_file() => return sync_parent(&backup),
        Ok(_) => {
            return Err(format!(
                "config backup {} is not a regular file",
                backup.display()
            ))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(io_error(&backup, error)),
    }
    let bytes = std::fs::read(path).map_err(|error| io_error(path, error))?;
    with_temporary(&backup, &bytes, |temporary| {
        match std::fs::hard_link(temporary, &backup) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                if std::fs::symlink_metadata(&backup).is_ok_and(|metadata| metadata.is_file()) {
                    Ok(())
                } else {
                    Err(io_error(&backup, error))
                }
            }
            Err(error) => Err(io_error(&backup, error)),
        }
    })?;
    sync_parent(&backup)
}

fn sync_parent(path: &Path) -> Result<(), String> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| io_error(path, error))
}

fn with_temporary(
    path: &Path,
    bytes: &[u8],
    publish: impl FnOnce(&Path) -> Result<(), String>,
) -> Result<(), String> {
    let temporary = adjacent(path, ".tmp");
    remove_temporary(&temporary)?;
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|error| io_error(path, error))?;
        file.write_all(bytes)
            .and_then(|()| file.sync_all())
            .map_err(|error| io_error(path, error))?;
        publish(&temporary)
    })();
    let cleanup = remove_temporary(&temporary);
    result.and(cleanup)
}

fn remove_temporary(path: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io_error(path, error)),
    }
}

impl AppState {
    pub(super) fn write_config_file(&self, path: &Path, config: &Value) -> Result<(), String> {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent).map_err(|error| io_error(path, error))?;
        }
        let bytes = serde_json::to_vec_pretty(config)
            .map_err(|error| format!("serialize config {}: {error}", path.display()))?;
        let previous = match std::fs::read(path) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(io_error(path, error)),
        };
        #[cfg(test)]
        if let Some(gate) = &self.config_persist_gate {
            gate.arrive();
        }
        #[cfg(test)]
        if self.config_persist_failure == Some(crate::app::ConfigPersistStep::Write) {
            return Err(format!(
                "write config {}: injected write failure",
                path.display()
            ));
        }
        with_temporary(path, &bytes, |temporary| {
            self.replace_config_file(path, temporary, previous.as_deref())
        })
    }

    fn replace_config_file(
        &self,
        path: &Path,
        temporary: &Path,
        previous: Option<&[u8]>,
    ) -> Result<(), String> {
        #[cfg(test)]
        if self.config_persist_failure == Some(crate::app::ConfigPersistStep::Rename) {
            return Err(format!(
                "write config {}: injected rename failure",
                path.display()
            ));
        }
        if let Some(bytes) = previous {
            let snapshot = adjacent(path, ".previous");
            with_temporary(&snapshot, bytes, |temporary| {
                std::fs::rename(temporary, &snapshot).map_err(|error| io_error(&snapshot, error))
            })?;
            sync_parent(&snapshot)?;
        }
        // Rename is the success boundary. All fallible backup durability work
        // happens before it, so a reported error never replaces current config.
        std::fs::rename(temporary, path).map_err(|error| io_error(path, error))
    }
}

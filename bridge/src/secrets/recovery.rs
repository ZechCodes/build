//! Only confirmed key absence retires grants. All other storage faults leave
//! metadata and files intact so repairing the fault can restore secret access.
use super::vault::Vault;
use super::{Result, SecretError};
use crate::store::StoreWriter;
use std::path::Path;

pub(super) fn open_vault_with<F>(
    root: &Path,
    writer: &StoreWriter,
    now: i64,
    opener: F,
) -> Result<Option<Vault>>
where
    F: FnOnce(&Path, bool) -> Result<Vault>,
{
    let unavailable = writer.secrets_unretired_count()?;
    match opener(root, true) {
        Ok(vault) => {
            if let Some(vault) = resume_registered_key(vault, writer) {
                return Ok(Some(vault));
            }
        }
        Err(error) if confirmed_key_loss(root, &error) => {
            if unavailable > 0 {
                report_unavailable(unavailable);
            }
            return Ok(initialize_missing_key(root, writer, now));
        }
        Err(_) => {}
    }
    // Includes corrupt keys, unsafe paths/modes/owners, EIO, and unknown or
    // malformed expected identifiers. None grants permission to rewrite state.
    report_unavailable(unavailable);
    Ok(None)
}

fn confirmed_key_loss(root: &Path, error: &SecretError) -> bool {
    matches!(error, SecretError::Io(error) if error.kind() == std::io::ErrorKind::NotFound)
        && Vault::key_is_absent(root).unwrap_or(false)
}

fn initialize_missing_key(root: &Path, writer: &StoreWriter, now: i64) -> Option<Vault> {
    // Even initial creation crosses this FULL-sync retirement barrier. Neither
    // a failure nor an unretired grant permits any filesystem modification.
    writer.secrets_retire_key_lost(now).ok()?;
    if writer.secrets_unretired_count().ok() != Some(0) {
        return None;
    }
    std::fs::create_dir_all(root).ok()?;
    let vault = Vault::prepare_after_retirement(root).ok()?;
    writer.secrets_prepare_vault_key(&vault.key_id()).ok()?;
    vault.publish_key().ok()?;
    activate_pending_key(vault, writer)
}

fn resume_registered_key(vault: Vault, writer: &StoreWriter) -> Option<Vault> {
    if writer.secret_vault_key_id().ok()? == Some(vault.key_id()) {
        return Some(vault);
    }
    if writer.secret_vault_pending_key_id().ok()? != Some(vault.key_id())
        || writer.secrets_unretired_count().ok()? != 0
    {
        return None;
    }
    activate_pending_key(vault, writer)
}

fn activate_pending_key(vault: Vault, writer: &StoreWriter) -> Option<Vault> {
    // A previous attempt may have failed after rename but before directory
    // fsync. Retry file AND directory durability before registering this key.
    vault.sync_key_publication().ok()?;
    writer.secrets_finish_vault_key(&vault.key_id()).ok()?;
    Some(vault)
}

pub(super) fn report_unavailable(count: usize) {
    emit_unavailable(count, |message| eprintln!("{message}"));
}

fn emit_unavailable(count: usize, mut sink: impl FnMut(&str)) {
    sink(&format!("secret vault unavailable records: {count}"));
}

#[cfg(test)]
#[path = "recovery_tests.rs"]
mod tests;

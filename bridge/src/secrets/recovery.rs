//! Key loss retires grants, never the rest of the bridge. A key identifier in
//! metadata detects even same-length key substitution without storing key bytes.
use super::vault::Vault;
use super::{Result, SecretError};
use crate::store::StoreWriter;
use std::path::Path;

pub(super) fn open_vault(root: &Path, writer: &StoreWriter, now: i64) -> Result<Option<Vault>> {
    let any_records = writer.secrets_have_records()?;
    let unretired = writer.secrets_unretired_count()?;
    let expected = match writer.secret_vault_key_id() {
        Ok(expected) => expected,
        Err(SecretError::Unavailable) => return Ok(reset(root, writer, now, Some(unretired))),
        Err(error) => return Err(error),
    };
    let Some(expected) = expected else {
        // Never adopt an unregistered key, even if a crash after a zero-row
        // retirement left a same-length bad key and no lifetime records.
        return Ok(reset(root, writer, now, any_records.then_some(unretired)));
    };
    if let Ok(vault) = Vault::open(root, true) {
        if vault.key_id() == expected {
            return Ok(Some(vault));
        }
    }
    Ok(reset(root, writer, now, Some(unretired)))
}

fn reset(root: &Path, writer: &StoreWriter, now: i64, diagnostic: Option<usize>) -> Option<Vault> {
    // At most one bounded diagnostic per startup. No name, ID or error detail.
    if let Some(count) = diagnostic {
        eprintln!("secret vault unavailable records: {count}");
    }
    // Even initial creation crosses this durable barrier before making a key.
    // Retirement failures close only secret access and leave the key untouched.
    writer.secrets_retire_key_lost(now).ok()?;
    if writer.secrets_unretired_count().ok() != Some(0) {
        return None;
    }
    std::fs::create_dir_all(root).ok()?;
    let vault = Vault::reinitialize_after_retirement(root).ok()?;
    register(writer, vault)
}

fn register(writer: &StoreWriter, vault: Vault) -> Option<Vault> {
    writer.secrets_set_vault_key_id(&vault.key_id()).ok()?;
    Some(vault)
}

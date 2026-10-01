//! The bridge's durable device identity — what binds this daemon to a user account.
//!
//! On first run the bridge generates an Ed25519 identity keypair (signs the relay's
//! auth challenge and registration) plus an X25519 transport keypair (clients wrap
//! session keys to it), and persists them to a single `0600` file (default
//! `~/.build/identity.json`). The same `device_id` + identity public key is what the
//! user approves during pairing, so the relay can verify this device on every connect.
//!
//! Keys are security-sensitive, so the file is written atomically (temp + chmod +
//! rename) and locked to the owner — never world- or group-readable.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::relay::DeviceIdentity;
use crate::transport::{self, KeyPairB64};

#[derive(Debug, thiserror::Error)]
pub enum IdentityError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("serde error: {0}")]
    Serde(#[from] serde_json::Error),
}

type Result<T> = std::result::Result<T, IdentityError>;

/// The persisted identity. `approved` flips to true once the human pairs this device
/// to their account, gating whether `serve` connects to the relay or first pairs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredIdentity {
    pub device_id: String,
    pub name: String,
    /// Ed25519 seed (base64) — signs the auth challenge and registration.
    pub identity_private_key_b64: String,
    /// Ed25519 public (base64) — cached so the fingerprint/verify path needn't re-derive.
    pub identity_public_key_b64: String,
    /// The durable X25519 transport keypair clients wrap session keys to.
    pub transport: KeyPairB64,
    /// Set once the user approves this device; until then the bridge stays unpaired.
    pub approved: bool,
}

/// The name a new identity is given: `BRIDGE_DEVICE_NAME`, else the host
/// name, else `bridge`. Also how the bridge names its machine in a sentence
/// the user reads.
pub fn default_device_name() -> String {
    std::env::var("BRIDGE_DEVICE_NAME").unwrap_or_else(|_| hostname())
}

/// A human-recognizable device name. Falls back to `bridge` when the host
/// name can't be determined.
fn hostname() -> String {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "bridge".to_string())
}

/// Generate a fresh, unpaired identity with a random `device_id` and the given name.
/// Pure: only calls the key generators and packs the struct.
pub fn generate(name: &str) -> StoredIdentity {
    let identity = transport::generate_identity_keypair();
    StoredIdentity {
        device_id: uuid::Uuid::new_v4().to_string(),
        name: name.to_string(),
        identity_private_key_b64: identity.private_key_b64,
        identity_public_key_b64: identity.public_key_b64,
        transport: transport::generate_transport_keypair(),
        approved: false,
    }
}

/// Load a stored identity, or `None` if the file does not exist.
pub fn load(path: &Path) -> Result<Option<StoredIdentity>> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    Ok(Some(serde_json::from_slice(&bytes)?))
}

/// Atomically persist `identity` to `path` with `0600` permissions: create a sibling
/// temp file that is owner-only from its **first byte** (`O_CREAT` with mode `0600`,
/// not write-then-chmod — a chmod-after-write leaves a window where another local
/// user could read the private keys), then rename over the target. Best-effort
/// `0700` on the parent directory. Does not mutate `identity`.
pub fn save(path: &Path, identity: &StoredIdentity) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
            let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
        }
    }

    let json = serde_json::to_vec_pretty(identity)?;
    let tmp = path.with_extension("tmp");
    write_owner_only(&tmp, &json)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Write `bytes` to a file that is `0600` from the moment it exists — the mode
/// rides the `open(2)` call, so there is no world-readable window at all.
fn write_owner_only(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    // Remove any leftover tmp first: O_CREAT's mode only applies to newly
    // created files, and a stale file could carry looser permissions.
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// Move `identity`'s file aside, beside itself and named for its device, so the
/// next load finds none and mints a new identity. A rename, so the kept copy is
/// the same owner-only file. Returns where it went.
pub fn retire(path: &Path, identity: &StoredIdentity) -> Result<PathBuf> {
    let mut kept_name = path.file_name().unwrap_or_default().to_os_string();
    kept_name.push(format!(".retired-{}", identity.device_id));
    let kept_at = path.with_file_name(kept_name);
    std::fs::rename(path, &kept_at)?;
    Ok(kept_at)
}

/// Build the runtime [`DeviceIdentity`] the relay client needs from a stored identity.
/// Pure.
pub fn to_device_identity(stored: &StoredIdentity) -> DeviceIdentity {
    DeviceIdentity {
        device_id: stored.device_id.clone(),
        identity_private_key_b64: stored.identity_private_key_b64.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport;

    #[test]
    fn load_missing_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("does-not-exist.json");
        assert!(load(&path).unwrap().is_none());
    }

    #[test]
    fn save_then_load_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("identity.json");
        let id = generate("my-box");
        save(&path, &id).unwrap();
        let loaded = load(&path).unwrap().expect("file exists after save");
        assert_eq!(loaded, id);
    }

    #[test]
    fn saved_file_is_0600() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("identity.json");
        save(&path, &generate("my-box")).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "identity file must be owner-only");
    }

    #[test]
    fn temp_file_is_owner_only_from_creation() {
        // The mode must ride the open(2) call — chmod-after-write would leave a
        // window where another local user can read the private keys.
        let dir = tempfile::tempdir().unwrap();
        let tmp = dir.path().join("identity.tmp");
        write_owner_only(&tmp, b"secret").unwrap();
        let mode = std::fs::metadata(&tmp).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "temp file must be owner-only at creation");

        // A stale, looser-permissioned leftover tmp is replaced, not reused.
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o644)).unwrap();
        write_owner_only(&tmp, b"secret-2").unwrap();
        let mode = std::fs::metadata(&tmp).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "stale tmp perms must not survive");
        assert_eq!(std::fs::read(&tmp).unwrap(), b"secret-2");
    }

    #[test]
    fn generate_produces_consistent_keys() {
        let id = generate("my-box");
        assert!(!id.approved);
        // The cached public key matches a signature made by the stored private key.
        let challenge = b"1750000000.GET./ws/device";
        let sig = transport::sign_message_b64(&id.identity_private_key_b64, challenge).unwrap();
        transport::verify_message_b64(&id.identity_public_key_b64, challenge, &sig)
            .expect("stored public key verifies a signature from the stored private key");
    }

    #[test]
    fn save_overwrites_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("identity.json");
        let first = generate("box-1");
        save(&path, &first).unwrap();

        let mut second = generate("box-2");
        second.approved = true;
        save(&path, &second).unwrap();

        let loaded = load(&path).unwrap().unwrap();
        assert_eq!(loaded, second);
        assert!(loaded.approved);
        // No leftover temp file beside the target.
        assert!(!path.with_extension("tmp").exists());
    }

    /// A retired identity moves aside whole — same bytes, still owner-only —
    /// and leaves nothing at the live path, so the next load mints a new one.
    #[test]
    fn retire_moves_the_identity_aside_and_keeps_it_owner_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("identity.json");
        let mut id = generate("my-box");
        id.approved = true;
        save(&path, &id).unwrap();

        let kept = retire(&path, &id).unwrap();

        assert!(!path.exists(), "the live identity is gone");
        assert_eq!(kept.parent(), path.parent(), "kept beside the live path");
        assert!(
            kept.file_name()
                .unwrap()
                .to_string_lossy()
                .contains(&id.device_id),
            "the kept file names the device it was: {kept:?}"
        );
        assert_eq!(load(&kept).unwrap(), Some(id));
        let mode = std::fs::metadata(&kept).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "a retired identity still holds private keys");
    }

    /// The relay identity is who the device is and the seed that signs its
    /// challenge; the transport keypair stays with the stored identity, whose
    /// one runtime holder is the `FrameIntake` that opens session keys.
    #[test]
    fn to_device_identity_carries_the_keys_the_relay_auth_needs() {
        let id = generate("my-box");
        let dev = to_device_identity(&id);
        assert_eq!(dev.device_id, id.device_id);
        assert_eq!(dev.identity_private_key_b64, id.identity_private_key_b64);
    }
}

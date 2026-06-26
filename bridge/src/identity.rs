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
use std::path::Path;

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

/// Atomically persist `identity` to `path` with `0600` permissions: write a sibling
/// temp file, lock it down, then rename over the target — so there is never a window
/// where the final file is partially written or group/other-readable. Best-effort
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
    std::fs::write(&tmp, &json)?;
    // Set perms before the rename so the final path is never readable by others.
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Build the runtime [`DeviceIdentity`] the relay client needs from a stored identity.
/// Pure.
pub fn to_device_identity(stored: &StoredIdentity) -> DeviceIdentity {
    DeviceIdentity {
        device_id: stored.device_id.clone(),
        identity_private_key_b64: stored.identity_private_key_b64.clone(),
        transport: stored.transport.clone(),
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

    #[test]
    fn to_device_identity_carries_keys() {
        let id = generate("my-box");
        let dev = to_device_identity(&id);
        assert_eq!(dev.device_id, id.device_id);
        assert_eq!(dev.identity_private_key_b64, id.identity_private_key_b64);
        assert_eq!(dev.transport.public_key_b64, id.transport.public_key_b64);
    }
}

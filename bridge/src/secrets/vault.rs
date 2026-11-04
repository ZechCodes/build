//! Encrypted files behind a pinned private directory descriptor. Plaintext is
//! never passed to the filesystem; all leaf names are generated identifiers.
use super::{validate_id, Result, SecretContext, SecretError, SecretValue, MAX_VALUE_BYTES};
use aes_gcm::aead::{Aead, AeadCore, KeyInit, OsRng, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use dryoc::types::{Bytes, MutBytes, NewByteArray, StackByteArray};
use rustix::fd::OwnedFd;
use rustix::fs::{self, AtFlags, FileType, Mode, OFlags};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;

const KEY_NAME: &str = "vault.key";
const MAGIC: &[u8] = b"BUILDSEC\x01";
const KEY_ID_BYTES: usize = 16;
const NONCE_BYTES: usize = 12;
const HEADER_BYTES: usize = MAGIC.len() + KEY_ID_BYTES + NONCE_BYTES;
const TAG_BYTES: usize = 16;
const MAX_FILE_BYTES: usize = HEADER_BYTES + TAG_BYTES + MAX_VALUE_BYTES;
const READ_FLAGS: OFlags = OFlags::RDONLY
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC)
    .union(OFlags::NONBLOCK);

/// Neither the key nor plaintext implements ordinary Debug/serialization at
/// this boundary. The key's backing storage zeroizes on drop.
pub struct Vault {
    directory: File,
    key: StackByteArray<32>,
    key_id: [u8; KEY_ID_BYTES],
}

impl Vault {
    /// A pre-existing private directory never receives a replacement key,
    /// including after an interrupted initialization. Recovery is explicit.
    pub fn open(state_root: &Path, has_records: bool) -> Result<Self> {
        let root = open_state_root(state_root)?;
        let (directory, created) = open_directory(&root, has_records)?;
        let key = if created {
            create_key(&directory)?
        } else {
            load_key(&directory)?
        };
        Ok(Self::from_key(directory, key))
    }

    /// Stable, non-secret identifier for comparing the dedicated key with
    /// durably persisted metadata. It is not an authorization credential.
    pub fn key_id(&self) -> String {
        self.key_id
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    /// Confirm key absence through private, pinned, no-follow directory handles.
    /// Every other filesystem fault is returned, never interpreted as loss.
    pub(super) fn key_is_absent(state_root: &Path) -> Result<bool> {
        let root = match open_state_root(state_root) {
            Ok(root) => root,
            Err(SecretError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(true)
            }
            Err(error) => return Err(error),
        };
        let directory = match open_directory(&root, true) {
            Ok((directory, _)) => directory,
            Err(SecretError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(true)
            }
            Err(error) => return Err(error),
        };
        missing_key(&directory)
    }

    /// Explicit destructive recovery after the grant-retirement barrier.
    ///
    /// The trusted caller MUST durably retire every existing grant before
    /// calling this method, and prevent new admission until it completes.
    /// This filesystem primitive cannot certify the database transaction.
    /// The key must be absent. A present key, including a corrupt one, is never
    /// unlinked or replaced. Strict `open` never invokes recovery. Interrupted
    /// initialization can retry only while the key remains absent.
    pub fn reinitialize_after_retirement(state_root: &Path) -> Result<Self> {
        let candidate = Self::prepare_after_retirement(state_root)?;
        candidate.publish_key()?;
        Ok(candidate)
    }

    /// Trusted caller has durably retired all grants. This creates no key
    /// file: persist this candidate's pending ID before calling publish_key.
    pub(super) fn prepare_after_retirement(state_root: &Path) -> Result<Self> {
        let root = open_state_root(state_root)?;
        let (directory, _) = open_directory(&root, false)?;
        erase_retired_files(&directory)?;
        Ok(Self::from_key(directory, StackByteArray::<32>::gen()))
    }

    pub(super) fn publish_key(&self) -> Result<()> {
        self.publish_key_with_sync(File::sync_all)
    }

    /// The injected directory sync executes after the actual atomic rename.
    /// Production always supplies File::sync_all; tests can fail this boundary.
    pub(super) fn publish_key_with_sync<F>(&self, sync: F) -> Result<()>
    where
        F: FnOnce(&File) -> std::io::Result<()>,
    {
        publish_with_sync(&self.directory, KEY_NAME, self.key.as_slice(), true, sync)
    }

    pub(super) fn sync_key_publication(&self) -> Result<()> {
        self.validate_key()?;
        private_file(&self.directory, KEY_NAME)?.sync_all()?;
        self.directory.sync_all()?;
        Ok(())
    }

    fn from_key(directory: File, key: StackByteArray<32>) -> Self {
        let digest = Sha256::digest(key.as_slice());
        let mut key_id = [0; KEY_ID_BYTES];
        key_id.copy_from_slice(&digest[..KEY_ID_BYTES]);
        Self {
            directory,
            key,
            key_id,
        }
    }

    pub fn write(&self, context: &SecretContext, value: &SecretValue) -> Result<()> {
        context.validate()?;
        self.validate_key()?;
        validate_existing_blob(&self.directory, &context.id)?;
        let cipher = self.cipher()?;
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let mut encrypted = self.header(nonce.as_slice());
        let aad = authenticated_context(context, &encrypted)?;
        let body = cipher
            .encrypt(
                &nonce,
                Payload {
                    msg: value.as_bytes(),
                    aad: &aad,
                },
            )
            .map_err(|_| SecretError::Unavailable)?;
        encrypted.extend_from_slice(&body);
        publish(&self.directory, &context.id, &encrypted, false)
    }

    pub fn read(&self, context: &SecretContext) -> Result<SecretValue> {
        context.validate()?;
        self.validate_key()?;
        let file = private_file(&self.directory, &context.id)?;
        let size = io(fs::fstat(&file))?.st_size;
        if size < (HEADER_BYTES + TAG_BYTES) as i64 || size > MAX_FILE_BYTES as i64 {
            return Err(SecretError::Unavailable);
        }
        // Both the stat bound and the read bound matter: a concurrent file
        // enlargement cannot turn a metadata-size check into unbounded input.
        let mut encrypted = Vec::with_capacity(size as usize);
        file.take((MAX_FILE_BYTES + 1) as u64)
            .read_to_end(&mut encrypted)?;
        self.validate_header(&encrypted)?;
        let nonce = Nonce::from_slice(&encrypted[HEADER_BYTES - NONCE_BYTES..HEADER_BYTES]);
        let aad = authenticated_context(context, &encrypted[..HEADER_BYTES])?;
        let plaintext = self
            .cipher()?
            .decrypt(
                nonce,
                Payload {
                    msg: &encrypted[HEADER_BYTES..],
                    aad: &aad,
                },
            )
            .map_err(|_| SecretError::Unavailable)?;
        SecretValue::new(plaintext)
    }

    pub fn delete(&self, id: &str) -> Result<()> {
        validate_id(id)?;
        self.delete_entry(id)
    }

    /// Does not follow links, even for a damaged/orphaned entry. A directory
    /// cannot be unlinked here; callers retain its tombstone and retry cleanup.
    pub fn delete_entry(&self, name: &str) -> Result<()> {
        validate_cleanup_name(name)?;
        validate_private(&self.directory, FileType::Directory, 0o700)?;
        match fs::unlinkat(&self.directory, name, AtFlags::empty()) {
            Ok(()) => self.directory.sync_all().map_err(SecretError::Io),
            Err(rustix::io::Errno::NOENT) => Ok(()),
            Err(error) => Err(SecretError::Io(error.into())),
        }
    }

    /// Only the vault's generated blob and temporary-file namespaces are
    /// returned. Key files are never candidates for lifecycle deletion.
    pub fn entries(&self) -> Result<Vec<String>> {
        generated_entries(&self.directory)
    }

    fn cipher(&self) -> Result<Aes256Gcm> {
        Aes256Gcm::new_from_slice(self.key.as_slice()).map_err(|_| SecretError::Unavailable)
    }

    fn header(&self, nonce: &[u8]) -> Vec<u8> {
        let mut header = Vec::with_capacity(HEADER_BYTES);
        header.extend_from_slice(MAGIC);
        header.extend_from_slice(&self.key_id);
        header.extend_from_slice(nonce);
        header
    }

    fn validate_header(&self, bytes: &[u8]) -> Result<()> {
        if bytes.len() < HEADER_BYTES + TAG_BYTES || bytes.len() > MAX_FILE_BYTES {
            return Err(SecretError::Unavailable);
        }
        if !bytes.starts_with(MAGIC)
            || bytes[MAGIC.len()..MAGIC.len() + KEY_ID_BYTES] != self.key_id
        {
            return Err(SecretError::Unavailable);
        }
        Ok(())
    }

    fn validate_key(&self) -> Result<()> {
        validate_private(&self.directory, FileType::Directory, 0o700)?;
        // A live handle also fails closed if its key disappears or changes.
        // Never replace a key simply because the cached key is still usable.
        if load_key(&self.directory)?.as_slice() != self.key.as_slice() {
            return Err(SecretError::Unavailable);
        }
        Ok(())
    }
}

fn open_state_root(state_root: &Path) -> Result<OwnedFd> {
    io(fs::open(
        state_root,
        READ_FLAGS | OFlags::DIRECTORY,
        Mode::empty(),
    ))
}

fn create_key(directory: &File) -> Result<StackByteArray<32>> {
    let key = StackByteArray::<32>::gen();
    publish(directory, KEY_NAME, key.as_slice(), true)?;
    Ok(key)
}

fn generated_entries(directory: &File) -> Result<Vec<String>> {
    validate_private(directory, FileType::Directory, 0o700)?;
    let mut entries = io(fs::Dir::read_from(directory))?;
    let mut names = Vec::new();
    while let Some(entry) = entries.read() {
        let entry = io(entry)?;
        if let Ok(name) = entry.file_name().to_str() {
            if validate_cleanup_name(name).is_ok() {
                names.push(name.to_owned());
            }
        }
    }
    Ok(names)
}

fn missing_key(directory: &File) -> Result<bool> {
    match private_file(directory, KEY_NAME) {
        Ok(_) => Ok(false),
        Err(SecretError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(error),
    }
}

fn erase_retired_files(directory: &File) -> Result<()> {
    if !missing_key(directory)? {
        return Err(SecretError::Unavailable);
    }
    for name in generated_entries(directory)? {
        unlink_retired_entry(directory, &name)?;
    }
    // Old blobs must be durably gone before publishing a fresh key. NOREPLACE
    // also refuses a key that appeared concurrently after the absence check.
    directory.sync_all()?;
    Ok(())
}

fn unlink_retired_entry(directory: &File, name: &str) -> Result<()> {
    match fs::unlinkat(directory, name, AtFlags::empty()) {
        Ok(()) | Err(rustix::io::Errno::NOENT) => Ok(()),
        Err(error) => Err(SecretError::Io(error.into())),
    }
}

fn open_directory(root: &OwnedFd, has_records: bool) -> Result<(File, bool)> {
    let created = if has_records {
        false
    } else {
        match fs::mkdirat(root, "secrets", Mode::from_bits_truncate(0o700)) {
            Ok(()) => {
                io(fs::fsync(root))?;
                true
            }
            Err(rustix::io::Errno::EXIST) => false,
            Err(error) => return Err(SecretError::Io(error.into())),
        }
    };
    let directory = File::from(io(fs::openat(
        root,
        "secrets",
        READ_FLAGS | OFlags::DIRECTORY,
        Mode::empty(),
    ))?);
    validate_private(&directory, FileType::Directory, 0o700)?;
    Ok((directory, created))
}

fn load_key(directory: &File) -> Result<StackByteArray<32>> {
    let mut file = private_file(directory, KEY_NAME)?;
    if io(fs::fstat(&file))?.st_size != 32 {
        return Err(SecretError::Unavailable);
    }
    let mut key = StackByteArray::<32>::default();
    file.read_exact(key.as_mut_slice())?;
    Ok(key)
}

fn private_file(directory: &File, name: &str) -> Result<File> {
    let file = File::from(io(fs::openat(directory, name, READ_FLAGS, Mode::empty()))?);
    validate_private(&file, FileType::RegularFile, 0o600)?;
    Ok(file)
}

fn validate_private(file: &File, file_type: FileType, permissions: fs::RawMode) -> Result<()> {
    // SAFETY: geteuid has no arguments or memory preconditions.
    let owner = unsafe { libc::geteuid() };
    validate_private_with_owner(file, file_type, permissions, owner)
}

fn validate_private_with_owner(
    file: &File,
    file_type: FileType,
    permissions: fs::RawMode,
    owner: libc::uid_t,
) -> Result<()> {
    let stat = io(fs::fstat(file))?;
    if FileType::from_raw_mode(stat.st_mode) != file_type
        || stat.st_uid != owner
        || stat.st_mode & 0o7777 != permissions
    {
        return Err(SecretError::Unavailable);
    }
    if file_type == FileType::RegularFile && stat.st_nlink != 1 {
        return Err(SecretError::Unavailable);
    }
    Ok(())
}

fn validate_existing_blob(directory: &File, name: &str) -> Result<()> {
    match private_file(directory, name) {
        Ok(_) => Err(SecretError::Conflict),
        Err(SecretError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn authenticated_context(context: &SecretContext, header: &[u8]) -> Result<Vec<u8>> {
    let mut aad = b"build.device.secret.vault.v1\0".to_vec();
    aad.extend_from_slice(header);
    aad.extend(serde_json::to_vec(context).map_err(|_| SecretError::Invalid)?);
    Ok(aad)
}

/// `bytes` contains only ciphertext (or the dedicated randomly generated key),
/// never a user's plaintext value. Temporary files stay in the private dir.
fn publish(directory: &File, name: &str, bytes: &[u8], key: bool) -> Result<()> {
    publish_with_sync(directory, name, bytes, key, File::sync_all)
}

fn publish_with_sync<F>(
    directory: &File,
    name: &str,
    bytes: &[u8],
    key: bool,
    sync: F,
) -> Result<()>
where
    F: FnOnce(&File) -> std::io::Result<()>,
{
    let prefix = if key { ".vault-key-" } else { ".sec-tmp-" };
    let temporary = format!("{prefix}{}", uuid::Uuid::new_v4());
    let descriptor = io(fs::openat(
        directory,
        temporary.as_str(),
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_bits_truncate(0o600),
    ))?;
    let result = write_and_rename(
        directory,
        File::from(descriptor),
        &temporary,
        name,
        bytes,
        sync,
    );
    if result.is_err() {
        let _ = fs::unlinkat(directory, temporary.as_str(), AtFlags::empty());
    }
    result
}

fn write_and_rename<F>(
    directory: &File,
    mut file: File,
    temporary: &str,
    name: &str,
    bytes: &[u8],
    sync: F,
) -> Result<()>
where
    F: FnOnce(&File) -> std::io::Result<()>,
{
    validate_private(&file, FileType::RegularFile, 0o600)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    // NOREPLACE fences another service instance publishing the same id
    // between our initial existence check and this atomic publication.
    match fs::renameat_with(
        directory,
        temporary,
        directory,
        name,
        fs::RenameFlags::NOREPLACE,
    ) {
        Ok(()) => {}
        Err(rustix::io::Errno::EXIST) => return Err(SecretError::Conflict),
        Err(error) => return Err(SecretError::Io(error.into())),
    }
    sync(directory)?;
    Ok(())
}

fn validate_cleanup_name(name: &str) -> Result<()> {
    if validate_id(name).is_ok() {
        return Ok(());
    }
    let raw = name
        .strip_prefix(".sec-tmp-")
        .or_else(|| name.strip_prefix(".vault-key-"))
        .ok_or(SecretError::Invalid)?;
    let uuid = uuid::Uuid::parse_str(raw).map_err(|_| SecretError::Invalid)?;
    if uuid.hyphenated().to_string() != raw {
        return Err(SecretError::Invalid);
    }
    Ok(())
}

fn io<T>(result: rustix::io::Result<T>) -> Result<T> {
    result.map_err(|error| SecretError::Io(error.into()))
}

#[cfg(test)]
#[path = "vault_tests.rs"]
mod tests;

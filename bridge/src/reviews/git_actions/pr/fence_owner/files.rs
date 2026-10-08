//! Leaf-only cleanup through pinned directory descriptors.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};

pub(super) const MARKER: &str = ".build-fence-owner.json";
const MAX_ENTRIES: usize = 512;
const MAX_CONTENT: u64 = 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub(super) struct Identity {
    device: u64,
    inode: u64,
    mode: u32,
}
impl Identity {
    pub(super) fn of(metadata: &Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            mode: metadata.mode(),
        }
    }
    pub(super) fn file(file: &File) -> Result<Self, String> {
        file.metadata()
            .map(|metadata| Self::of(&metadata))
            .map_err(|error| error.to_string())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub(super) struct Entry {
    name: String,
    pin: String,
    identity: Identity,
    content: Content,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
enum Content {
    File(String),
    Symlink(PathBuf),
}

pub(super) fn at(directory: &File, name: impl AsRef<Path>) -> PathBuf {
    directory_path(directory).join(name)
}

#[cfg(target_os = "linux")]
fn directory_path(directory: &File) -> PathBuf {
    PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd()))
}

#[cfg(target_os = "macos")]
fn directory_path(directory: &File) -> PathBuf {
    use std::os::unix::ffi::OsStrExt;
    let mut buffer = [0_u8; libc::PATH_MAX as usize];
    // SAFETY: F_GETPATH writes at most PATH_MAX bytes into the provided buffer.
    if unsafe { libc::fcntl(directory.as_raw_fd(), libc::F_GETPATH, buffer.as_mut_ptr()) } == 0 {
        let length = buffer
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(buffer.len());
        PathBuf::from(std::ffi::OsStr::from_bytes(&buffer[..length]))
    } else {
        PathBuf::from(format!("/dev/fd/{}", directory.as_raw_fd()))
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn directory_path(directory: &File) -> PathBuf {
    PathBuf::from(format!("/dev/fd/{}", directory.as_raw_fd()))
}

pub(super) fn directory(path: &Path) -> Result<File, String> {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(path)
        .map_err(|error| error.to_string())
}

pub(super) fn regular(path: &Path, create: bool) -> Result<File, String> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(create)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|error| error.to_string())
}

pub(super) fn write_record(file: &File, bytes: &[u8]) -> Result<(), String> {
    let mut file = file;
    if file.metadata().map_err(|error| error.to_string())?.len() + bytes.len() as u64 + 2
        > MAX_CONTENT
    {
        return Err("oversized fence ownership journal".into());
    }
    file.seek(SeekFrom::End(0))
        // A preceding incomplete frame cannot swallow this durable frame.
        .and_then(|_| file.write_all(b"\n"))
        .and_then(|_| file.write_all(bytes))
        .and_then(|_| file.write_all(b"\n"))
        .and_then(|_| file.sync_all())
        .map_err(|error| error.to_string())
}

pub(super) fn read_record(file: &File) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    file.take(MAX_CONTENT + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 > MAX_CONTENT {
        return Err("oversized fence ownership record".into());
    }
    Ok(bytes)
}

pub(super) fn names(directory: &File, limit: usize) -> Result<Vec<String>, String> {
    let mut names = Vec::new();
    for entry in fs::read_dir(at(directory, ""))
        .map_err(|error| error.to_string())?
        .take(limit + 1)
    {
        let name = entry
            .map_err(|error| error.to_string())?
            .file_name()
            .into_string()
            .map_err(|_| "hook entry name is not UTF-8".to_string())?;
        names.push(name);
    }
    if names.len() > limit {
        return Err("fence registry exceeds its bounded capacity".into());
    }
    names.sort();
    Ok(names)
}

pub(super) fn snapshot(
    directory: &File,
    registry: &File,
    token: &str,
) -> Result<Vec<Entry>, String> {
    let mut entries = Vec::new();
    for name in names(directory, MAX_ENTRIES)? {
        if name == MARKER {
            continue;
        }
        let path = at(directory, &name);
        let metadata = fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
        let pin = format!(".pin-{token}-{}", entries.len());
        fs::hard_link(&path, at(registry, &pin)).map_err(|error| error.to_string())?;
        let entry = Entry {
            name,
            pin,
            identity: Identity::of(&metadata),
            content: content(&path, &metadata)?,
        };
        verify_entry(directory, registry, &entry)?;
        entries.push(entry);
    }
    registry.sync_all().map_err(|error| error.to_string())?;
    Ok(entries)
}

fn content(path: &Path, metadata: &Metadata) -> Result<Content, String> {
    if metadata.file_type().is_symlink() {
        return fs::read_link(path)
            .map(Content::Symlink)
            .map_err(|error| error.to_string());
    }
    if !metadata.is_file() {
        return Err("unregistered directory or special file in PR hooks".into());
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|error| error.to_string())?;
    if Identity::file(&file)? != Identity::of(metadata) {
        return Err("hook file changed while registering".into());
    }
    Ok(Content::File(format!(
        "{:x}",
        Sha256::digest(read_record(&file)?)
    )))
}

fn leaf(name: &str) -> Result<(), String> {
    let mut components = Path::new(name).components();
    if matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none() {
        Ok(())
    } else {
        Err("invalid owned hook entry name".into())
    }
}

fn verify_path(path: &Path, entry: &Entry) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if Identity::of(&metadata) != entry.identity || content(path, &metadata)? != entry.content {
        return Err("PR hook ownership changed; preserving replacement".into());
    }
    Ok(())
}

fn verify_entry(directory: &File, registry: &File, entry: &Entry) -> Result<(), String> {
    leaf(&entry.name)?;
    leaf(&entry.pin)?;
    verify_path(&at(registry, &entry.pin), entry)?;
    let path = at(directory, &entry.name);
    if path.try_exists().map_err(|error| error.to_string())? || fs::symlink_metadata(&path).is_ok()
    {
        verify_path(&path, entry)?;
    }
    Ok(())
}

pub(super) fn verify_entries(
    directory: &File,
    registry: &File,
    entries: &[Entry],
) -> Result<(), String> {
    for name in names(directory, MAX_ENTRIES)? {
        if name != MARKER && !entries.iter().any(|entry| entry.name == name) {
            return Err("unknown PR hook entry; preserving the fence".into());
        }
    }
    for entry in entries {
        verify_entry(directory, registry, entry)?;
    }
    Ok(())
}

pub(super) fn unlink(directory: &File, name: &str, directory_entry: bool) -> Result<(), String> {
    leaf(name)?;
    let name = std::ffi::CString::new(name).map_err(|error| error.to_string())?;
    let flags = if directory_entry {
        libc::AT_REMOVEDIR
    } else {
        0
    };
    // SAFETY: the name is one validated leaf below the pinned directory FD.
    if unsafe { libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), flags) } == 0 {
        return Ok(());
    }
    let error = std::io::Error::last_os_error();
    if error.kind() == std::io::ErrorKind::NotFound {
        return Ok(());
    }
    Err(error.to_string())
}

pub(super) fn remove_entries(
    directory: &File,
    registry: &File,
    entries: &[Entry],
) -> Result<(), String> {
    for entry in entries {
        verify_entry(directory, registry, entry)?;
        unlink(directory, &entry.name, false)?;
    }
    directory.sync_all().map_err(|error| error.to_string())
}

pub(super) fn remove_pins(registry: &File, entries: &[Entry]) -> Result<(), String> {
    for entry in entries {
        verify_path(&at(registry, &entry.pin), entry)?;
        unlink(registry, &entry.pin, false)?;
    }
    registry.sync_all().map_err(|error| error.to_string())
}

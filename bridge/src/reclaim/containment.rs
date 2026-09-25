//! The filesystem boundary for a managed workspace. The storage directory is
//! fixed when the registry opens; a manifest cannot redefine that boundary.
//! Destructive walks use directory descriptors and never traverse a link.

use super::artifacts::Artifact;
use super::Budget;
use std::collections::HashMap;
use std::ffi::{CString, OsStr, OsString};
use std::fs::Metadata;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FileId {
    device: u64,
    inode: u64,
}

impl FileId {
    fn of(metadata: &Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
        }
    }
}

/// The storage directory the registry was configured to own. Its inode is
/// captured as soon as it exists, including when a new registry creates it.
#[derive(Clone, Debug)]
pub struct StorageAnchor {
    raw: PathBuf,
    canonical: PathBuf,
    identity: Arc<OnceLock<FileId>>,
    workspace_identities: Arc<Mutex<HashMap<PathBuf, FileId>>>,
}

impl StorageAnchor {
    pub fn new(root: &Path) -> Self {
        let anchor = Self {
            raw: root.to_path_buf(),
            canonical: crate::worktree::canonical_planned_path(root),
            identity: Arc::new(OnceLock::new()),
            workspace_identities: Arc::new(Mutex::new(HashMap::new())),
        };
        anchor.capture();
        anchor
    }

    pub fn capture(&self) {
        if let Ok(metadata) = std::fs::metadata(&self.canonical) {
            if metadata.is_dir() {
                let _ = self.identity.set(FileId::of(&metadata));
            }
        }
    }

    /// Remember an existing registered root once. Reloading the registry
    /// must never bless a different directory placed at the same path.
    pub fn capture_workspace(&self, root: &Path) -> io::Result<()> {
        let (expected, identity) = self.workspace_identity_on_disk(root)?;
        self.workspace_identities
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .entry(expected)
            .or_insert(identity);
        Ok(())
    }

    /// A newly created root may reuse the name of a workspace deleted earlier
    /// in this process. Creation is the one point that installs its new inode.
    pub fn register_workspace(&self, root: &Path) -> io::Result<()> {
        let (expected, identity) = self.workspace_identity_on_disk(root)?;
        self.workspace_identities
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(expected, identity);
        Ok(())
    }

    fn workspace_identity_on_disk(&self, root: &Path) -> io::Result<(PathBuf, FileId)> {
        let relative = root.strip_prefix(&self.raw).map_err(|_| refused())?;
        if relative.components().count() != 2 || !normal_components(relative) {
            return Err(refused());
        }
        let expected = self.canonical.join(relative);
        let metadata = std::fs::symlink_metadata(&expected)?;
        if !metadata.is_dir() {
            return Err(refused());
        }
        Ok((expected, FileId::of(&metadata)))
    }

    fn registered_workspace_identity(&self, root: &Path) -> Option<FileId> {
        self.workspace_identities
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(root)
            .copied()
    }
}

#[derive(Clone, Debug)]
pub struct WorkspaceBoundary {
    anchor: StorageAnchor,
    root: PathBuf,
    checkouts: Vec<PathBuf>,
    expected_root: PathBuf,
    root_identity: FileId,
}

impl WorkspaceBoundary {
    /// `root` and `checkouts` come from a registered manifest. The expected
    /// location is rebuilt from the registry path, not from their resolution.
    pub fn new(anchor: StorageAnchor, root: &Path, checkouts: Vec<PathBuf>) -> Option<Self> {
        let relative = root.strip_prefix(&anchor.raw).ok()?;
        if relative.components().count() != 2 || !normal_components(relative) {
            return None;
        }
        let expected_root = anchor.canonical.join(relative);
        if checkouts.iter().any(|path| {
            path.strip_prefix(root).map_or(true, |relative| {
                relative.as_os_str().is_empty() || !normal_components(relative)
            })
        }) {
            return None;
        }
        let root_identity = anchor.registered_workspace_identity(&expected_root)?;
        Some(Self {
            anchor,
            root: root.to_path_buf(),
            checkouts,
            expected_root,
            root_identity,
        })
    }

    pub fn expected_root(&self) -> &Path {
        &self.expected_root
    }

    pub fn validate(&self) -> io::Result<ValidatedBoundary> {
        self.open(false)
    }

    /// A partly completed explicit removal may already have removed a
    /// checkout. Every checkout still present must remain within this root.
    pub fn validate_removal(&self) -> io::Result<ValidatedBoundary> {
        self.open(true)
    }

    fn open(&self, missing_checkouts_allowed: bool) -> io::Result<ValidatedBoundary> {
        let storage = open_absolute(&self.anchor.canonical)?;
        let actual = file_id(storage.as_raw_fd())?;
        if self
            .anchor
            .identity
            .get()
            .is_none_or(|expected| *expected != actual)
        {
            return Err(refused());
        }
        let relative = self
            .expected_root
            .strip_prefix(&self.anchor.canonical)
            .map_err(|_| refused())?;
        let project_name = relative
            .components()
            .next()
            .ok_or_else(refused)?
            .as_os_str();
        let root_name = relative
            .components()
            .nth(1)
            .ok_or_else(refused)?
            .as_os_str();
        let project = open_child(storage.as_raw_fd(), project_name)?;
        let root = open_child(project.as_raw_fd(), root_name)?;
        if self.root_identity != file_id(root.as_raw_fd())? {
            return Err(refused());
        }
        if std::fs::canonicalize(&self.root)? != self.expected_root {
            return Err(refused());
        }
        for checkout in &self.checkouts {
            let relative = checkout.strip_prefix(&self.root).map_err(|_| refused())?;
            match open_relative(root.as_raw_fd(), relative) {
                Ok(directory) => {
                    let expected = self.expected_root.join(relative);
                    if std::fs::canonicalize(checkout)? != expected
                        || file_id(directory.as_raw_fd())?
                            != FileId::of(&std::fs::metadata(checkout)?)
                    {
                        return Err(refused());
                    }
                }
                Err(error)
                    if missing_checkouts_allowed && error.kind() == io::ErrorKind::NotFound =>
                {
                    if std::fs::symlink_metadata(checkout).is_ok() {
                        return Err(refused());
                    }
                }
                Err(error) => return Err(error),
            }
        }
        Ok(ValidatedBoundary {
            storage,
            root,
            project,
            root_name: root_name.to_os_string(),
            expected_storage: self.anchor.canonical.clone(),
            expected_root: self.expected_root.clone(),
        })
    }
}

pub struct ValidatedBoundary {
    storage: OwnedFd,
    root: OwnedFd,
    project: OwnedFd,
    root_name: OsString,
    expected_storage: PathBuf,
    expected_root: PathBuf,
}

impl ValidatedBoundary {
    pub fn expected_root(&self) -> &Path {
        &self.expected_root
    }

    pub fn open_checkout(&self, path: &Path) -> io::Result<OwnedFd> {
        self.ensure_current()?;
        let relative = path
            .strip_prefix(&self.expected_root)
            .map_err(|_| refused())?;
        if relative.as_os_str().is_empty() {
            return Err(refused());
        }
        open_relative(self.root.as_raw_fd(), relative)
    }

    /// Activity is walked from the held root, so a renamed or linked path
    /// cannot make a sweep read an unrelated tree.
    pub fn newest_change_ms(&self, budget: &Budget) -> Result<Option<i64>, super::Unfinished> {
        let mut pending = vec![PathBuf::new()];
        let mut newest = None;
        while let Some(relative) = pending.pop() {
            let directory =
                open_relative(self.root.as_raw_fd(), &relative).map_err(|_| super::Unfinished)?;
            for name in
                directory_names(directory.as_raw_fd(), budget).map_err(|_| super::Unfinished)?
            {
                if skipped_for_activity(&name, relative.as_os_str().is_empty()) {
                    continue;
                }
                let stat =
                    child_stat(directory.as_raw_fd(), &name).map_err(|_| super::Unfinished)?;
                if stat.st_mode & libc::S_IFMT == libc::S_IFDIR {
                    pending.push(relative.join(&name));
                } else if stat.st_mode & libc::S_IFMT == libc::S_IFREG {
                    let file =
                        open_file(directory.as_raw_fd(), &name).map_err(|_| super::Unfinished)?;
                    let metadata = std::fs::File::from(file)
                        .metadata()
                        .map_err(|_| super::Unfinished)?;
                    if !metadata.is_file() {
                        return Err(super::Unfinished);
                    }
                    let changed = metadata.modified().map_err(|_| super::Unfinished)?;
                    newest = newest.max(Some(changed));
                }
            }
        }
        Ok(newest.map(system_ms))
    }

    pub fn size_on_disk(&self, budget: &Budget) -> Result<u64, super::Unfinished> {
        let mut pending = vec![PathBuf::new()];
        let mut total = 0u64;
        while let Some(relative) = pending.pop() {
            let directory =
                open_relative(self.root.as_raw_fd(), &relative).map_err(|_| super::Unfinished)?;
            budget.spend()?;
            total = total
                .saturating_add(fd_blocks(directory.as_raw_fd()).map_err(|_| super::Unfinished)?);
            for name in
                directory_names(directory.as_raw_fd(), budget).map_err(|_| super::Unfinished)?
            {
                let stat =
                    child_stat(directory.as_raw_fd(), &name).map_err(|_| super::Unfinished)?;
                if stat.st_mode & libc::S_IFMT == libc::S_IFDIR {
                    pending.push(relative.join(&name));
                } else {
                    total = total.saturating_add((stat.st_blocks as u64).saturating_mul(512));
                }
            }
        }
        Ok(total)
    }

    pub fn manifest_created_ms(&self) -> Result<i64, super::Unfinished> {
        let file = open_file(
            self.root.as_raw_fd(),
            OsStr::new(crate::workspace::MANIFEST_FILE),
        )
        .map_err(|_| super::Unfinished)?;
        let metadata = std::fs::File::from(file)
            .metadata()
            .map_err(|_| super::Unfinished)?;
        if !metadata.is_file() {
            return Err(super::Unfinished);
        }
        metadata
            .created()
            .or_else(|_| metadata.modified())
            .map(system_ms)
            .map_err(|_| super::Unfinished)
    }

    pub fn move_to_trash(&self, artifacts: &[Artifact]) -> Vec<Artifact> {
        if self.ensure_current().is_err() {
            return Vec::new();
        }
        let Ok(trash) = self.trash(true) else {
            return Vec::new();
        };
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|since| since.as_nanos())
            .unwrap_or(0);
        artifacts
            .iter()
            .enumerate()
            .filter_map(|(index, artifact)| {
                self.ensure_current().ok()?;
                let relative = artifact.path.strip_prefix(&self.expected_root).ok()?;
                let parent = open_relative(self.root.as_raw_fd(), relative.parent()?).ok()?;
                let name = relative.file_name()?;
                let directory = open_child(parent.as_raw_fd(), name).ok()?;
                if !file_id(directory.as_raw_fd()).ok().is_some_and(|identity| {
                    std::fs::symlink_metadata(&artifact.path)
                        .ok()
                        .is_some_and(|metadata| {
                            metadata.is_dir() && FileId::of(&metadata) == identity
                        })
                }) {
                    return None;
                }
                let target = format!("{stamp}-{index}-{}", name.to_string_lossy());
                rename_child(
                    parent.as_raw_fd(),
                    name,
                    trash.as_raw_fd(),
                    OsStr::new(&target),
                )
                .ok()
                .map(|()| artifact.clone())
            })
            .collect()
    }

    pub fn empty_trash(&self, budget: &Budget) -> u64 {
        if self.ensure_current().is_err() {
            return 0;
        }
        let Ok(trash) = self.trash(false) else {
            return 0;
        };
        let Ok(names) = directory_names(trash.as_raw_fd(), budget) else {
            return 0;
        };
        let mut freed = 0;
        for name in names {
            if budget.check().is_err() || self.ensure_current().is_err() {
                break;
            }
            let Ok(bytes) = remove_child(trash.as_raw_fd(), &name, budget, 0) else {
                break;
            };
            freed += bytes;
        }
        freed
    }

    /// Remove the tree while leaving its manifest discoverable if cleanup
    /// stops. Metadata unregistering runs before the final manifest unlink.
    pub fn remove_contents_preserving_manifest(&self, budget: &Budget) -> io::Result<()> {
        self.ensure_current()?;
        for name in directory_names(self.root.as_raw_fd(), budget)? {
            if name.as_os_str() == OsStr::new(crate::workspace::MANIFEST_FILE) {
                continue;
            }
            self.ensure_current()?;
            remove_child(self.root.as_raw_fd(), &name, budget, 0)?;
        }
        Ok(())
    }

    pub fn finish_remove_workspace(&self, budget: &Budget) -> io::Result<()> {
        budget.check().map_err(|_| refused())?;
        self.ensure_current()?;
        let manifest = OsStr::new(crate::workspace::MANIFEST_FILE);
        match unlink_child(self.root.as_raw_fd(), manifest, false) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        self.ensure_current()?;
        unlink_child(self.project.as_raw_fd(), &self.root_name, true)
    }

    pub fn remove_workspace(&self, budget: &Budget) -> io::Result<()> {
        self.remove_contents_preserving_manifest(budget)?;
        self.finish_remove_workspace(budget)
    }

    fn ensure_current(&self) -> io::Result<()> {
        for (expected, pinned) in [
            (self.expected_storage.as_path(), self.storage.as_raw_fd()),
            (
                self.expected_root.parent().ok_or_else(refused)?,
                self.project.as_raw_fd(),
            ),
            (self.expected_root.as_path(), self.root.as_raw_fd()),
        ] {
            let current = open_absolute(expected)?;
            if file_id(current.as_raw_fd())? != file_id(pinned)? {
                return Err(refused());
            }
        }
        Ok(())
    }

    fn trash(&self, create: bool) -> io::Result<OwnedFd> {
        let build = child_directory(self.root.as_raw_fd(), OsStr::new(".build"), create)?;
        child_directory(build.as_raw_fd(), OsStr::new("reclaim"), create)
    }
}

fn normal_components(path: &Path) -> bool {
    path.components()
        .all(|part| matches!(part, Component::Normal(_)))
}

fn refused() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "managed workspace boundary changed",
    )
}

fn name_c(name: &OsStr) -> io::Result<CString> {
    CString::new(name.as_bytes()).map_err(|_| refused())
}

fn open_absolute(path: &Path) -> io::Result<OwnedFd> {
    if !path.is_absolute() {
        return Err(refused());
    }
    let mut directory = open_raw(libc::AT_FDCWD, OsStr::new("/"))?;
    for part in path.components() {
        if let Component::Normal(name) = part {
            directory = open_child(directory.as_raw_fd(), name)?;
        }
    }
    Ok(directory)
}

fn open_relative(parent: RawFd, path: &Path) -> io::Result<OwnedFd> {
    if !normal_components(path) {
        return Err(refused());
    }
    let duplicate = unsafe { libc::dup(parent) };
    if duplicate < 0 {
        return Err(io::Error::last_os_error());
    }
    let mut directory = unsafe { OwnedFd::from_raw_fd(duplicate) };
    for part in path.components() {
        directory = open_child(directory.as_raw_fd(), part.as_os_str())?;
    }
    Ok(directory)
}

fn open_child(parent: RawFd, name: &OsStr) -> io::Result<OwnedFd> {
    open_raw(parent, name)
}

fn open_raw(parent: RawFd, name: &OsStr) -> io::Result<OwnedFd> {
    let name = name_c(name)?;
    let fd = unsafe {
        libc::openat(
            parent,
            name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }
}

fn open_file(parent: RawFd, name: &OsStr) -> io::Result<OwnedFd> {
    let name = name_c(name)?;
    let fd = unsafe {
        libc::openat(
            parent,
            name.as_ptr(),
            libc::O_RDONLY | libc::O_NONBLOCK | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }
}

fn child_stat(parent: RawFd, name: &OsStr) -> io::Result<libc::stat> {
    let name = name_c(name)?;
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    if unsafe {
        libc::fstatat(
            parent,
            name.as_ptr(),
            stat.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } < 0
    {
        Err(io::Error::last_os_error())
    } else {
        Ok(unsafe { stat.assume_init() })
    }
}

fn fd_blocks(fd: RawFd) -> io::Result<u64> {
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    if unsafe { libc::fstat(fd, stat.as_mut_ptr()) } < 0 {
        return Err(io::Error::last_os_error());
    }
    let stat = unsafe { stat.assume_init() };
    Ok((stat.st_blocks as u64).saturating_mul(512))
}

fn skipped_for_activity(name: &OsStr, is_root: bool) -> bool {
    let Some(name) = name.to_str() else {
        return false;
    };
    name == ".git"
        || name == ".build"
        || super::artifacts::ARTIFACT_DIRS.contains(&name)
        || (is_root && name == crate::workspace::MANIFEST_FILE)
}

fn system_ms(at: SystemTime) -> i64 {
    at.duration_since(UNIX_EPOCH)
        .map(|since| i64::try_from(since.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

fn file_id(fd: RawFd) -> io::Result<FileId> {
    // Clone the borrowed descriptor to use Metadata's portable device/inode
    // accessors without taking ownership from the pinned boundary.
    let borrowed = unsafe { std::os::fd::BorrowedFd::borrow_raw(fd) };
    let file = std::fs::File::from(borrowed.try_clone_to_owned()?);
    file.metadata().map(|metadata| FileId::of(&metadata))
}

fn child_directory(parent: RawFd, name: &OsStr, create: bool) -> io::Result<OwnedFd> {
    match open_child(parent, name) {
        Ok(directory) => Ok(directory),
        Err(error) if create && error.kind() == io::ErrorKind::NotFound => {
            let name = name_c(name)?;
            if unsafe { libc::mkdirat(parent, name.as_ptr(), 0o700) } < 0 {
                return Err(io::Error::last_os_error());
            }
            open_child(parent, OsStr::from_bytes(name.as_bytes()))
        }
        Err(error) => Err(error),
    }
}

fn rename_child(from: RawFd, name: &OsStr, to: RawFd, target: &OsStr) -> io::Result<()> {
    let name = name_c(name)?;
    let target = name_c(target)?;
    if unsafe { libc::renameat(from, name.as_ptr(), to, target.as_ptr()) } < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn unlink_child(parent: RawFd, name: &OsStr, directory: bool) -> io::Result<()> {
    let name = name_c(name)?;
    let flags = if directory { libc::AT_REMOVEDIR } else { 0 };
    if unsafe { libc::unlinkat(parent, name.as_ptr(), flags) } < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn directory_names(fd: RawFd, budget: &Budget) -> io::Result<Vec<OsString>> {
    let fresh = open_child(fd, OsStr::new("."))?;
    let raw = fresh.into_raw_fd();
    let stream = unsafe { libc::fdopendir(raw) };
    if stream.is_null() {
        unsafe { libc::close(raw) };
        return Err(io::Error::last_os_error());
    }
    let mut names = Vec::new();
    loop {
        unsafe { *errno_ptr() = 0 };
        let entry = unsafe { libc::readdir(stream) };
        if entry.is_null() {
            let error = unsafe { *errno_ptr() };
            if error != 0 {
                unsafe { libc::closedir(stream) };
                return Err(io::Error::from_raw_os_error(error));
            }
            break;
        }
        if budget.spend().is_err() {
            unsafe { libc::closedir(stream) };
            return Err(refused());
        }
        let name = unsafe { std::ffi::CStr::from_ptr((*entry).d_name.as_ptr()) };
        if name.to_bytes() != b"." && name.to_bytes() != b".." {
            names.push(OsString::from_vec(name.to_bytes().to_vec()));
        }
    }
    unsafe { libc::closedir(stream) };
    Ok(names)
}

#[cfg(target_os = "linux")]
fn errno_ptr() -> *mut libc::c_int {
    unsafe { libc::__errno_location() }
}

#[cfg(not(target_os = "linux"))]
fn errno_ptr() -> *mut libc::c_int {
    unsafe { libc::__error() }
}

fn remove_child(parent: RawFd, name: &OsStr, budget: &Budget, depth: usize) -> io::Result<u64> {
    if depth >= 256 {
        return Err(refused());
    }
    budget.spend().map_err(|_| refused())?;
    let name_c = name_c(name)?;
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    if unsafe {
        libc::fstatat(
            parent,
            name_c.as_ptr(),
            stat.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } < 0
    {
        return Err(io::Error::last_os_error());
    }
    let stat = unsafe { stat.assume_init() };
    let bytes = (stat.st_blocks as u64).saturating_mul(512);
    if stat.st_mode & libc::S_IFMT != libc::S_IFDIR {
        unlink_child(parent, name, false)?;
        return Ok(bytes);
    }
    let directory = open_child(parent, name)?;
    let mut freed = bytes;
    for child in directory_names(directory.as_raw_fd(), budget)? {
        freed += remove_child(directory.as_raw_fd(), &child, budget, depth + 1)?;
    }
    unlink_child(parent, name, true)?;
    Ok(freed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn fixture() -> (tempfile::TempDir, StorageAnchor, PathBuf, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let storage = tmp.path().join("managed");
        let root = storage.join("project/workspace");
        let checkout = root.join("repo");
        std::fs::create_dir_all(&checkout).unwrap();
        let anchor = StorageAnchor::new(&storage);
        anchor.capture_workspace(&root).unwrap();
        (tmp, anchor, root, checkout)
    }

    fn budget() -> Budget {
        Budget::new(
            100_000,
            std::time::Duration::from_secs(60),
            Default::default(),
        )
    }

    #[test]
    fn refuses_workspace_root_replaced_by_a_link() {
        let (tmp, anchor, root, checkout) = fixture();
        let boundary = WorkspaceBoundary::new(anchor, &root, vec![checkout]).unwrap();
        let outside = tmp.path().join("outside");
        std::fs::rename(&root, &outside).unwrap();
        symlink(&outside, &root).unwrap();
        assert!(boundary.validate().is_err());
        assert!(outside.join("repo").is_dir());
    }

    #[test]
    fn refuses_storage_or_project_replaced_by_a_link() {
        for level in ["managed", "managed/project"] {
            let (tmp, anchor, root, checkout) = fixture();
            let boundary = WorkspaceBoundary::new(anchor, &root, vec![checkout]).unwrap();
            let replaced = tmp.path().join(level);
            let outside = tmp.path().join("outside");
            std::fs::rename(&replaced, &outside).unwrap();
            symlink(&outside, &replaced).unwrap();
            assert!(boundary.validate().is_err(), "{level}");
            assert!(outside
                .join(if level == "managed" {
                    "project/workspace/repo"
                } else {
                    "workspace/repo"
                })
                .is_dir());
        }
    }

    #[test]
    fn refuses_checkout_replaced_by_a_link_before_work_starts() {
        let (tmp, anchor, root, checkout) = fixture();
        let boundary = WorkspaceBoundary::new(anchor, &root, vec![checkout.clone()]).unwrap();
        let outside = tmp.path().join("outside");
        std::fs::rename(&checkout, &outside).unwrap();
        symlink(&outside, &checkout).unwrap();
        assert!(boundary.validate().is_err());
    }

    #[test]
    fn held_guard_refuses_cleanup_after_root_is_substituted() {
        let (tmp, anchor, root, checkout) = fixture();
        let boundary = WorkspaceBoundary::new(anchor, &root, vec![checkout]).unwrap();
        let guard = boundary.validate().unwrap();
        let trash = root.join(".build/reclaim");
        std::fs::create_dir_all(&trash).unwrap();
        std::fs::write(trash.join("keep"), b"keep").unwrap();
        let outside = tmp.path().join("outside");
        std::fs::rename(&root, &outside).unwrap();
        symlink(&outside, &root).unwrap();

        assert_eq!(guard.empty_trash(&budget()), 0);
        assert!(guard.remove_workspace(&budget()).is_err());
        assert!(outside.join(".build/reclaim/keep").exists());
    }

    #[test]
    fn held_guard_refuses_cleanup_after_an_ancestor_is_substituted() {
        for level in ["managed", "managed/project"] {
            let (tmp, anchor, root, checkout) = fixture();
            let boundary = WorkspaceBoundary::new(anchor, &root, vec![checkout]).unwrap();
            let guard = boundary.validate().unwrap();
            let trash = root.join(".build/reclaim");
            std::fs::create_dir_all(&trash).unwrap();
            std::fs::write(trash.join("keep"), b"keep").unwrap();
            let replaced = tmp.path().join(level);
            let outside = tmp.path().join("outside");
            std::fs::rename(&replaced, &outside).unwrap();
            symlink(&outside, &replaced).unwrap();

            assert_eq!(guard.empty_trash(&budget()), 0, "{level}");
            assert!(guard.remove_workspace(&budget()).is_err(), "{level}");
            assert!(outside
                .join(if level == "managed" {
                    "project/workspace/.build/reclaim/keep"
                } else {
                    "workspace/.build/reclaim/keep"
                })
                .exists());
        }
    }

    #[test]
    fn linked_trash_is_never_followed() {
        let (tmp, anchor, root, checkout) = fixture();
        let boundary = WorkspaceBoundary::new(anchor, &root, vec![checkout]).unwrap();
        let guard = boundary.validate().unwrap();
        let outside = tmp.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("keep"), b"keep").unwrap();
        std::fs::create_dir(root.join(".build")).unwrap();
        symlink(&outside, root.join(".build/reclaim")).unwrap();

        assert_eq!(guard.empty_trash(&budget()), 0);
        assert!(outside.join("keep").exists());
    }

    #[test]
    fn measurement_stays_on_the_pinned_root_after_a_path_swap() {
        let (tmp, anchor, root, checkout) = fixture();
        std::fs::write(root.join(crate::workspace::MANIFEST_FILE), b"{}").unwrap();
        std::fs::write(checkout.join("source.txt"), b"source").unwrap();
        let boundary = WorkspaceBoundary::new(anchor, &root, vec![checkout]).unwrap();
        let guard = boundary.validate().unwrap();
        let original_size = guard.size_on_disk(&budget()).unwrap();
        let original_activity = guard.newest_change_ms(&budget()).unwrap();
        let saved = tmp.path().join("saved");
        std::fs::rename(&root, &saved).unwrap();
        let other = tmp.path().join("other");
        std::fs::create_dir(&other).unwrap();
        std::fs::write(other.join("large"), vec![9u8; 128 * 1024]).unwrap();
        symlink(&other, &root).unwrap();

        assert_eq!(guard.size_on_disk(&budget()), Ok(original_size));
        assert_eq!(
            guard.newest_change_ms(&budget()).unwrap(),
            original_activity
        );
        assert!(boundary.validate().is_err());
    }

    #[test]
    fn descriptor_removal_unlinks_a_child_link_without_visiting_its_target() {
        let (tmp, anchor, root, checkout) = fixture();
        let outside = tmp.path().join("other");
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("keep"), b"keep").unwrap();
        symlink(&outside, checkout.join("linked")).unwrap();
        let boundary = WorkspaceBoundary::new(anchor, &root, vec![checkout]).unwrap();
        let guard = boundary.validate().unwrap();

        guard.remove_workspace(&budget()).unwrap();

        assert!(!root.exists());
        assert!(outside.join("keep").exists());
    }

    #[test]
    fn failed_recursive_cleanup_leaves_the_manifest_for_retry() {
        let (_tmp, anchor, root, checkout) = fixture();
        std::fs::write(root.join(crate::workspace::MANIFEST_FILE), b"{}").unwrap();
        let mut nested = checkout.clone();
        for _ in 0..257 {
            nested.push("d");
            std::fs::create_dir(&nested).unwrap();
        }
        let boundary = WorkspaceBoundary::new(anchor, &root, vec![checkout]).unwrap();
        let guard = boundary.validate().unwrap();

        assert!(guard
            .remove_contents_preserving_manifest(&budget())
            .is_err());
        assert!(root.join(crate::workspace::MANIFEST_FILE).is_file());
    }
}

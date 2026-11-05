//! Opt-in directory keysets. Scans are linear, but retained rows are bounded.
use super::{errors, fenced_scope_path};
use crate::api::ApiError;
use aws_lc_rs::hmac;
use aws_lc_rs::rand::{SecureRandom, SystemRandom};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BinaryHeap};
use std::fs::{DirEntry, File, Metadata};
use std::path::Path;
use std::sync::{Arc, Condvar, LazyLock, Mutex, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const DEFAULT_LIMIT: usize = 100;
const MAX_LIMIT: usize = 200;
const MAX_CURSOR_BYTES: usize = 8192;
const MAX_LEASES: usize = 32;
const LEASE_TTL: Duration = Duration::from_secs(120);
static LEASES: LazyLock<TimedLeaseRegistry> =
    LazyLock::new(|| TimedLeaseRegistry::new(MAX_LEASES, LEASE_TTL));
// A restarted bridge refuses its predecessor's directory cursors. Cached
// entries still paint; opening the directory obtains a fresh first page.
static CURSOR_KEY: LazyLock<hmac::Key> = LazyLock::new(|| {
    let mut secret = [0_u8; 32];
    SystemRandom::new()
        .fill(&mut secret)
        .expect("OS randomness for directory cursors");
    hmac::Key::new(hmac::HMAC_SHA256, &secret)
});

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
struct SortKey(u8, String, Vec<u8>);

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    version: u8,
    binding: String,
    after: SortKey,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    lease: Option<u64>,
}

struct DirectoryLease {
    handle: Arc<File>,
    expires: Instant,
}

struct LeaseRegistry {
    entries: BTreeMap<u64, DirectoryLease>,
    next_id: u64,
    capacity: usize,
    ttl: Duration,
    cleanup: Option<Weak<LeaseCleanup>>,
    stopped: bool,
}

struct TimedLeaseRegistry {
    registry: Arc<Mutex<LeaseRegistry>>,
    cleanup: Arc<LeaseCleanup>,
}

impl TimedLeaseRegistry {
    fn new(capacity: usize, ttl: Duration) -> Self {
        let registry = Arc::new(Mutex::new(LeaseRegistry::new(capacity, ttl)));
        let cleanup = Arc::new(LeaseCleanup {
            registry: Arc::downgrade(&registry),
            wake: Condvar::new(),
            worker: Mutex::new(None),
        });
        registry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .cleanup = Some(Arc::downgrade(&cleanup));
        Self { registry, cleanup }
    }
}

impl Drop for TimedLeaseRegistry {
    fn drop(&mut self) {
        {
            let mut registry = self
                .registry
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            registry.stopped = true;
            registry.entries.clear();
            self.cleanup.wake.notify_all();
        }
        let worker = self
            .cleanup
            .worker
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(worker) = worker {
            let _ = worker.join();
        }
    }
}

struct LeaseCleanup {
    registry: Weak<Mutex<LeaseRegistry>>,
    wake: Condvar,
    worker: Mutex<Option<JoinHandle<()>>>,
}

fn cleanup_unavailable() -> String {
    errors::encode(ApiError::unavailable(
        "Directory lease cleanup is unavailable.",
    ))
}

impl LeaseCleanup {
    fn start(self: &Arc<Self>) -> Result<(), String> {
        let mut worker = self
            .worker
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if worker.is_none() {
            let cleanup = Arc::clone(self);
            *worker = Some(
                std::thread::Builder::new()
                    .name("directory-lease-expiry".into())
                    .spawn(move || cleanup.run())
                    .map_err(|_| cleanup_unavailable())?,
            );
        }
        Ok(())
    }

    fn run(&self) {
        let Some(registry) = self.registry.upgrade() else {
            return;
        };
        let mut held = registry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while !held.stopped {
            if held.expire(Instant::now()) {
                self.wake.notify_all();
            }
            held = match held.entries.values().map(|lease| lease.expires).min() {
                Some(deadline) => {
                    self.wake
                        .wait_timeout(held, deadline.saturating_duration_since(Instant::now()))
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .0
                }
                None => self
                    .wake
                    .wait(held)
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
            };
        }
    }
}

impl LeaseRegistry {
    fn new(capacity: usize, ttl: Duration) -> Self {
        assert!(capacity > 0);
        Self {
            entries: BTreeMap::new(),
            next_id: 0,
            capacity,
            ttl,
            cleanup: None,
            stopped: false,
        }
    }

    fn expire(&mut self, now: Instant) -> bool {
        let before = self.entries.len();
        self.entries.retain(|_, lease| lease.expires > now);
        before != self.entries.len()
    }

    fn insert(&mut self, handle: Arc<File>, now: Instant) -> Result<u64, String> {
        if self.stopped {
            return Err(cleanup_unavailable());
        }
        let cleanup = self
            .cleanup
            .as_ref()
            .map(|cleanup| cleanup.upgrade().ok_or_else(cleanup_unavailable))
            .transpose()?;
        if let Some(cleanup) = &cleanup {
            cleanup.start()?;
        }
        self.expire(now);
        if self.entries.len() >= self.capacity {
            self.entries.pop_first();
        }
        self.next_id = self
            .next_id
            .checked_add(1)
            .expect("directory lease id space");
        self.entries.insert(
            self.next_id,
            DirectoryLease {
                handle,
                expires: now + self.ttl,
            },
        );
        if let Some(cleanup) = cleanup {
            cleanup.wake.notify_one();
        }
        Ok(self.next_id)
    }

    fn get(&mut self, id: u64, now: Instant) -> Result<Arc<File>, String> {
        self.expire(now);
        self.entries
            .get(&id)
            .map(|lease| Arc::clone(&lease.handle))
            .ok_or_else(|| invalid("directory cursor lease expired or was evicted"))
    }
}

struct DirectoryProof {
    binding: String,
    lease: Option<(u64, Arc<File>)>,
}

impl DirectoryProof {
    fn binding_at(
        &self,
        root: &Path,
        target: &Path,
        path: &str,
        created: &dyn Fn(&Metadata) -> Option<Duration>,
    ) -> Result<String, String> {
        let metadata = directory_metadata(target)?;
        if let Some((id, handle)) = &self.lease {
            if fenced_scope_path(root, path).ok().as_deref() != Some(target) {
                return Err(invalid("directory cursor path changed while reading"));
            }
            let held = handle
                .metadata()
                .map_err(|_| invalid("directory lease is no longer readable"))?;
            let identity = required_file_identity(&held)?;
            if required_file_identity(&metadata)? != identity {
                return Err(invalid("directory cursor path was replaced"));
            }
            identity_binding(root, target, path, ("lease", identity, id))
        } else {
            let identity = stable_directory_identity(file_identity(&metadata), created(&metadata))?;
            identity_binding(root, target, path, identity)
        }
    }
}

struct Entry {
    key: SortKey,
    row: Value,
}
impl PartialEq for Entry {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}
impl Eq for Entry {}
impl PartialOrd for Entry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Entry {
    fn cmp(&self, other: &Self) -> Ordering {
        self.key.cmp(&other.key)
    }
}

fn invalid(message: &str) -> String {
    errors::encode(ApiError::invalid_params(message))
}

pub(super) fn listing(root: &Path, path: &str, params: &Value) -> Result<Value, String> {
    listing_with_proof(
        root,
        path,
        params,
        &LEASES.registry,
        Instant::now(),
        &creation_time,
    )
}

fn creation_time(metadata: &Metadata) -> Option<Duration> {
    metadata
        .created()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
}

fn listing_with_proof(
    root: &Path,
    path: &str,
    params: &Value,
    leases: &Mutex<LeaseRegistry>,
    now: Instant,
    created: &dyn Fn(&Metadata) -> Option<Duration>,
) -> Result<Value, String> {
    let requested_limit = params
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_LIMIT as u64);
    if !(1..=MAX_LIMIT as u64).contains(&requested_limit) {
        return Err(invalid("directory page limit must be between 1 and 200"));
    }
    let limit = requested_limit as usize;
    let cursor = params
        .get("cursor")
        .and_then(Value::as_str)
        .map(decode)
        .transpose()?;
    let target = fenced_scope_path(root, path).map_err(|error| {
        if cursor.is_some() {
            invalid("directory cursor path no longer exists or changed")
        } else {
            error
        }
    })?;
    let proof = directory_proof(root, &target, path, cursor.as_ref(), leases, now, created)?;
    let after = cursor.as_ref().map(|cursor| &cursor.after);
    let entries = match &proof.lease {
        Some((_, handle)) => select_lease(handle, after, limit + 1)?,
        None => select(&target, after, limit + 1)?,
    };
    if proof.binding_at(root, &target, path, created)? != proof.binding {
        return Err(invalid("directory changed while reading its page"));
    }
    let has_more = entries.len() > limit;
    let mut page = entries;
    page.truncate(limit);
    let next_cursor =
        has_more.then(|| encode(&proof, &page.last().expect("nonempty bounded page").key));
    if !has_more && cursor.is_none() {
        if let Some((id, _)) = &proof.lease {
            leases
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .entries
                .remove(id);
        }
    }
    Ok(
        json!({ "path":path, "entries":page.into_iter().map(|entry|entry.row).collect::<Vec<_>>(), "has_more":has_more, "next_cursor":next_cursor }),
    )
}

fn directory_proof(
    root: &Path,
    target: &Path,
    path: &str,
    cursor: Option<&Cursor>,
    leases: &Mutex<LeaseRegistry>,
    now: Instant,
    created: &dyn Fn(&Metadata) -> Option<Duration>,
) -> Result<DirectoryProof, String> {
    let metadata = directory_metadata(target)?;
    let lease = if let Some(id) = cursor.and_then(|cursor| cursor.lease) {
        let handle = leases
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(id, now)?;
        Some((id, handle))
    } else if created(&metadata).is_none() {
        if cursor.is_some() {
            return Err(invalid("directory cursor creation proof changed"));
        }
        required_file_identity(&metadata)?;
        let handle = Arc::new(
            File::open(target).map_err(|error| format!("cannot open directory lease: {error}"))?,
        );
        if required_file_identity(
            &handle
                .metadata()
                .map_err(|_| invalid("directory lease is no longer readable"))?,
        )? != required_file_identity(&metadata)?
        {
            return Err(invalid("directory changed while opening its lease"));
        }
        let id = leases
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(Arc::clone(&handle), now)?;
        Some((id, handle))
    } else {
        None
    };
    let mut proof = DirectoryProof {
        binding: String::new(),
        lease,
    };
    proof.binding = proof.binding_at(root, target, path, created)?;
    if cursor.is_some_and(|cursor| cursor.binding != proof.binding) {
        return Err(invalid("malformed or foreign directory cursor"));
    }
    Ok(proof)
}

fn select(target: &Path, after: Option<&SortKey>, capacity: usize) -> Result<Vec<Entry>, String> {
    let reader =
        std::fs::read_dir(target).map_err(|error| format!("cannot read directory: {error}"))?;
    let mut selected = BinaryHeap::<Entry>::with_capacity(capacity + 1);
    for item in reader {
        let item = item.map_err(|error| format!("cannot read directory entry: {error}"))?;
        let Some(entry) = read_entry(item)? else {
            continue;
        };
        if after.is_some_and(|after| entry.key <= *after) {
            continue;
        }
        selected.push(entry);
        if selected.len() > capacity {
            selected.pop();
        }
    }
    Ok(selected.into_sorted_vec())
}

fn read_entry(entry: DirEntry) -> Result<Option<Entry>, String> {
    let filename = entry.file_name();
    let name = filename.to_string_lossy();
    if name == ".git" {
        return Ok(None);
    }
    let kind = match entry.file_type() {
        Ok(kind) => kind,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("cannot inspect directory entry: {error}")),
    };
    let row = if kind.is_dir() {
        json!({"name":name,"kind":"dir"})
    } else if kind.is_symlink() {
        json!({"name":name,"kind":"symlink"})
    } else {
        json!({"name":name,"kind":"file","size":entry.metadata().map(|metadata|metadata.len()).unwrap_or(0)})
    };
    Ok(Some(Entry {
        key: SortKey(
            u8::from(!kind.is_dir()),
            name.to_lowercase(),
            filename.as_encoded_bytes().to_vec(),
        ),
        row,
    }))
}

#[cfg(unix)]
fn select_lease(
    handle: &File,
    after: Option<&SortKey>,
    capacity: usize,
) -> Result<Vec<Entry>, String> {
    use rustix::fs::{statat, AtFlags, Dir, FileType};
    let reader =
        Dir::read_from(handle).map_err(|error| format!("cannot read directory lease: {error}"))?;
    let mut selected = BinaryHeap::<Entry>::with_capacity(capacity + 1);
    for item in reader {
        let item = item.map_err(|error| format!("cannot read directory entry: {error}"))?;
        let bytes = item.file_name().to_bytes();
        if matches!(bytes, b"." | b".." | b".git") {
            continue;
        }
        let stat = match statat(handle, item.file_name(), AtFlags::SYMLINK_NOFOLLOW) {
            Ok(stat) => stat,
            Err(rustix::io::Errno::NOENT) => continue,
            Err(error) => return Err(format!("cannot inspect directory entry: {error}")),
        };
        let name = String::from_utf8_lossy(bytes);
        let kind = FileType::from_raw_mode(stat.st_mode);
        let row = match kind {
            FileType::Directory => json!({"name":name,"kind":"dir"}),
            FileType::Symlink => json!({"name":name,"kind":"symlink"}),
            _ => json!({"name":name,"kind":"file","size":u64::try_from(stat.st_size).unwrap_or(0)}),
        };
        let entry = Entry {
            key: SortKey(
                u8::from(kind != FileType::Directory),
                name.to_lowercase(),
                bytes.to_vec(),
            ),
            row,
        };
        if after.is_some_and(|after| entry.key <= *after) {
            continue;
        }
        selected.push(entry);
        if selected.len() > capacity {
            selected.pop();
        }
    }
    Ok(selected.into_sorted_vec())
}

#[cfg(not(unix))]
fn select_lease(
    _handle: &File,
    _after: Option<&SortKey>,
    _capacity: usize,
) -> Result<Vec<Entry>, String> {
    Err(errors::encode(ApiError::unavailable(
        "Directory paging needs stable Unix directory identity.",
    )))
}

fn directory_metadata(target: &Path) -> Result<Metadata, String> {
    let metadata =
        std::fs::metadata(target).map_err(|_| invalid("directory cursor path no longer exists"))?;
    if !metadata.is_dir() {
        return Err(invalid("directory cursor path is not a directory"));
    }
    Ok(metadata)
}

fn required_file_identity(metadata: &Metadata) -> Result<(u64, u64), String> {
    file_identity(metadata)
        .filter(|(device, inode)| *device != 0 && *inode != 0)
        .ok_or_else(|| {
            errors::encode(ApiError::unavailable(
                "Directory paging needs stable Unix directory identity.",
            ))
        })
}

fn identity_binding(
    root: &Path,
    target: &Path,
    path: &str,
    identity: impl Serialize,
) -> Result<String, String> {
    let canonical_root =
        std::fs::canonicalize(root).map_err(|_| invalid("directory scope no longer exists"))?;
    let bytes = serde_json::to_vec(&(
        canonical_root.as_os_str().as_encoded_bytes(),
        target.as_os_str().as_encoded_bytes(),
        path,
        identity,
    ))
    .expect("directory identity serializes");
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

/// A birth-time proof requires creation metadata. The page reader instead
/// holds a bounded directory lease when this is the only missing proof.
fn stable_directory_identity(
    file_id: Option<(u64, u64)>,
    created: Option<std::time::Duration>,
) -> Result<(u64, u64, u128), String> {
    match (file_id, created) {
        (Some((device, inode)), Some(created)) if device != 0 && inode != 0 => {
            Ok((device, inode, created.as_nanos()))
        }
        _ => Err(errors::encode(ApiError::unavailable(
            "Directory paging is unavailable: the filesystem cannot provide stable directory identity and creation metadata.",
        ))),
    }
}

#[cfg(unix)]
fn file_identity(metadata: &Metadata) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    Some((metadata.dev(), metadata.ino()))
}
#[cfg(not(unix))]
fn file_identity(_metadata: &Metadata) -> Option<(u64, u64)> {
    None
}

fn encode(proof: &DirectoryProof, after: &SortKey) -> String {
    let bytes = serde_json::to_vec(&Cursor {
        version: 1,
        binding: proof.binding.clone(),
        after: after.clone(),
        lease: proof.lease.as_ref().map(|(id, _)| *id),
    })
    .expect("directory cursor serializes");
    let signature = hmac::sign(&CURSOR_KEY, &bytes);
    format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(bytes),
        URL_SAFE_NO_PAD.encode(signature.as_ref())
    )
}

fn decode(token: &str) -> Result<Cursor, String> {
    let refusal = || invalid("malformed or foreign directory cursor");
    if token.len() > MAX_CURSOR_BYTES {
        return Err(refusal());
    }
    let (payload, signature) = token.split_once('.').ok_or_else(refusal)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload).map_err(|_| refusal())?;
    let signature = URL_SAFE_NO_PAD.decode(signature).map_err(|_| refusal())?;
    hmac::verify(&CURSOR_KEY, &bytes, &signature).map_err(|_| refusal())?;
    let cursor: Cursor = serde_json::from_slice(&bytes).map_err(|_| refusal())?;
    if cursor.version != 1 || cursor.after.0 > 1 {
        return Err(refusal());
    }
    Ok(cursor)
}

#[cfg(test)]
mod identity_tests {
    use super::*;
    use std::time::Duration;

    fn assert_identity_unavailable(file_id: Option<(u64, u64)>, created: Option<Duration>) {
        let error = stable_directory_identity(file_id, created)
            .expect_err("unproven filesystem identity must refuse directory paging");
        let refusal = crate::app::AppState::decode_file_error(&error)
            .expect("identity refusal retains the standard filesystem API error shape");
        assert_eq!(refusal.code(), "unavailable");
    }

    #[test]
    fn directory_identity_requires_creation_proof_even_with_a_known_device_and_inode() {
        assert_identity_unavailable(Some((7, 11)), None);
    }

    #[test]
    fn directory_identity_requires_a_present_nonzero_platform_file_id() {
        let created = Some(Duration::new(123, 456));
        for file_id in [None, Some((0, 0)), Some((0, 11)), Some((7, 0))] {
            assert_identity_unavailable(file_id, created);
        }
    }

    #[test]
    fn directory_identity_preserves_supported_file_and_creation_proof() {
        assert_eq!(
            stable_directory_identity(Some((7, 11)), Some(Duration::new(123, 456))).unwrap(),
            (7, 11, 123_000_000_456_u128),
        );
    }
}

#[cfg(all(test, unix))]
mod lease_tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    fn fixture() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn fill(path: &Path) {
        std::fs::create_dir_all(path).unwrap();
        for name in ["a", "b", "c", "d", "e"] {
            std::fs::write(path.join(name), "synthetic").unwrap();
        }
    }

    fn page(
        root: &Path,
        path: &str,
        params: Value,
        leases: &Mutex<LeaseRegistry>,
        now: Instant,
    ) -> Result<Value, String> {
        listing_with_proof(root, path, &params, leases, now, &|_| None)
    }

    fn assert_refused(answer: Result<Value, String>) {
        let error = answer.expect_err("a retired directory lease cannot continue");
        let refusal = crate::app::AppState::decode_file_error(&error).unwrap();
        assert_eq!(refusal.code(), "invalid_params");
    }

    #[test]
    fn directory_lease_idle_expiry_releases_last_file_without_another_request() {
        let root = fixture();
        let retained = Arc::new(File::open(root.path()).unwrap());
        let released = Arc::downgrade(&retained);
        let timed = TimedLeaseRegistry::new(2, Duration::from_millis(30));
        let mut registry = timed.registry.lock().unwrap();
        registry.insert(retained, Instant::now()).unwrap();
        assert_eq!(released.strong_count(), 1);
        // Waiting observes the actual File owner's lifetime. No later registry
        // get/insert can perform lazy cleanup and make this pass accidentally.
        let (registry, _) = timed
            .cleanup
            .wake
            .wait_timeout_while(registry, Duration::from_secs(2), |_| {
                released.strong_count() != 0
            })
            .unwrap();
        assert_eq!(
            released.strong_count(),
            0,
            "idle timer must release the last retained directory File"
        );
        assert!(registry.entries.is_empty());
    }

    #[test]
    fn directory_lease_owner_drop_stops_cleanup_and_releases_live_files() {
        let root = fixture();
        let retained = Arc::new(File::open(root.path()).unwrap());
        let released = Arc::downgrade(&retained);
        let timed = TimedLeaseRegistry::new(2, Duration::from_secs(120));
        timed
            .registry
            .lock()
            .unwrap()
            .insert(retained, Instant::now())
            .unwrap();
        let (sent, received) = std::sync::mpsc::channel();
        let owner = std::thread::spawn(move || {
            drop(timed);
            sent.send(()).unwrap();
        });
        received
            .recv_timeout(Duration::from_secs(2))
            .expect("cleanup worker must stop promptly with its owner");
        owner.join().unwrap();
        assert_eq!(released.strong_count(), 0);
    }

    #[test]
    fn directory_lease_birth_time_reads_do_not_start_a_cleanup_worker() {
        let root = fixture();
        fill(root.path());
        let timed = TimedLeaseRegistry::new(2, Duration::from_secs(120));
        let first = listing_with_proof(
            root.path(),
            "",
            &json!({"limit":2}),
            &timed.registry,
            Instant::now(),
            &|_| Some(Duration::new(123, 456)),
        )
        .unwrap();
        assert!(first["has_more"].as_bool().unwrap());
        assert!(timed.cleanup.worker.lock().unwrap().is_none());
        assert!(timed.registry.lock().unwrap().entries.is_empty());
    }

    #[test]
    fn directory_lease_pages_a_cold_directory_without_creation_metadata() {
        let root = fixture();
        fill(root.path());
        let leases = Mutex::new(LeaseRegistry::new(2, Duration::from_secs(120)));
        let now = Instant::now();
        let mut params = json!({"limit":2});
        let mut names = Vec::new();
        loop {
            let answer = page(root.path(), "", params.clone(), &leases, now).unwrap();
            let entries = answer["entries"].as_array().unwrap();
            assert!(entries.len() <= 2);
            names.extend(
                entries
                    .iter()
                    .map(|entry| entry["name"].as_str().unwrap().to_owned()),
            );
            if !answer["has_more"].as_bool().unwrap() {
                assert!(answer["next_cursor"].is_null());
                break;
            }
            params["cursor"] = answer["next_cursor"].clone();
        }
        assert_eq!(names, ["a", "b", "c", "d", "e"]);
        assert_eq!(leases.lock().unwrap().entries.len(), 1);
    }

    #[test]
    fn directory_lease_holds_the_removed_directory_open_and_refuses_its_replacement() {
        use std::os::unix::fs::MetadataExt;
        let root = fixture();
        let path = root.path().join("one");
        fill(&path);
        let original = file_identity(&std::fs::metadata(&path).unwrap());
        let leases = Mutex::new(LeaseRegistry::new(2, Duration::from_secs(120)));
        let now = Instant::now();
        let first = page(root.path(), "one", json!({"limit":2}), &leases, now).unwrap();
        std::fs::remove_dir_all(&path).unwrap();
        fill(&path);
        let mut registry = leases.lock().unwrap();
        let id = *registry.entries.keys().next().unwrap();
        let held = registry.get(id, now).unwrap().metadata().unwrap();
        assert_eq!(file_identity(&held), original);
        assert_eq!(
            held.nlink(),
            0,
            "the lease pins the unlinked original inode"
        );
        assert_ne!(file_identity(&std::fs::metadata(&path).unwrap()), original);
        drop(registry);
        assert_refused(page(
            root.path(),
            "one",
            json!({"cursor":first["next_cursor"]}),
            &leases,
            now,
        ));
    }

    #[test]
    fn directory_lease_registry_caps_retained_handles_and_evicts_oldest_cursor() {
        let root = fixture();
        let leases = Mutex::new(LeaseRegistry::new(2, Duration::from_secs(120)));
        let now = Instant::now();
        let mut pages = Vec::new();
        for path in ["one", "two", "three"] {
            fill(&root.path().join(path));
            pages.push(page(root.path(), path, json!({"limit":2}), &leases, now).unwrap());
            assert!(leases.lock().unwrap().entries.len() <= 2);
        }
        assert_refused(page(
            root.path(),
            "one",
            json!({"cursor":pages[0]["next_cursor"]}),
            &leases,
            now,
        ));
        assert!(page(
            root.path(),
            "three",
            json!({"cursor":pages[2]["next_cursor"]}),
            &leases,
            now
        )
        .is_ok());
        assert!(page(root.path(), "one", json!({"limit":2}), &leases, now).is_ok());
    }

    #[test]
    fn directory_lease_expiry_refuses_continuation_but_allows_a_new_first_page() {
        let root = fixture();
        fill(root.path());
        let ttl = Duration::from_secs(120);
        let leases = Mutex::new(LeaseRegistry::new(2, ttl));
        let now = Instant::now();
        let first = page(root.path(), "", json!({"limit":2}), &leases, now).unwrap();
        let second = page(
            root.path(),
            "",
            json!({"limit":1,"cursor":first["next_cursor"]}),
            &leases,
            now + ttl / 2,
        )
        .unwrap();
        let expired = now + ttl;
        assert_refused(page(
            root.path(),
            "",
            json!({"cursor":second["next_cursor"]}),
            &leases,
            expired,
        ));
        assert!(page(root.path(), "", json!({"limit":2}), &leases, expired).is_ok());
        assert_eq!(leases.lock().unwrap().entries.len(), 1);
    }

    #[test]
    fn directory_lease_cursors_are_bound_to_the_path_and_the_live_registry() {
        let root = fixture();
        fill(&root.path().join("one"));
        fill(&root.path().join("two"));
        let leases = Mutex::new(LeaseRegistry::new(2, Duration::from_secs(120)));
        let now = Instant::now();
        let first = page(root.path(), "one", json!({"limit":2}), &leases, now).unwrap();
        let cursor = json!({"cursor":first["next_cursor"]});
        assert_refused(page(root.path(), "two", cursor.clone(), &leases, now));
        let restarted = Mutex::new(LeaseRegistry::new(2, Duration::from_secs(120)));
        assert_refused(page(root.path(), "one", cursor, &restarted, now));
    }

    #[test]
    fn directory_lease_scans_the_held_directory_not_a_replacement_at_its_path() {
        let root = fixture();
        let target = root.path().join("one");
        fill(&target);
        let held = std::fs::File::open(&target).unwrap();
        std::fs::rename(&target, root.path().join("retired")).unwrap();
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("foreign"), "synthetic").unwrap();
        let page = select_lease(&held, None, 2).unwrap();
        assert_eq!(
            page.iter()
                .map(|entry| entry.row["name"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["a", "b"]
        );
    }

    #[test]
    fn directory_lease_is_not_used_when_creation_proof_exists() {
        let root = fixture();
        fill(root.path());
        let leases = Mutex::new(LeaseRegistry::new(2, Duration::from_secs(120)));
        let first = listing_with_proof(
            root.path(),
            "",
            &json!({"limit":2}),
            &leases,
            Instant::now(),
            &|_| Some(Duration::new(123, 456)),
        )
        .unwrap();
        let payload = first["next_cursor"]
            .as_str()
            .unwrap()
            .split_once('.')
            .unwrap()
            .0;
        let fields: Value =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).unwrap()).unwrap();
        assert_eq!(
            fields.as_object().unwrap().len(),
            3,
            "normal cursors retain exactly version/binding/after"
        );
        assert!(leases.lock().unwrap().entries.is_empty());
    }
}

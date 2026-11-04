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
use std::collections::BinaryHeap;
use std::fs::{DirEntry, Metadata};
use std::path::Path;
use std::sync::LazyLock;

const DEFAULT_LIMIT: usize = 100;
const MAX_LIMIT: usize = 200;
const MAX_CURSOR_BYTES: usize = 8192;
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
    let requested_limit = params
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_LIMIT as u64);
    if !(1..=MAX_LIMIT as u64).contains(&requested_limit) {
        return Err(invalid("directory page limit must be between 1 and 200"));
    }
    let limit = requested_limit as usize;
    let target = fenced_scope_path(root, path).map_err(|error| {
        if params.get("cursor").is_some() {
            invalid("directory cursor path no longer exists or changed")
        } else {
            error
        }
    })?;
    let binding = directory_binding(root, &target, path)?;
    let after = params
        .get("cursor")
        .and_then(Value::as_str)
        .map(|token| decode(token, &binding))
        .transpose()?;
    let entries = select(&target, after.as_ref(), limit + 1)?;
    if directory_binding(root, &target, path)? != binding {
        return Err(invalid("directory changed while reading its page"));
    }
    let has_more = entries.len() > limit;
    let mut page = entries;
    page.truncate(limit);
    let next_cursor =
        has_more.then(|| encode(&binding, &page.last().expect("nonempty bounded page").key));
    Ok(
        json!({ "path":path, "entries":page.into_iter().map(|entry|entry.row).collect::<Vec<_>>(), "has_more":has_more, "next_cursor":next_cursor }),
    )
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

fn directory_binding(root: &Path, target: &Path, path: &str) -> Result<String, String> {
    let metadata =
        std::fs::metadata(target).map_err(|_| invalid("directory cursor path no longer exists"))?;
    if !metadata.is_dir() {
        return Err(invalid("directory cursor path is not a directory"));
    }
    let canonical_root =
        std::fs::canonicalize(root).map_err(|_| invalid("directory scope no longer exists"))?;
    let created = metadata
        .created()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos());
    let bytes = serde_json::to_vec(&(
        canonical_root.as_os_str().as_encoded_bytes(),
        target.as_os_str().as_encoded_bytes(),
        path,
        file_identity(&metadata),
        created,
    ))
    .expect("directory identity serializes");
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

#[cfg(unix)]
fn file_identity(metadata: &Metadata) -> (u64, u64) {
    use std::os::unix::fs::MetadataExt;
    (metadata.dev(), metadata.ino())
}
#[cfg(not(unix))]
fn file_identity(_metadata: &Metadata) -> (u64, u64) {
    (0, 0)
}

fn encode(binding: &str, after: &SortKey) -> String {
    let bytes = serde_json::to_vec(&Cursor {
        version: 1,
        binding: binding.into(),
        after: after.clone(),
    })
    .expect("directory cursor serializes");
    let signature = hmac::sign(&CURSOR_KEY, &bytes);
    format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(bytes),
        URL_SAFE_NO_PAD.encode(signature.as_ref())
    )
}

fn decode(token: &str, binding: &str) -> Result<SortKey, String> {
    let refusal = || invalid("malformed or foreign directory cursor");
    if token.len() > MAX_CURSOR_BYTES {
        return Err(refusal());
    }
    let (payload, signature) = token.split_once('.').ok_or_else(refusal)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload).map_err(|_| refusal())?;
    let signature = URL_SAFE_NO_PAD.decode(signature).map_err(|_| refusal())?;
    hmac::verify(&CURSOR_KEY, &bytes, &signature).map_err(|_| refusal())?;
    let cursor: Cursor = serde_json::from_slice(&bytes).map_err(|_| refusal())?;
    if cursor.version != 1 || cursor.binding != binding || cursor.after.0 > 1 {
        return Err(refusal());
    }
    Ok(cursor.after)
}

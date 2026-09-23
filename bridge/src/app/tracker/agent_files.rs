//! Files an agent made itself, filed on an issue (#116).
//!
//! An agent proves a claim with a screenshot, a recording or a log it just
//! produced, and before this it could only list the paths in its text — which
//! nobody reading the issue from the app, least of all from a phone, can open.
//! So an agent's `create_issue` and `comment_issue` take a file by its full
//! path on this machine, and the bridge copies it into the attachment store at
//! intake, exactly as a user's upload is saved: the issue holds the bytes, and
//! the agent clearing out its scratch folder takes nothing off the issue.
//!
//! Only the agent's tools come through here. A path from a CLIENT is hostile
//! input and stays fenced to the store (`attachments.rs`); an agent already
//! reads this disk through its own shell, so a file it names is one it could
//! have pasted anyway. What is refused is what an issue has no use for: a
//! folder, a path with nothing at it, a file this bridge may not read, anything
//! over the cap, and a file that is neither media nor plain text.
//!
//! A path that already names an attachment — what the user sent the agent — is
//! passed through untouched, as it always was.

use super::AppState;
use crate::app::conversations::{mime_hint, sanitize_attachment_name, write_attachment};
use crate::app::sha256_hex;
use serde_json::{json, Value};
use std::io::Read;
use std::path::Path;

/// The largest file an agent may attach. Recordings are the reason it is well
/// past a user upload's cap: a user's file crosses the DataChannel in one
/// message, and an agent's is read back in pieces (`issues.attachment` ranges).
pub const AGENT_ATTACHMENT_MAX_BYTES: u64 = 50 * 1_048_576;

/// Images and videos, by extension. The lightbox draws these; anything else an
/// issue carries is a download.
const MEDIA_EXTENSIONS: [&str; 7] = ["png", "jpg", "jpeg", "webp", "gif", "mp4", "webm"];

/// What a text file may be typed as. HTML and SVG are text too, and are left
/// out on purpose: a file an issue carries is never markup a reader renders.
const TEXT_MIMES: [&str; 3] = ["text/plain", "text/markdown", "application/json"];

impl AppState {
    /// The attachments an agent's issue tool named, with every file the agent
    /// made copied into the store and renamed to the copy. What comes out is
    /// what `parse_issue_attachments` reads, the same as a client's list.
    pub(in crate::app) fn take_in_agent_files(
        &self,
        listed: &[Value],
    ) -> Result<Vec<Value>, String> {
        listed
            .iter()
            .map(|entry| self.take_in_agent_file(entry))
            .collect()
    }

    fn take_in_agent_file(&self, entry: &Value) -> Result<Value, String> {
        // No path is the parse's refusal to make, in its own words.
        let Some(path) = entry.get("path").and_then(Value::as_str) else {
            return Ok(entry.clone());
        };
        if self.resolve_issue_attachment(path).is_ok() {
            return Ok(entry.clone());
        }
        let content = agent_file_content(path)?;
        let leaf = sanitize_attachment_name(path);
        let stored = format!("{}-{leaf}", &sha256_hex(&content)[..12]);
        let home = self.local_attachments_dir();
        write_attachment(&home, &stored, &content)?;
        let mut taken = entry.clone();
        taken["path"] = json!(home.join(&stored).display().to_string());
        let named = entry
            .get("name")
            .and_then(Value::as_str)
            .is_some_and(|name| !name.trim().is_empty());
        if !named {
            taken["name"] = json!(leaf);
        }
        Ok(taken)
    }
}

/// The bytes of a file an agent named, or the sentence that says why not.
fn agent_file_content(path: &str) -> Result<Vec<u8>, String> {
    let source = Path::new(path);
    if !source.is_absolute() {
        return Err(format!(
            "Build cannot attach {path}: give the file's full path."
        ));
    }
    let metadata = std::fs::metadata(source).map_err(|error| unopenable(path, &error))?;
    if metadata.is_dir() {
        return Err(format!(
            "Build cannot attach {path}: it is a folder, not a file."
        ));
    }
    if metadata.len() > AGENT_ATTACHMENT_MAX_BYTES {
        return Err(over_the_cap());
    }
    let content = read_capped(source).map_err(|error| unopenable(path, &error))?;
    if content.len() as u64 > AGENT_ATTACHMENT_MAX_BYTES {
        // It grew between the look and the read.
        return Err(over_the_cap());
    }
    if content.is_empty() {
        return Err(format!("Build cannot attach {path}: it is empty."));
    }
    if !is_media(source) && !is_plain_text(source, &content) {
        return Err(format!(
            "Build cannot attach {path}: only images (png, jpg, webp, gif), videos (mp4, webm) and plain text or logs can be attached."
        ));
    }
    Ok(content)
}

fn over_the_cap() -> String {
    format!(
        "Build cannot attach a file larger than {} MB.",
        AGENT_ATTACHMENT_MAX_BYTES / 1_048_576
    )
}

fn unopenable(path: &str, error: &std::io::Error) -> String {
    match error.kind() {
        std::io::ErrorKind::NotFound => {
            format!("Build cannot attach {path}: there is no file there.")
        }
        _ => format!("Build cannot attach {path}: it cannot be read."),
    }
}

/// At most one byte past the cap, so a file that grows under the read is
/// caught without reading all of it.
fn read_capped(source: &Path) -> std::io::Result<Vec<u8>> {
    let mut content = Vec::new();
    std::fs::File::open(source)?
        .take(AGENT_ATTACHMENT_MAX_BYTES + 1)
        .read_to_end(&mut content)?;
    Ok(content)
}

fn is_media(source: &Path) -> bool {
    source
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_lowercase)
        .is_some_and(|extension| MEDIA_EXTENSIONS.contains(&extension.as_str()))
}

fn is_plain_text(source: &Path, content: &[u8]) -> bool {
    TEXT_MIMES.contains(&mime_hint(source, content)) && std::str::from_utf8(content).is_ok()
}

//! Files filed with an issue (spec: Issues → Attachments).
//!
//! The same story a conversation's attachments tell, told once. Everything that
//! decides what a file IS — the size cap, the name flattening, the
//! content-addressed leaf, the mime sniff — is
//! [`crate::app::conversations::attachments`]'s, imported rather than copied,
//! so a screenshot sent to an agent in a conversation and one filed on an issue
//! are the same object with the same rules.
//!
//! What differs is WHERE, and it differs because of what an issue is.
//!
//! A conversation attachment has two homes: the worktree its agent reads from,
//! and the bridge's own store, which outlives every checkout. An issue has no
//! worktree. It may never have one — an issue filed unassigned has no
//! conversation at all, and one assigned to the user never gets a checkout — so
//! there is nowhere to put a second copy and nothing that would read it. The
//! durable store is therefore the only home, which is also why
//! [`AppState::issues_attach`] takes a `project_id` and not an `entity_id`:
//! `attachment_homes` resolves a plan or a run, and an issue being CREATED is
//! neither.
//!
//! ## The fence
//!
//! A path from a client is hostile input, and an "attachment" that reads any
//! file on the disk is an arbitrary-file read with a nice name. So a path is
//! resolved and then checked for containment under the store after
//! canonicalisation — never a prefix match on the string, which `..` walks
//! straight through.
//!
//! The leaf retry is what lets an AGENT attach what the user sent it. Its own
//! copy is worktree-relative (`.build/attachments/<leaf>`), but both homes
//! store the same content-addressed leaf, so the durable copy answers for a
//! path written against a checkout this issue knows nothing about.

use super::AppState;
use crate::app::conversations::{
    mime_hint, sanitize_attachment_name, write_attachment, ATTACHMENTS_PER_MESSAGE_MAX,
    ATTACHMENT_MAX_BYTES,
};
use crate::app::require_str;
use crate::app::sha256_hex;
use crate::encoding::{b64decode, b64encode};
use serde_json::{json, Value};
use std::io::Read;

/// The most one `issues.attachment` answer carries. A user's upload is capped
/// at the same size so it always comes back whole; an agent's recording can be
/// ten times that, and is read back in pieces of this size. Base64 costs a
/// third on top, and the piece plus its envelope still fits under the 8 MiB
/// DataChannel reassembly cap.
pub const ATTACHMENT_READ_CHUNK_BYTES: u64 = ATTACHMENT_MAX_BYTES;

/// How much of a file is read to type it — the same head `thread.attach` sniffs,
/// so one file typed twice types the same both times.
const MIME_SNIFF_BYTES: usize = 8192;

impl AppState {
    /// `issues.attach` — take one file the reviewer is filing with an issue and
    /// put it where the issue can name it.
    ///
    /// Writing is deliberately separate from filing, exactly as it is for a
    /// message: the bytes are on disk and verified before the issue that
    /// references them exists, so an issue can never point at an upload that
    /// failed halfway, and a file that will not land is refused on its own chip
    /// rather than failing the whole filing.
    ///
    /// Content-addressed, so the same screenshot filed on three issues costs
    /// one copy.
    pub(crate) fn issues_attach(&mut self, params: &Value) -> Result<Value, String> {
        let project_id = require_str(params, "project_id")?;
        // Refused for a project this bridge does not serve: answering for one
        // is saying it exists.
        self.tracker_project_path(&project_id)?;
        let filename = require_str(params, "filename")?;
        let content = b64decode(&require_str(params, "content_b64")?)?;
        if content.len() as u64 > ATTACHMENT_MAX_BYTES {
            return Err(format!(
                "attachment is {} bytes; the limit is {ATTACHMENT_MAX_BYTES} bytes",
                content.len()
            ));
        }
        if content.is_empty() {
            return Err("attachment is empty".to_string());
        }
        let home = self.local_attachments_dir();
        let name = sanitize_attachment_name(&filename);
        let stored = format!("{}-{name}", &sha256_hex(&content)[..12]);
        write_attachment(&home, &stored, &content)?;
        let path = home.join(&stored);
        let head = &content[..content.len().min(MIME_SNIFF_BYTES)];
        Ok(json!({
            "name": name,
            "path": path.display().to_string(),
            "mime": mime_hint(std::path::Path::new(&stored), head),
            "size": content.len(),
        }))
    }

    /// `issues.attachment` — hand an attachment's bytes back to the surface
    /// that is drawing the issue. The browser cannot reach the disk, and
    /// routing the read through the issue means no caller has to know (or can
    /// get wrong) where the file landed.
    ///
    /// One piece at a time (1.19): `offset` and `length` name a range, the
    /// answer says where it starts, and `size` is always the whole file's, so
    /// a reader asks again from `offset + piece` until it has `size` bytes. A
    /// file no bigger than one piece comes back whole from an unranged read,
    /// exactly as it did before ranges.
    pub(crate) fn issues_attachment(&mut self, params: &Value) -> Result<Value, String> {
        let issue_id = require_str(params, "issue_id")?;
        let path = require_str(params, "path")?;
        // The issue has to exist and be one this bridge serves before any of
        // its bytes are read: a read fenced only by the store would answer for
        // an issue that is not there.
        self.tracker_issue(&issue_id)?;
        let target = self.resolve_issue_attachment(&path)?;
        let offset = params.get("offset").and_then(Value::as_u64).unwrap_or(0);
        let length = params
            .get("length")
            .and_then(Value::as_u64)
            .unwrap_or(ATTACHMENT_READ_CHUNK_BYTES)
            .min(ATTACHMENT_READ_CHUNK_BYTES);
        let (size, piece) = read_range(&target, offset, length)
            .map_err(|e| format!("cannot read the attachment: {e}"))?;
        let head = read_head(&target).map_err(|e| format!("cannot read the attachment: {e}"))?;
        Ok(json!({
            "path": path,
            "size": size,
            "mime": mime_hint(&target, &head),
            "offset": offset,
            "content_b64": b64encode(&piece),
        }))
    }

    /// Resolve a client-supplied path to a real file, refusing anything that is
    /// not in this bridge's attachment store.
    ///
    /// Containment after canonicalisation, never a prefix match on the string:
    /// `../../etc/passwd` satisfies a prefix check and fails this one.
    pub(in crate::app) fn resolve_issue_attachment(
        &self,
        path: &str,
    ) -> Result<std::path::PathBuf, String> {
        let home = self.local_attachments_dir();
        let canonical_home = std::fs::canonicalize(&home)
            .map_err(|_| format!("not an attachment on this issue: {path}"))?;
        let candidate = std::path::Path::new(path);
        let attempts = [
            candidate.is_absolute().then(|| candidate.to_path_buf()),
            // An agent attaches what the user sent IT, and its copy is named
            // relative to a checkout. Both homes store the same leaf.
            candidate.file_name().map(|leaf| home.join(leaf)),
        ];
        for attempt in attempts.into_iter().flatten() {
            let Ok(resolved) = std::fs::canonicalize(&attempt) else {
                continue;
            };
            if resolved.starts_with(&canonical_home) && resolved.is_file() {
                return Ok(resolved);
            }
        }
        Err(format!("not an attachment on this issue: {path}"))
    }

    /// The files an `issues.create` or `issues.comment` says it is filing.
    ///
    /// Name, mime and size are re-read from disk rather than trusted: the
    /// client's copy is a display hint, and the record a reader opens should
    /// describe the bytes that exist.
    pub(in crate::app) fn parse_issue_attachments(
        &self,
        params: &Value,
    ) -> Result<Vec<crate::thread::MessageAttachment>, String> {
        let Some(value) = params.get("attachments").filter(|v| !v.is_null()) else {
            return Ok(Vec::new());
        };
        let listed = value.as_array().ok_or("attachments must be an array")?;
        if listed.len() > ATTACHMENTS_PER_MESSAGE_MAX {
            return Err(format!(
                "an issue carries at most {ATTACHMENTS_PER_MESSAGE_MAX} attachments"
            ));
        }
        listed
            .iter()
            .map(|entry| self.one_issue_attachment(entry))
            .collect()
    }

    fn one_issue_attachment(
        &self,
        entry: &Value,
    ) -> Result<crate::thread::MessageAttachment, String> {
        let path = entry
            .get("path")
            .and_then(Value::as_str)
            .ok_or("each attachment needs a path")?;
        let resolved = self.resolve_issue_attachment(path)?;
        let size = std::fs::metadata(&resolved)
            .map_err(|e| format!("cannot stat the attachment: {e}"))?
            .len();
        let head = read_head(&resolved).map_err(|e| format!("cannot read the attachment: {e}"))?;
        let name = entry
            .get("name")
            .and_then(Value::as_str)
            .map(sanitize_attachment_name)
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| sanitize_attachment_name(path));
        Ok(crate::thread::MessageAttachment {
            name,
            // The path as the client named it, so a record written against a
            // checkout still reads the way it was written.
            path: path.to_string(),
            mime: mime_hint(&resolved, &head).to_string(),
            size,
        })
    }
}

/// The first bytes of a file, enough to type it.
fn read_head(path: &std::path::Path) -> std::io::Result<Vec<u8>> {
    let mut head = Vec::new();
    std::fs::File::open(path)?
        .take(MIME_SNIFF_BYTES as u64)
        .read_to_end(&mut head)?;
    Ok(head)
}

/// The whole file's size, and at most `length` bytes of it from `offset`. An
/// offset at or past the end answers an empty piece rather than an error: the
/// reader asked for what is left, and nothing is.
fn read_range(path: &std::path::Path, offset: u64, length: u64) -> std::io::Result<(u64, Vec<u8>)> {
    use std::io::{Seek, SeekFrom};
    let mut file = std::fs::File::open(path)?;
    let size = file.metadata()?.len();
    let mut piece = Vec::new();
    file.seek(SeekFrom::Start(offset.min(size)))?;
    file.take(length).read_to_end(&mut piece)?;
    Ok((size, piece))
}

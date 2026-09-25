use super::AppState;
use crate::app::{require_str, sha256_hex};
use crate::encoding::{b64decode, b64encode};
use serde_json::{json, Value};
use std::io::Read;

/// The folders one conversation's attachments live in.
pub(in crate::app) struct AttachmentHomes {
    /// `<worktree>/.build/attachments`, when the entity has a checkout: the
    /// copy the agent is told about.
    worktree: Option<std::path::PathBuf>,
    /// The bridge's own store: the copy that outlives every checkout.
    local: std::path::PathBuf,
}

impl AttachmentHomes {
    /// Worktree first — a path recorded against it should resolve there rather
    /// than through the fallback, so a stale local copy can never shadow the
    /// file the agent is actually looking at.
    pub(in crate::app) fn in_read_order(self) -> Vec<std::path::PathBuf> {
        self.worktree.into_iter().chain([self.local]).collect()
    }
}

/// How large one conversation attachment may be. An upload crosses the DataChannel,
/// reassembled under the 8 MiB DataChannel cap (`MAX_REASSEMBLED_BYTES`,
/// `rtc/chunk.rs`), and base64
/// costs a third on top, so the cap is set where a file plus its envelope still fits
/// with room to spare — and refused here, with a number the composer can show,
/// rather than by a closed channel.
pub const ATTACHMENT_MAX_BYTES: u64 = 5 * 1_048_576;

/// The one folder a conversation attachment may live in, worktree-relative.
pub(in crate::app) const ATTACHMENTS_DIR: &str = ".build/attachments";

/// How many files may ride one message.
pub(in crate::app) const ATTACHMENTS_PER_MESSAGE_MAX: usize = 10;

/// Flatten a filename from the reviewer's machine into one safe leaf.
///
/// This is hostile input: it arrives from a file picker, a paste, or a drop, on
/// any OS, and it decides a name on THIS disk. Everything that could make it
/// mean a location rather than a name — separators, `..`, control characters,
/// a leading dot — is removed, and what survives is capped with its extension
/// kept so the mime hint and the reviewer's eye both still work.
pub(in crate::app) fn sanitize_attachment_name(raw: &str) -> String {
    let leaf = raw.rsplit(['/', '\\']).next().unwrap_or_default();
    let cleaned: String = leaf
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | ':') {
                '-'
            } else {
                c
            }
        })
        .collect();
    let cleaned = cleaned.trim_matches(|c: char| c == '.' || c.is_whitespace());
    if cleaned.is_empty() {
        return "attachment".to_string();
    }
    const NAME_MAX_CHARS: usize = 80;
    if cleaned.chars().count() <= NAME_MAX_CHARS {
        return cleaned.to_string();
    }
    let extension = std::path::Path::new(cleaned)
        .extension()
        .and_then(|e| e.to_str())
        .filter(|e| e.chars().count() <= 12)
        .map(|e| format!(".{e}"))
        .unwrap_or_default();
    let head: String = cleaned
        .chars()
        .take(NAME_MAX_CHARS - extension.chars().count())
        .collect();
    format!("{head}{extension}")
}

pub(in crate::app) fn write_attachment(
    home: &std::path::Path,
    stored_name: &str,
    content: &[u8],
) -> Result<(), String> {
    std::fs::create_dir_all(home)
        .map_err(|e| format!("cannot create the attachments folder: {e}"))?;
    std::fs::write(home.join(stored_name), content)
        .map_err(|e| format!("cannot write {stored_name}: {e}"))
}

/// Keep a worktree's attachments out of every diff and every commit.
///
/// `.build/.gitignore` already exists to hold `mcp.json` back from the agent's
/// own `git add -A`; conversation attachments need exactly the same protection
/// for the same reason, and appending is idempotent so a worktree scaffolded by
/// an older build picks the rule up the first time a file is attached to it.
pub(in crate::app) fn ensure_attachments_ignored(
    build_dir: &std::path::Path,
) -> std::io::Result<()> {
    let ignore = build_dir.join(".gitignore");
    let existing = std::fs::read_to_string(&ignore).unwrap_or_default();
    if existing.lines().any(|line| line.trim() == "attachments/") {
        return Ok(());
    }
    let mut updated = existing;
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    updated.push_str("attachments/\n");
    std::fs::write(&ignore, updated)
}

/// Extension-based mime hint for `fs.read` previews (spec §4.3's pinned
/// table). `head` is (at most) the first 8 KiB of the file's content — used
/// only to distinguish text from binary when the extension doesn't match.
pub(in crate::app) fn mime_hint(path: &std::path::Path, head: &[u8]) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_lowercase);
    match ext.as_deref() {
        Some("md") | Some("markdown") => "text/markdown",
        Some("html") | Some("htm") => "text/html",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        Some("bmp") => "image/bmp",
        Some("mp3") => "audio/mpeg",
        Some("wav") => "audio/wav",
        Some("m4a") => "audio/mp4",
        Some("aac") => "audio/aac",
        Some("flac") => "audio/flac",
        Some("mp4") | Some("m4v") => "video/mp4",
        Some("webm") => "video/webm",
        Some("mov") => "video/quicktime",
        Some("json") => "application/json",
        Some("pdf") => "application/pdf",
        _ if head.contains(&0u8) => "application/octet-stream",
        _ => "text/plain",
    }
}

pub(in crate::app) fn media_mime_hint(path: &std::path::Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_lowercase();
    match ext.as_str() {
        "mp3" => Some("audio/mpeg"),
        "wav" => Some("audio/wav"),
        "m4a" => Some("audio/mp4"),
        "aac" => Some("audio/aac"),
        "flac" => Some("audio/flac"),
        "mp4" | "m4v" => Some("video/mp4"),
        "webm" => Some("video/webm"),
        "mov" => Some("video/quicktime"),
        _ => None,
    }
}

impl AppState {
    /// Where this entity's attachments live: the bridge's own store, always,
    /// and the checkout its agent reads from, when it has one.
    ///
    /// Both, because the two homes answer different questions and neither
    /// answers the other's. A conversation outlives its checkouts —
    /// implementations get archived — so the
    /// durable copy has to sit somewhere Build owns, or a screenshot from last
    /// week renders as a broken image. But a sandboxed harness can only be
    /// relied on to open paths inside its own tree, so the copy the AGENT is
    /// told about has to be worktree-relative. One upload writes both.
    pub(in crate::app) fn attachment_homes(
        &self,
        entity_id: &str,
    ) -> Result<AttachmentHomes, String> {
        let local = self.local_attachments_dir();
        if let Some(active) = self.plans.get(entity_id) {
            // The Issue owns the conversation, but a live implementation owns
            // the checkout its agent reads from — the same redirection
            // `thread.post` makes when it decides whom to nudge.
            let worktree = self
                .current_issue_implementation_id(entity_id)
                .and_then(|run_id| self.runs.get(&run_id).map(|run| run.worktree.path.clone()))
                .or_else(|| active.workspace.as_ref().map(|w| w.checkout.clone()));
            return Ok(AttachmentHomes {
                worktree: worktree.map(|path| path.join(ATTACHMENTS_DIR)),
                local,
            });
        }
        if let Some(active) = self.runs.get(entity_id) {
            return Ok(AttachmentHomes {
                worktree: Some(active.worktree.path.join(ATTACHMENTS_DIR)),
                local,
            });
        }
        Err("unknown conversation owner".to_string())
    }

    /// Where attachments go for an entity that has no checkout to put them in.
    pub(in crate::app) fn local_attachments_dir(&self) -> std::path::PathBuf {
        self.store
            .as_ref()
            .map(|store| store.attachments_dir())
            .unwrap_or_else(|| self.worktrees_root.join("attachments"))
    }

    /// Take one file the reviewer is sending with a message and put it where the
    /// agent can open it. Content-addressed, so re-sending the same screenshot
    /// costs one copy rather than one per send.
    ///
    /// Writing is deliberately separate from posting: the bytes are on disk and
    /// verified before the message that references them exists, so a message can
    /// never point at an upload that failed halfway.
    pub(crate) fn thread_attach(&mut self, params: &Value) -> Result<Value, String> {
        let entity_id = require_str(params, "entity_id")?;
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
        let homes = self.attachment_homes(&entity_id)?;
        if let Some(worktree_home) = &homes.worktree {
            self.refuse_writers_while_reserved(worktree_home)?;
        }
        let name = sanitize_attachment_name(&filename);
        // Content-addressed, so re-sending the same screenshot costs one copy
        // rather than one per send, and the two homes agree on the leaf name —
        // which is what lets a read fall back to the durable copy after a
        // worktree is swept.
        let stored = format!("{}-{name}", &sha256_hex(&content)[..12]);

        write_attachment(&homes.local, &stored, &content)?;
        let wire_path = match &homes.worktree {
            Some(worktree_home) => {
                write_attachment(worktree_home, &stored, &content)?;
                // The agent's own `git add -A` runs in this tree, and so does
                // the diff the human reviews. Conversation is neither.
                ensure_attachments_ignored(
                    worktree_home
                        .parent()
                        .expect("a worktree home is always .build/attachments"),
                )
                .map_err(|e| format!("cannot keep attachments out of git: {e}"))?;
                format!("{ATTACHMENTS_DIR}/{stored}")
            }
            None => homes.local.join(&stored).display().to_string(),
        };
        let head = &content[..content.len().min(8192)];
        Ok(json!({
            "name": name,
            "path": wire_path,
            "mime": mime_hint(std::path::Path::new(&stored), head),
            "size": content.len(),
        }))
    }

    /// Hand an attachment's bytes back to the surface that sent it. The browser
    /// cannot reach the disk, and routing the read through the entity means no
    /// caller has to know (or can get wrong) which checkout the file landed in.
    pub(crate) fn thread_attachment(&self, params: &Value) -> Result<Value, String> {
        let entity_id = require_str(params, "entity_id")?;
        let path = require_str(params, "path")?;
        let target = self.resolve_attachment(&entity_id, &path)?;
        let content =
            std::fs::read(&target).map_err(|e| format!("cannot read the attachment: {e}"))?;
        let head = &content[..content.len().min(8192)];
        Ok(json!({
            "path": path,
            "size": content.len(),
            "mime": mime_hint(&target, head),
            "content_b64": b64encode(&content),
        }))
    }

    /// Resolve a client-supplied attachment path to a real file, refusing
    /// anything that is not one of this entity's own attachments.
    ///
    /// A message body is reviewer input and so is this path, so the check is
    /// containment after canonicalization — not a prefix match on the string —
    /// or an "attachment" is an arbitrary-file read primitive with a nice name.
    ///
    /// A path recorded against a worktree that has since been swept still
    /// resolves: both homes store the file under the same content-addressed
    /// leaf, so the durable copy answers for the one that is gone.
    pub(in crate::app) fn resolve_attachment(
        &self,
        entity_id: &str,
        path: &str,
    ) -> Result<std::path::PathBuf, String> {
        let candidate = std::path::Path::new(path);
        let leaf = candidate.file_name().map(std::path::PathBuf::from);
        for home in self.attachment_homes(entity_id)?.in_read_order() {
            let Ok(canonical_home) = std::fs::canonicalize(&home) else {
                continue;
            };
            // A worktree path is written relative to the worktree, which is the
            // attachments folder's own parent twice over; the leaf retry is
            // what covers a home the path was not written against.
            let worktree_relative = home
                .parent()
                .and_then(|p| p.parent())
                .map(|worktree| worktree.join(candidate));
            let attempts = [
                candidate.is_absolute().then(|| candidate.to_path_buf()),
                worktree_relative.filter(|_| !candidate.is_absolute()),
                leaf.as_ref().map(|leaf| home.join(leaf)),
            ];
            for attempt in attempts.into_iter().flatten() {
                let Ok(resolved) = std::fs::canonicalize(&attempt) else {
                    continue;
                };
                if resolved.starts_with(&canonical_home) && resolved.is_file() {
                    return Ok(resolved);
                }
            }
        }
        Err(format!("not an attachment on this conversation: {path}"))
    }

    /// The files a `thread.post` says it is sending. Name and size are re-read
    /// from disk rather than trusted: the client's copy is a display hint, and
    /// the record the agent reads should describe the bytes that exist.
    pub(in crate::app) fn parse_message_attachments(
        &self,
        entity_id: &str,
        params: &Value,
    ) -> Result<Vec<crate::thread::MessageAttachment>, String> {
        let Some(value) = params.get("attachments").filter(|v| !v.is_null()) else {
            return Ok(Vec::new());
        };
        let listed = value.as_array().ok_or("attachments must be an array")?;
        if listed.len() > ATTACHMENTS_PER_MESSAGE_MAX {
            return Err(format!(
                "a message carries at most {ATTACHMENTS_PER_MESSAGE_MAX} attachments"
            ));
        }
        listed
            .iter()
            .map(|entry| {
                let path = entry
                    .get("path")
                    .and_then(Value::as_str)
                    .ok_or("each attachment needs a path")?;
                let resolved = self.resolve_attachment(entity_id, path)?;
                let size = std::fs::metadata(&resolved)
                    .map_err(|e| format!("cannot stat the attachment: {e}"))?
                    .len();
                // The same head `thread.attach` typed the file from, so a name
                // with no extension resolves to the same mime on the record as
                // it did in the upload's answer.
                let mut head = Vec::new();
                std::fs::File::open(&resolved)
                    .map_err(|e| format!("cannot read the attachment: {e}"))?
                    .take(8192)
                    .read_to_end(&mut head)
                    .map_err(|e| format!("cannot read the attachment: {e}"))?;
                let name = entry
                    .get("name")
                    .and_then(Value::as_str)
                    .map(sanitize_attachment_name)
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| sanitize_attachment_name(path));
                Ok(crate::thread::MessageAttachment {
                    name,
                    path: path.to_string(),
                    mime: mime_hint(&resolved, &head).to_string(),
                    size,
                })
            })
            .collect()
    }
}

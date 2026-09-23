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
//! reads this disk through its own shell, so where a file lives is not policed
//! (Zech, #116 review). What IS policed is what the bridge does with it:
//!
//! - It is opened once, without following a link and without blocking, and
//!   everything after that asks the open handle: a path swapped for a link or
//!   a FIFO between a look and a read has nothing to swap, and a FIFO named
//!   `run.log` is refused rather than waited on.
//! - Media is what its bytes say it is, not what its name says; text is UTF-8.
//! - Every file of a call is opened and checked before any is copied, so a
//!   refused call leaves the store as it found it.
//!
//! An absolute path is always the file at that path. A path that names an
//! attachment already in the store — or, relatively, one the user sent the
//! agent — is passed through untouched, as it always was.

use super::AppState;
use crate::app::conversations::{
    mime_hint, sanitize_attachment_name, write_attachment, ATTACHMENTS_PER_MESSAGE_MAX,
};
use crate::app::sha256_hex;
use serde_json::{json, Value};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// The largest file an agent may attach. Recordings are the reason it is well
/// past a user upload's cap: a user's file crosses the DataChannel in one
/// message, and an agent's is read back in pieces (`issues.attachment` ranges).
pub const AGENT_ATTACHMENT_MAX_BYTES: u64 = 50 * 1_048_576;

/// What a text file may be typed as. HTML and SVG are text too, and are left
/// out on purpose: a file an issue carries is never markup a reader renders.
const TEXT_MIMES: [&str; 3] = ["text/plain", "text/markdown", "application/json"];

/// How many leading bytes the media sniff reads.
const MAGIC_BYTES: usize = 16;

/// One media kind: the extensions that claim it, what to call it in a
/// refusal, and the bytes that prove it.
struct Media {
    extensions: &'static [&'static str],
    label: &'static str,
    is: fn(&[u8]) -> bool,
}

const MEDIA: [Media; 6] = [
    Media {
        extensions: &["png"],
        label: "PNG image",
        is: |head| head.starts_with(b"\x89PNG\r\n\x1a\n"),
    },
    Media {
        extensions: &["jpg", "jpeg"],
        label: "JPEG image",
        is: |head| head.starts_with(b"\xff\xd8\xff"),
    },
    Media {
        extensions: &["gif"],
        label: "GIF image",
        is: |head| head.starts_with(b"GIF87a") || head.starts_with(b"GIF89a"),
    },
    Media {
        extensions: &["webp"],
        label: "WebP image",
        is: |head| head.starts_with(b"RIFF") && head.get(8..12) == Some(b"WEBP"),
    },
    Media {
        extensions: &["mp4"],
        label: "MP4 video",
        is: |head| head.get(4..8) == Some(b"ftyp"),
    },
    Media {
        extensions: &["webm"],
        label: "WebM video",
        is: |head| head.starts_with(b"\x1a\x45\xdf\xa3"),
    },
];

/// One entry of a call, after it has been checked and before anything is
/// copied.
enum Intake {
    /// Already an attachment: handed on as it came.
    PassThrough(Value),
    /// A file the agent made, open and checked, waiting to be copied.
    Copy {
        entry: Value,
        path: String,
        file: File,
    },
}

impl AppState {
    /// The attachments an agent's issue tool named, with every file the agent
    /// made copied into the store and renamed to the copy. What comes out is
    /// what `parse_issue_attachments` reads, the same as a client's list.
    ///
    /// All or nothing: the count, and every file's type, size and contents,
    /// are checked before the first copy is written.
    pub(in crate::app) fn take_in_agent_files(
        &self,
        listed: &[Value],
    ) -> Result<Vec<Value>, String> {
        if listed.len() > ATTACHMENTS_PER_MESSAGE_MAX {
            return Err(format!(
                "Build cannot attach more than {ATTACHMENTS_PER_MESSAGE_MAX} files at once."
            ));
        }
        let checked = listed
            .iter()
            .map(|entry| self.check_agent_file(entry))
            .collect::<Result<Vec<_>, _>>()?;
        checked
            .into_iter()
            .map(|intake| self.copy_in(intake))
            .collect()
    }

    fn check_agent_file(&self, entry: &Value) -> Result<Intake, String> {
        let Some(path) = entry.get("path").and_then(Value::as_str) else {
            return Err("Build cannot attach a file without its path.".to_string());
        };
        let source = Path::new(path);
        if !source.is_absolute() {
            // Relative is only ever an attachment the agent was sent, named
            // against its checkout; the bridge does not guess at a folder.
            return self
                .resolve_issue_attachment(path)
                .map(|_| Intake::PassThrough(entry.clone()))
                .map_err(|_| format!("Build cannot attach {path}: give the file's full path."));
        }
        if self.names_a_stored_file(source) {
            return Ok(Intake::PassThrough(entry.clone()));
        }
        Ok(Intake::Copy {
            entry: entry.clone(),
            path: path.to_string(),
            file: checked_agent_file(path)?,
        })
    }

    /// Whether an absolute path names a file in the store itself — its folder
    /// is the store's. Never by leaf alone: a file elsewhere that happens to
    /// share a stored file's name is a different file.
    fn names_a_stored_file(&self, source: &Path) -> bool {
        let folder = |path: &Path| std::fs::canonicalize(path).ok();
        let store = folder(&self.local_attachments_dir());
        let parent = source.parent().and_then(folder);
        store.is_some() && store == parent && source.is_file()
    }

    fn copy_in(&self, intake: Intake) -> Result<Value, String> {
        match intake {
            Intake::PassThrough(entry) => Ok(entry),
            Intake::Copy { entry, path, file } => self.copy_file_in(entry, &path, file),
        }
    }

    fn copy_file_in(&self, entry: Value, path: &str, mut file: File) -> Result<Value, String> {
        let content = read_from_start(&mut file).map_err(|_| cannot_be_read(path))?;
        if content.len() as u64 > AGENT_ATTACHMENT_MAX_BYTES {
            // It grew between the check and the copy.
            return Err(over_the_cap());
        }
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

/// Open the file an agent named and check it from the handle: a regular file,
/// within the cap, not empty, and what it claims to be.
fn checked_agent_file(path: &str) -> Result<File, String> {
    let source = Path::new(path);
    let mut file = open_without_following(source).map_err(|error| unopenable(path, &error))?;
    let metadata = file.metadata().map_err(|_| cannot_be_read(path))?;
    if metadata.is_dir() {
        return Err(format!(
            "Build cannot attach {path}: it is a folder, not a file."
        ));
    }
    if !metadata.is_file() {
        return Err(format!(
            "Build cannot attach {path}: it is not a regular file."
        ));
    }
    if metadata.len() > AGENT_ATTACHMENT_MAX_BYTES {
        return Err(over_the_cap());
    }
    if metadata.len() == 0 {
        return Err(format!("Build cannot attach {path}: it is empty."));
    }
    check_contents(path, source, &mut file)?;
    Ok(file)
}

/// Media by its magic bytes, anything else as UTF-8 text.
fn check_contents(path: &str, source: &Path, file: &mut File) -> Result<(), String> {
    if let Some(media) = media_named(source) {
        let head = read_head(file).map_err(|_| cannot_be_read(path))?;
        return if (media.is)(&head) {
            Ok(())
        } else {
            Err(format!(
                "Build cannot attach {path}: it is not a {}.",
                media.label
            ))
        };
    }
    let content = read_from_start(file).map_err(|_| cannot_be_read(path))?;
    if content.len() as u64 > AGENT_ATTACHMENT_MAX_BYTES {
        return Err(over_the_cap());
    }
    if is_plain_text(source, &content) {
        return Ok(());
    }
    Err(format!(
        "Build cannot attach {path}: only images (png, jpg, webp, gif), videos (mp4, webm) and plain text or logs can be attached."
    ))
}

/// The one open. No link is followed, so the file checked is the file named;
/// and it never blocks, so a FIFO answers at once and is then refused by the
/// handle's own type.
#[cfg(unix)]
fn open_without_following(source: &Path) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(source)
}

#[cfg(not(unix))]
fn open_without_following(source: &Path) -> std::io::Result<File> {
    File::open(source)
}

fn over_the_cap() -> String {
    format!(
        "Build cannot attach a file larger than {} MB.",
        AGENT_ATTACHMENT_MAX_BYTES / 1_048_576
    )
}

fn cannot_be_read(path: &str) -> String {
    format!("Build cannot attach {path}: it cannot be read.")
}

fn unopenable(path: &str, error: &std::io::Error) -> String {
    #[cfg(unix)]
    if error.raw_os_error() == Some(libc::ELOOP) {
        return format!("Build cannot attach {path}: it is a link, not a file.");
    }
    match error.kind() {
        std::io::ErrorKind::NotFound => {
            format!("Build cannot attach {path}: there is no file there.")
        }
        _ => cannot_be_read(path),
    }
}

/// The whole file from its first byte, and at most one byte past the cap, so
/// a file that grows under the read is caught without reading all of it.
fn read_from_start(file: &mut File) -> std::io::Result<Vec<u8>> {
    file.seek(SeekFrom::Start(0))?;
    let mut content = Vec::new();
    file.take(AGENT_ATTACHMENT_MAX_BYTES + 1)
        .read_to_end(&mut content)?;
    Ok(content)
}

fn read_head(file: &mut File) -> std::io::Result<Vec<u8>> {
    file.seek(SeekFrom::Start(0))?;
    let mut head = Vec::new();
    file.take(MAGIC_BYTES as u64).read_to_end(&mut head)?;
    Ok(head)
}

fn media_named(source: &Path) -> Option<&'static Media> {
    let extension = source.extension()?.to_str()?.to_lowercase();
    MEDIA
        .iter()
        .find(|media| media.extensions.contains(&extension.as_str()))
}

fn is_plain_text(source: &Path, content: &[u8]) -> bool {
    TEXT_MIMES.contains(&mime_hint(source, content))
        && !content.contains(&0)
        && std::str::from_utf8(content).is_ok()
}

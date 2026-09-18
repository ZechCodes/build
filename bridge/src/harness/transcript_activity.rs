use super::{ActivityReport, AgentActivity};
use serde_json::Value;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::{Mutex, Weak};
use std::time::Duration;
use tokio::sync::broadcast;

const MAX_TRANSCRIPT_READ: u64 = 2 * 1024 * 1024;

#[derive(Clone, Copy, Default)]
struct Cursor {
    offset: u64,
    skipping_oversized: bool,
    identity: Option<(u64, u64)>,
}

#[cfg(unix)]
fn file_identity(metadata: &std::fs::Metadata) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    Some((metadata.dev(), metadata.ino()))
}

#[cfg(not(unix))]
fn file_identity(_metadata: &std::fs::Metadata) -> Option<(u64, u64)> {
    None
}

pub(super) type TerminalAlive = Weak<Mutex<Option<broadcast::Sender<Vec<u8>>>>>;

/// Follow only newly appended, complete JSONL records from one exact provider
/// transcript. A resumed transcript starts at EOF, preventing old compactions
/// from being replayed into a new Build session.
pub(super) fn follow(
    resolve: impl Fn() -> Option<PathBuf> + Send + 'static,
    resumed: bool,
    terminal_alive: TerminalAlive,
    parse: fn(&Value) -> Option<bool>,
) -> broadcast::Receiver<ActivityReport> {
    let (tx, rx) = broadcast::channel(32);
    let initial_path = resolve();
    let initial_metadata = initial_path
        .as_ref()
        .and_then(|path| std::fs::metadata(path).ok());
    let initial_offset = if resumed {
        initial_metadata
            .as_ref()
            .map(|metadata| metadata.len())
            .unwrap_or(0)
    } else {
        0
    };
    std::thread::spawn(move || {
        let mut path = initial_path;
        let mut cursor = Cursor {
            offset: initial_offset,
            skipping_oversized: false,
            identity: initial_metadata.as_ref().and_then(file_identity),
        };
        let mut snapshot_when_found = resumed && path.is_none();
        while terminal_alive
            .upgrade()
            .is_some_and(|live| live.lock().unwrap().is_some())
        {
            if path.is_none() {
                path = resolve();
                if snapshot_when_found {
                    let metadata = path.as_ref().and_then(|path| std::fs::metadata(path).ok());
                    cursor.offset = metadata
                        .as_ref()
                        .map(|metadata| metadata.len())
                        .unwrap_or(0);
                    cursor.identity = metadata.as_ref().and_then(file_identity);
                    snapshot_when_found = path.is_none();
                }
            }
            if let Some(current) = path.as_ref() {
                cursor = read_complete_lines(current, cursor, |value| {
                    if let Some(completed) = parse(value) {
                        let _ = tx.send(ActivityReport::own_work(AgentActivity::Compaction {
                            completed,
                        }));
                    }
                });
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    });
    rx
}

pub(crate) fn follow_sidecar(
    path: PathBuf,
    terminal_alive: TerminalAlive,
) -> broadcast::Receiver<ActivityReport> {
    follow(
        move || Some(path.clone()),
        false,
        terminal_alive,
        |value| {
            (value["type"] == "compaction")
                .then(|| value["completed"].as_bool())
                .flatten()
        },
    )
}

fn read_complete_lines(path: &PathBuf, cursor: Cursor, mut visit: impl FnMut(&Value)) -> Cursor {
    let Ok(mut file) = File::open(path) else {
        return cursor;
    };
    let metadata = file.metadata().ok();
    let identity = metadata.as_ref().and_then(file_identity);
    let replaced = cursor.identity.is_some() && cursor.identity != identity;
    let truncated = metadata
        .as_ref()
        .is_some_and(|metadata| metadata.len() < cursor.offset);
    let skipping_oversized = cursor.skipping_oversized && !replaced && !truncated;
    let offset = metadata
        .as_ref()
        .filter(|metadata| !replaced && metadata.len() >= cursor.offset)
        .map(|_| cursor.offset)
        .unwrap_or(0);
    if file.seek(SeekFrom::Start(offset)).is_err() {
        return Cursor::default();
    }
    let mut appended = Vec::new();
    if file
        .take(MAX_TRANSCRIPT_READ)
        .read_to_end(&mut appended)
        .is_err()
    {
        return Cursor {
            offset,
            identity,
            skipping_oversized,
        };
    }
    let start = if skipping_oversized {
        match appended.iter().position(|byte| *byte == b'\n') {
            Some(newline) => newline + 1,
            None => {
                return Cursor {
                    offset: offset + appended.len() as u64,
                    skipping_oversized: true,
                    identity,
                }
            }
        }
    } else {
        0
    };
    let complete_bytes = appended
        .get(start..)
        .unwrap_or_default()
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(start, |at| start + at + 1);
    for line in appended[start..complete_bytes].split(|byte| *byte == b'\n') {
        if line.is_empty() {
            continue;
        }
        if let Ok(value) = serde_json::from_slice(line) {
            visit(&value);
        }
    }
    if complete_bytes == start && appended.len() as u64 == MAX_TRANSCRIPT_READ {
        Cursor {
            offset: offset + appended.len() as u64,
            skipping_oversized: true,
            identity,
        }
    } else {
        Cursor {
            offset: offset + complete_bytes as u64,
            skipping_oversized: false,
            identity,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::Arc;

    fn compacted(value: &Value) -> Option<bool> {
        (value["type"] == "compacted").then_some(true)
    }

    #[test]
    fn partial_records_are_held_until_their_newline_arrives() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("transcript.jsonl");
        std::fs::write(&path, "{\"type\":\"compacted\"").unwrap();
        let mut seen = Vec::new();
        let cursor =
            read_complete_lines(&path, Cursor::default(), |value| seen.push(value.clone()));
        assert_eq!(cursor.offset, 0);
        assert!(seen.is_empty());
        std::fs::write(&path, "{\"type\":\"compacted\"}\n").unwrap();
        let cursor = read_complete_lines(&path, cursor, |value| seen.push(value.clone()));
        assert_eq!(seen.len(), 1);
        assert_eq!(cursor.offset, std::fs::metadata(path).unwrap().len());
    }

    #[test]
    fn resumed_followers_skip_history_and_emit_each_new_cycle_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("transcript.jsonl");
        std::fs::write(&path, "{\"type\":\"compacted\"}\n").unwrap();
        let live = Arc::new(Mutex::new(Some(broadcast::channel(1).0)));
        let followed = path.clone();
        let mut rx = follow(
            move || Some(followed.clone()),
            true,
            Arc::downgrade(&live),
            compacted,
        );
        std::thread::sleep(Duration::from_millis(150));
        assert!(matches!(
            rx.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));

        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(file, "{{\"type\":\"compacted\"}}").unwrap();
        writeln!(file, "{{\"type\":\"compacted\"}}").unwrap();
        std::thread::sleep(Duration::from_millis(250));
        assert_eq!(rx.try_recv().unwrap().activity.summary(), "Compacted");
        assert_eq!(rx.try_recv().unwrap().activity.summary(), "Compacted");
        assert!(matches!(
            rx.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
        *live.lock().unwrap() = None;
    }

    #[test]
    fn sidecar_reports_both_compaction_boundaries() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("compaction.jsonl");
        std::fs::write(&path, "").unwrap();
        let live = Arc::new(Mutex::new(Some(broadcast::channel(1).0)));
        let mut rx = follow_sidecar(path.clone(), Arc::downgrade(&live));
        std::fs::write(
            &path,
            "{\"type\":\"compaction\",\"completed\":false}\n{\"type\":\"compaction\",\"completed\":true}\n",
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(250));
        assert_eq!(rx.try_recv().unwrap().activity.summary(), "Compacting");
        assert_eq!(rx.try_recv().unwrap().activity.summary(), "Compacted");
        *live.lock().unwrap() = None;
    }

    #[test]
    fn truncation_resets_the_cursor() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("transcript.jsonl");
        std::fs::write(&path, "{\"type\":\"old\"}\n").unwrap();
        let old_end = std::fs::metadata(&path).unwrap().len();
        std::fs::write(&path, "{\"type\":\"compacted\"}\n").unwrap();
        let mut seen = Vec::new();
        let new_end = read_complete_lines(
            &path,
            Cursor {
                offset: old_end + 100,
                skipping_oversized: false,
                identity: None,
            },
            |value| seen.push(value.clone()),
        );
        assert_eq!(seen.len(), 1);
        assert!(new_end.offset > 0);
    }

    #[test]
    fn an_oversized_record_cannot_block_later_compactions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("transcript.jsonl");
        let huge = format!(
            "{{\"type\":\"message\",\"body\":\"{}\"}}\n",
            "x".repeat(MAX_TRANSCRIPT_READ as usize)
        );
        std::fs::write(&path, format!("{huge}{{\"type\":\"compacted\"}}\n")).unwrap();
        let mut cursor = Cursor::default();
        let mut seen = Vec::new();
        for _ in 0..3 {
            cursor = read_complete_lines(&path, cursor, |value| {
                if compacted(value).is_some() {
                    seen.push(value.clone());
                }
            });
        }
        assert_eq!(seen.len(), 1);
    }
}

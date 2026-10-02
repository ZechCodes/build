//! Bounded rendering of a saved commit diff for review reads.

use super::{
    canonical_patch_options, delta_path, fnv1a64_fold, is_mcp_config, map_status, ChangedFile,
    DiffError, DiffPaths, DiffStat, FNV_OFFSET_BASIS, LARGE_FILE_BYTES,
};
use crate::body_page::{page_len, BodyRange, BodySpan, BODY_PAGE_MAX_BYTES};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::Path;

pub(crate) const MAX_REVIEW_FILE_ROWS: usize = 1_000;

pub(crate) struct SavedCommitDiff {
    pub stat: DiffStat,
    pub files: Vec<ChangedFile>,
    pub files_truncated: bool,
    pub patch: Option<String>,
    pub truncated: bool,
    pub range: Option<BodySpan>,
    #[cfg(test)]
    pub buffered_patch_bytes: usize,
}

pub(crate) fn diff_between_saved_commits_bounded(
    git_dir: &Path,
    base_sha: Option<&str>,
    head_sha: &str,
    paths: DiffPaths<'_>,
    include_patch: bool,
    range: Option<BodyRange>,
) -> Result<SavedCommitDiff, DiffError> {
    let repo = git2::Repository::open_bare(git_dir)?;
    let base_tree = base_sha
        .map(|sha| {
            let oid = git2::Oid::from_str(sha)?;
            repo.find_commit(oid)?.tree()
        })
        .transpose()?;
    let head = repo.find_commit(git2::Oid::from_str(head_sha)?)?;
    let head_tree = head.tree()?;
    let mut opts = canonical_patch_options();
    paths.narrow(&mut opts);
    opts.max_size(LARGE_FILE_BYTES as i64);
    let diff = repo.diff_tree_to_tree(base_tree.as_ref(), Some(&head_tree), Some(&mut opts))?;
    render_saved_diff(&diff, include_patch, range)
}

fn render_saved_diff(
    diff: &git2::Diff<'_>,
    include_patch: bool,
    range: Option<BodyRange>,
) -> Result<SavedCommitDiff, DiffError> {
    let mut rows = SavedRows::new(diff, include_patch, range);
    diff.print(git2::DiffFormat::Patch, |delta, _hunk, line| {
        rows.record(&delta, &line)
    })?;
    Ok(rows.finish())
}

struct SavedRows {
    files: Vec<ChangedFile>,
    files_changed: usize,
    row_of: HashMap<String, usize>,
    keys: Vec<u64>,
    insertions: usize,
    deletions: usize,
    patch: SavedPatchBuffer,
}

impl SavedRows {
    fn new(diff: &git2::Diff<'_>, include_patch: bool, range: Option<BodyRange>) -> Self {
        let mut files = Vec::new();
        let mut files_changed = 0;
        for delta in diff.deltas() {
            let path = delta_path(&delta);
            if is_mcp_config(&path) {
                continue;
            }
            files_changed += 1;
            if files.len() < MAX_REVIEW_FILE_ROWS {
                files.push(ChangedFile {
                    path,
                    status: map_status(delta.status()),
                    additions: 0,
                    deletions: 0,
                    content_key: String::new(),
                });
            }
        }
        let row_of = files
            .iter()
            .enumerate()
            .map(|(index, file)| (file.path.clone(), index))
            .collect();
        let keys = vec![FNV_OFFSET_BASIS; files.len()];
        Self {
            files,
            files_changed,
            row_of,
            keys,
            insertions: 0,
            deletions: 0,
            patch: SavedPatchBuffer::new(include_patch, range),
        }
    }

    fn record(&mut self, delta: &git2::DiffDelta<'_>, line: &git2::DiffLine<'_>) -> bool {
        let path = delta_path(delta);
        if is_mcp_config(&path) {
            return true;
        }
        let index = self.row_of.get(&path).copied();
        self.count_line(index, line.origin());
        self.capture_line(index, line);
        true
    }

    fn count_line(&mut self, index: Option<usize>, origin: char) {
        match origin {
            '+' => {
                self.insertions += 1;
                if let Some(index) = index {
                    self.files[index].additions += 1;
                }
            }
            '-' => {
                self.deletions += 1;
                if let Some(index) = index {
                    self.files[index].deletions += 1;
                }
            }
            _ => {}
        }
    }

    fn capture_line(&mut self, index: Option<usize>, line: &git2::DiffLine<'_>) {
        if matches!(line.origin(), '+' | '-' | ' ') {
            let origin = [line.origin() as u8];
            if self.patch.include_patch {
                self.patch.push(&origin);
            }
            if let Some(index) = index {
                self.keys[index] = fnv1a64_fold(self.keys[index], &origin);
            }
        }
        if let Some(index) = index {
            self.keys[index] = fnv1a64_fold(self.keys[index], line.content());
        }
        if self.patch.include_patch {
            self.patch
                .push(String::from_utf8_lossy(line.content()).as_bytes());
        }
    }

    fn finish(mut self) -> SavedCommitDiff {
        for (file, key) in self.files.iter_mut().zip(self.keys) {
            file.content_key = format!("{key:016x}");
        }
        let (patch, range, truncated, buffered_patch_bytes) = self.patch.finish();
        #[cfg(not(test))]
        let _ = buffered_patch_bytes;
        SavedCommitDiff {
            stat: DiffStat {
                files_changed: self.files_changed,
                insertions: self.insertions,
                deletions: self.deletions,
            },
            files: self.files,
            files_truncated: self.files_changed > MAX_REVIEW_FILE_ROWS,
            patch,
            truncated,
            range,
            #[cfg(test)]
            buffered_patch_bytes,
        }
    }
}

struct SavedPatchBuffer {
    include_patch: bool,
    range: Option<BodyRange>,
    start: usize,
    stop: usize,
    bytes: Vec<u8>,
    total: usize,
    digest: Sha256,
}

impl SavedPatchBuffer {
    fn new(include_patch: bool, range: Option<BodyRange>) -> Self {
        let (start, stop) = match (include_patch, range) {
            (true, Some(range)) => {
                let offset = usize::try_from(range.offset).unwrap_or(usize::MAX);
                (
                    offset.saturating_sub(3),
                    offset.saturating_add(range.capacity()),
                )
            }
            (true, None) => (0, BODY_PAGE_MAX_BYTES as usize),
            (false, _) => (0, 0),
        };
        Self {
            include_patch,
            range,
            start,
            stop,
            bytes: Vec::new(),
            total: 0,
            digest: Sha256::new(),
        }
    }

    fn push(&mut self, bytes: &[u8]) {
        let end = self.total.saturating_add(bytes.len());
        if self.range.is_some() {
            self.digest.update(bytes);
        }
        if self.include_patch && end > self.start && self.total < self.stop {
            let from = self.start.max(self.total) - self.total;
            let to = self.stop.min(end) - self.total;
            self.bytes.extend_from_slice(&bytes[from..to]);
        }
        self.total = end;
    }

    fn finish(self) -> (Option<String>, Option<BodySpan>, bool, usize) {
        let buffered = self.bytes.len();
        if !self.include_patch {
            return (None, None, false, buffered);
        }
        match self.range {
            Some(range) => self.finish_page(range),
            None => {
                let valid = std::str::from_utf8(&self.bytes)
                    .map_or_else(|error| error.valid_up_to(), str::len);
                (
                    Some(
                        String::from_utf8(self.bytes[..valid].to_vec())
                            .expect("patch prefix is UTF-8"),
                    ),
                    None,
                    valid < self.total,
                    buffered,
                )
            }
        }
    }

    fn finish_page(self, range: BodyRange) -> (Option<String>, Option<BodySpan>, bool, usize) {
        let buffered = self.bytes.len();
        let mut offset = usize::try_from(range.offset)
            .unwrap_or(usize::MAX)
            .min(self.total);
        let version = self.digest.finalize()[..8]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        if offset == self.total {
            let span = BodySpan {
                offset: offset as u64,
                end: offset as u64,
                total: self.total as u64,
                version: Some(version),
            };
            return (Some(String::new()), Some(span), false, buffered);
        }
        while self.bytes[offset - self.start] & 0b1100_0000 == 0b1000_0000 {
            offset -= 1;
        }
        let window_end = offset.saturating_add(range.capacity()).min(self.total);
        let from = offset - self.start;
        let to = window_end - self.start;
        let window = &self.bytes[from..to];
        let len = page_len(window, window_end == self.total);
        let body = String::from_utf8(window[..len].to_vec()).expect("patch page is UTF-8");
        let span = BodySpan {
            offset: offset as u64,
            end: (offset + len) as u64,
            total: self.total as u64,
            version: Some(version),
        };
        (Some(body), Some(span), false, buffered)
    }
}

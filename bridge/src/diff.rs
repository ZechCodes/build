//! Git is the integration layer. The bridge never asks a harness what it did —
//! the worktree knows.
//!
//! This module renders `git diff` of a task's worktree against its base branch in
//! two shapes: a cheap **summary** (files changed, +/- lines) for the quiet
//! progress state while building, and the **full patch** for the review gate. It
//! also answers the plan-phase enforcement question — *did anything change
//! outside `.build/`?* — so the UI can flag a planning agent that wrote code.

use std::path::Path;
use std::time::Duration;

use tokio::sync::mpsc;

/// Where plan-phase work is supposed to stay confined.
pub const PLAN_SCOPE_PREFIX: &str = ".build/";

/// A file larger than this is binary as far as both diff paths are concerned:
/// libgit2 renders it as `Binary files … differ` (no lines), and the stat path
/// counts no lines for it. One threshold on both paths is what keeps the cheap
/// stat equal to the rendered patch's numbers — and keeps a stray 200 MB log
/// file out of the review surface.
pub const LARGE_FILE_BYTES: u64 = 4 * 1024 * 1024;

/// A worktree path's filesystem modification time as Unix milliseconds.
/// Missing paths (most commonly deletions) have no honest edit time to report.
pub(crate) fn file_edited_at(worktree_root: &Path, path: &str) -> Option<u64> {
    std::fs::symlink_metadata(worktree_root.join(path))
        .ok()?
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_millis()).ok())
}

/// Roll-up counts for the quiet progress state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DiffStat {
    pub files_changed: usize,
    pub insertions: usize,
    pub deletions: usize,
}

impl DiffStat {
    /// The counts as the wire object every surface reads them in.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "files_changed": self.files_changed,
            "insertions": self.insertions,
            "deletions": self.deletions,
        })
    }
}

/// How a single path changed relative to base.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
    Other,
}

/// One changed path in the worktree's delta from base.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedFile {
    pub path: String,
    pub status: ChangeStatus,
}

/// The worktree's complete delta from its base branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeDiff {
    stat: DiffStat,
    files: Vec<ChangedFile>,
    patch: String,
}

#[derive(Debug, thiserror::Error)]
pub enum DiffError {
    #[error("git error: {0}")]
    Git(#[from] git2::Error),
}

impl WorktreeDiff {
    /// The roll-up counts (the building-state progress fact).
    pub fn stat(&self) -> DiffStat {
        self.stat
    }

    /// The changed files, in diff order.
    pub fn files(&self) -> &[ChangedFile] {
        &self.files
    }

    /// The full unified patch (the review-gate surface).
    pub fn patch(&self) -> &str {
        &self.patch
    }

    /// Changed paths that fall outside `prefix`. Used for plan-phase enforcement
    /// by observation: a planning agent should only touch `.build/`.
    pub fn paths_outside<'a>(&'a self, prefix: &str) -> Vec<&'a str> {
        self.files
            .iter()
            .map(|f| f.path.as_str())
            .filter(|p| !p.starts_with(prefix))
            .collect()
    }

    /// Whether the planning agent strayed outside `.build/`.
    pub fn touched_outside_plan_scope(&self) -> bool {
        !self.paths_outside(PLAN_SCOPE_PREFIX).is_empty()
    }
}

/// Compute the worktree's diff against `base_branch`.
///
/// Compares the base branch's tree to the worktree's working directory *and*
/// index, so it captures committed, staged, and unstaged changes alike — the
/// total delta a reviewer should see, regardless of how the agent committed.
pub fn diff_against_base(
    worktree_path: &Path,
    base_branch: &str,
) -> Result<WorktreeDiff, DiffError> {
    let repo = git2::Repository::open(worktree_path)?;
    let base_tree = base_tree(&repo, base_branch)?;
    diff_tree_to_dirty_workdir(&repo, Some(&base_tree))
}

/// [`diff_against_base`]'s counts alone, without rendering the patch. This is
/// the poll-surface entry point: the numbers cost a tree walk, not a full
/// patch of every file in the worktree.
pub fn stat_against_base(worktree_path: &Path, base_branch: &str) -> Result<DiffStat, DiffError> {
    let repo = git2::Repository::open(worktree_path)?;
    let base_tree = base_tree(&repo, base_branch)?;
    stat_tree_to_dirty_workdir(&repo, Some(&base_tree))
}

fn base_tree<'repo>(
    repo: &'repo git2::Repository,
    base_branch: &str,
) -> Result<git2::Tree<'repo>, DiffError> {
    Ok(repo.revparse_single(base_branch)?.peel_to_tree()?)
}

/// The worktree's total delta from its fork point with `base_branch`: the
/// merge-base tree vs the working directory *and* index, untracked included —
/// committed, staged, unstaged, and new files together. This is the browse/
/// review surface for external worktrees, which may long predate the base tip;
/// `diff_against_base` (anchored on the run's `base_sha`) remains the run-diff
/// surface.
pub fn diff_against_merge_base(
    worktree_path: &Path,
    base_branch: &str,
) -> Result<WorktreeDiff, DiffError> {
    let repo = git2::Repository::open(worktree_path)?;
    let merge_base_tree = merge_base_tree(&repo, base_branch)?;
    diff_tree_to_dirty_workdir(&repo, Some(&merge_base_tree))
}

/// [`diff_against_merge_base`]'s counts alone, without rendering the patch.
pub fn stat_against_merge_base(
    worktree_path: &Path,
    base_branch: &str,
) -> Result<DiffStat, DiffError> {
    let repo = git2::Repository::open(worktree_path)?;
    let merge_base_tree = merge_base_tree(&repo, base_branch)?;
    stat_tree_to_dirty_workdir(&repo, Some(&merge_base_tree))
}

fn merge_base_tree<'repo>(
    repo: &'repo git2::Repository,
    base_branch: &str,
) -> Result<git2::Tree<'repo>, DiffError> {
    let base_commit = repo.revparse_single(base_branch)?.peel_to_commit()?;
    let head_commit = repo.head()?.peel_to_commit()?;
    let merge_base_id = repo.merge_base(base_commit.id(), head_commit.id())?;
    Ok(repo.find_commit(merge_base_id)?.tree()?)
}

/// One local branch's total delta from its fork point with `base_branch`, by
/// tree alone — no working directory or index involved, so this reads safely
/// for ANY branch, not only the one presently checked out. The branch-switcher
/// list uses this: every branch's own weight, not just the checked-out one's.
pub fn stat_branch_against_base(
    worktree_path: &Path,
    branch: &str,
    base_branch: &str,
) -> Result<DiffStat, DiffError> {
    let repo = git2::Repository::open(worktree_path)?;
    let branch_commit = repo.revparse_single(branch)?.peel_to_commit()?;
    let base_commit = repo.revparse_single(base_branch)?.peel_to_commit()?;
    let merge_base_id = repo.merge_base(branch_commit.id(), base_commit.id())?;
    let merge_base_tree = repo.find_commit(merge_base_id)?.tree()?;
    let branch_tree = branch_commit.tree()?;
    let diff = repo.diff_tree_to_tree(Some(&merge_base_tree), Some(&branch_tree), None)?;
    diff_stat_without_rendering(&repo, &diff)
}

/// Render an immutable commit-to-commit range. Inputs must be full object ids,
/// not revspecs: callers resolve only persisted stage boundaries through this
/// helper, so later HEAD movement and dirty files cannot alter the result.
pub fn diff_between_commits(
    worktree_path: &Path,
    start_sha: &str,
    completion_sha: &str,
) -> Result<WorktreeDiff, DiffError> {
    let repo = git2::Repository::open(worktree_path)?;
    let start = repo.find_commit(git2::Oid::from_str(start_sha)?)?;
    let completion = repo.find_commit(git2::Oid::from_str(completion_sha)?)?;
    let start_tree = start.tree()?;
    let completion_tree = completion.tree()?;
    let diff = repo.diff_tree_to_tree(Some(&start_tree), Some(&completion_tree), None)?;
    worktree_diff_from_git_diff(&diff)
}

/// Shared tail of both diff entry points: `old_tree` vs the worktree's dirty
/// working directory and index (untracked included).
/// The scaffolded per-owner MCP config: machine-local plumbing, never the
/// user's work — excluded from every review surface. One per agent
/// (`.build/mcp-<agent_id>.json`), plus the pre-agent `.build/mcp.json` still
/// sitting in worktrees scaffolded by an older build.
pub(crate) const MCP_CONFIG_PATH: &str = ".build/mcp.json";

/// Whether a path is one of those configs.
pub(crate) fn is_mcp_config(path: &str) -> bool {
    path == MCP_CONFIG_PATH || (path.starts_with(".build/mcp-") && path.ends_with(".json"))
}

fn delta_path(delta: &git2::DiffDelta) -> String {
    delta
        .new_file()
        .path()
        .or_else(|| delta.old_file().path())
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The options both dirty-workdir paths share. `with_untracked_content` is the
/// one difference: the review surface loads new files so it can print them, the
/// stat surface never does — it counts their lines off disk instead.
fn dirty_workdir_options(with_untracked_content: bool) -> git2::DiffOptions {
    let mut opts = git2::DiffOptions::new();
    opts.include_untracked(true)
        .recurse_untracked_dirs(true)
        .show_untracked_content(with_untracked_content)
        .max_size(LARGE_FILE_BYTES as i64);
    opts
}

fn diff_tree_to_dirty_workdir(
    repo: &git2::Repository,
    old_tree: Option<&git2::Tree>,
) -> Result<WorktreeDiff, DiffError> {
    let mut opts = dirty_workdir_options(true);
    let diff = repo.diff_tree_to_workdir_with_index(old_tree, Some(&mut opts))?;
    worktree_diff_from_git_diff(&diff)
}

/// A cheap identity for the complete delta against a fixed base. It walks the
/// delta but never asks libgit2 to load untracked bodies or print patch lines.
/// Tracked/index changes carry their object ids; working-tree and untracked
/// changes also carry filesystem size and nanosecond mtime, matching the
/// metadata keys used by the git status surface.
pub fn key_against_base(worktree_path: &Path, base_branch: &str) -> Result<String, DiffError> {
    let repo = git2::Repository::open(worktree_path)?;
    let tree = base_tree(&repo, base_branch)?;
    dirty_diff_key(&repo, Some(&tree))
}

pub fn key_against_merge_base(
    worktree_path: &Path,
    base_branch: &str,
) -> Result<String, DiffError> {
    let repo = git2::Repository::open(worktree_path)?;
    let tree = merge_base_tree(&repo, base_branch)?;
    dirty_diff_key(&repo, Some(&tree))
}

fn dirty_diff_key(
    repo: &git2::Repository,
    old_tree: Option<&git2::Tree<'_>>,
) -> Result<String, DiffError> {
    let mut opts = dirty_workdir_options(false);
    let diff = repo.diff_tree_to_workdir_with_index(old_tree, Some(&mut opts))?;
    let root = repo.workdir().unwrap_or_else(|| repo.path());
    let mut material = String::new();
    if let Some(tree) = old_tree {
        material.push_str(&tree.id().to_string());
    }
    for delta in diff.deltas() {
        let path = delta_path(&delta);
        if is_mcp_config(&path) {
            continue;
        }
        let old = delta.old_file();
        let new = delta.new_file();
        use std::fmt::Write as _;
        let _ = write!(
            material,
            "\0{:?}\0{}\0{}\0{}\0{:?}\0{:?}",
            delta.status(),
            path,
            old.id(),
            new.id(),
            old.mode(),
            new.mode()
        );
        if let Ok(metadata) = std::fs::symlink_metadata(root.join(&path)) {
            let modified_nanos = metadata
                .modified()
                .ok()
                .and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |duration| duration.as_nanos());
            let _ = write!(material, "\0{}\0{modified_nanos}", metadata.len());
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt as _;
                let _ = write!(
                    material,
                    "\0{}\0{}\0{}",
                    metadata.ino(),
                    metadata.ctime(),
                    metadata.ctime_nsec()
                );
            }
        } else {
            material.push_str("\0deleted");
        }
    }
    Ok(fnv1a64_hex(&material))
}

fn stat_tree_to_dirty_workdir(
    repo: &git2::Repository,
    old_tree: Option<&git2::Tree>,
) -> Result<DiffStat, DiffError> {
    let mut opts = dirty_workdir_options(false);
    let diff = repo.diff_tree_to_workdir_with_index(old_tree, Some(&mut opts))?;
    diff_stat_without_rendering(repo, &diff)
}

/// The roll-up counts of `diff`, computed from libgit2's own line stats rather
/// than from a rendered patch.
///
/// Two wrinkles keep the numbers identical to the rendered patch's:
/// * The excluded MCP config's lines are subtracted back out — it must count
///   for nothing, exactly as it prints nothing.
/// * Untracked files carry no content here (that load is the expensive part),
///   so their added lines are counted off disk by [`added_lines`].
fn diff_stat_without_rendering(
    repo: &git2::Repository,
    diff: &git2::Diff<'_>,
) -> Result<DiffStat, DiffError> {
    let stats = diff.stats()?;
    let mut insertions = stats.insertions();
    let mut deletions = stats.deletions();
    let mut files_changed = 0usize;

    for (index, delta) in diff.deltas().enumerate() {
        let path = delta_path(&delta);
        if is_mcp_config(&path) {
            if let Some(patch) = git2::Patch::from_diff(diff, index)? {
                let (_context, added, removed) = patch.line_stats()?;
                insertions = insertions.saturating_sub(added);
                deletions = deletions.saturating_sub(removed);
            }
            continue;
        }
        files_changed += 1;
        if delta.status() == git2::Delta::Untracked {
            if let Some(workdir) = repo.workdir() {
                insertions += added_lines(&workdir.join(&path));
            }
        }
    }

    Ok(DiffStat {
        files_changed,
        insertions,
        deletions,
    })
}

/// How many lines an untracked file adds, counted by streaming its bytes —
/// never by loading it into a patch.
///
/// Returns 0 for everything libgit2 would render as `Binary files … differ`:
/// files past [`LARGE_FILE_BYTES`], files holding a NUL byte or a wide-encoding
/// byte-order mark, and files whose non-printable bytes outweigh their
/// printable ones (libgit2's own text heuristic). A symlink is its target path:
/// one line, never followed.
fn added_lines(path: &Path) -> usize {
    use std::io::BufRead;

    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return 0;
    };
    if metadata.file_type().is_symlink() {
        return 1;
    }
    if !metadata.is_file() || metadata.len() > LARGE_FILE_BYTES {
        return 0;
    }
    let Ok(file) = std::fs::File::open(path) else {
        return 0;
    };

    let mut reader = std::io::BufReader::with_capacity(64 * 1024, file);
    let mut newlines = 0usize;
    let mut printable = 0usize;
    let mut nonprintable = 0usize;
    let mut last_byte = None;
    let mut at_start = true;
    loop {
        let Ok(chunk) = reader.fill_buf() else {
            return 0;
        };
        if chunk.is_empty() {
            break;
        }
        if at_start {
            at_start = false;
            if starts_with_wide_bom(chunk) {
                return 0;
            }
        }
        for &byte in chunk {
            match byte {
                0 => return 0,
                b'\n' => newlines += 1,
                _ => {}
            }
            if byte > 0x1F && byte != 0x7F {
                printable += 1;
            } else if !matches!(byte, b'\t' | b'\n' | 0x0B | 0x0C | b'\r' | b' ') {
                nonprintable += 1;
            }
        }
        last_byte = chunk.last().copied();
        let consumed = chunk.len();
        reader.consume(consumed);
    }
    if (printable >> 7) < nonprintable {
        return 0;
    }
    match last_byte {
        None => 0,
        // A final line without its newline is still an added line.
        Some(b'\n') => newlines,
        Some(_) => newlines + 1,
    }
}

/// A UTF-16/32 byte-order mark, which libgit2 reads as "this is binary".
fn starts_with_wide_bom(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF])
}

// Counts the render path's patch walks so a test can prove the stat path never
// takes it. Thread-local: each test owns its own count.
#[cfg(test)]
thread_local! {
    static PATCH_PRINTS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
fn patch_prints_on_this_thread() -> usize {
    PATCH_PRINTS.with(|count| count.get())
}

fn worktree_diff_from_git_diff(diff: &git2::Diff<'_>) -> Result<WorktreeDiff, DiffError> {
    #[cfg(test)]
    PATCH_PRINTS.with(|count| count.set(count.get() + 1));

    let files: Vec<ChangedFile> = diff
        .deltas()
        .map(|delta| ChangedFile {
            path: delta_path(&delta),
            status: map_status(delta.status()),
        })
        .filter(|file| !is_mcp_config(&file.path))
        .collect();

    // Stats are counted while printing (instead of `diff.stats()`) so the
    // excluded MCP config contributes to neither the patch nor the numbers.
    let mut insertions = 0;
    let mut deletions = 0;
    let mut patch = String::new();
    diff.print(git2::DiffFormat::Patch, |delta, _hunk, line| {
        if is_mcp_config(&delta_path(&delta)) {
            return true;
        }
        match line.origin() {
            '+' => insertions += 1,
            '-' => deletions += 1,
            _ => {}
        }
        if matches!(line.origin(), '+' | '-' | ' ') {
            patch.push(line.origin());
        }
        patch.push_str(&String::from_utf8_lossy(line.content()));
        true
    })?;
    let stat = DiffStat {
        files_changed: files.len(),
        insertions,
        deletions,
    };

    Ok(WorktreeDiff { stat, files, patch })
}

fn map_status(status: git2::Delta) -> ChangeStatus {
    match status {
        git2::Delta::Added | git2::Delta::Untracked | git2::Delta::Copied => ChangeStatus::Added,
        git2::Delta::Modified | git2::Delta::Typechange => ChangeStatus::Modified,
        git2::Delta::Deleted => ChangeStatus::Deleted,
        git2::Delta::Renamed => ChangeStatus::Renamed,
        _ => ChangeStatus::Other,
    }
}

// ---- hunk identity ---------------------------------------------------------

/// One hunk of a unified patch, carrying the identity review prioritization
/// keys on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchHunk {
    /// `h` + 12 hex digits — see [`patch_hunks`] for how it is derived.
    pub hunk_id: String,
    /// The file the hunk belongs to (the `b/` side of its `diff --git` line).
    pub path: String,
    /// The hunk's `@@` header line, verbatim.
    pub header: String,
}

/// The FNV-1a 64 offset basis and prime.
const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// FNV-1a 64 over the UTF-8 bytes of `text`, as 16 lowercase hex digits.
///
/// Identity, not integrity. A cryptographic digest would have to be reachable
/// from the browser too, and the only one there (`crypto.subtle`) is async —
/// hunk ids have to be assignable inside a synchronous render.
pub(crate) fn fnv1a64_hex(text: &str) -> String {
    let mut hash = FNV_OFFSET_BASIS;
    for byte in text.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    format!("{hash:016x}")
}

/// A patch's lines, without the empty tail a trailing newline leaves behind.
fn patch_lines(patch: &str) -> Vec<&str> {
    let mut lines: Vec<&str> = patch.split('\n').collect();
    if lines.last() == Some(&"") {
        lines.pop();
    }
    lines
}

/// The path a `diff --git a/x b/x` line is about: its `b/` side.
fn diff_header_path(line: &str) -> Option<String> {
    let path = &line[line.find(" b/")? + 3..];
    (!path.is_empty()).then(|| path.to_string())
}

/// The line count of one `@@` range (`12,7` → `7`; a bare `12` → `1`, git's
/// own convention). `None` for anything that is not a range.
fn range_count(range: &str) -> Option<String> {
    let (start, count) = match range.split_once(',') {
        Some((start, count)) => (start, count.to_string()),
        None => (range, "1".to_string()),
    };
    let digits = |text: &str| !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit());
    (digits(start) && digits(&count)).then_some(count)
}

/// A hunk header reduced to its line counts: `@@ -12,7 +14,9 @@ fn x()` becomes
/// `-7 +9`. Dropping the start lines is what lets a hunk that only MOVED keep
/// its id. `None` when the line is not a well-formed hunk header.
fn normalized_hunk_header(line: &str) -> Option<String> {
    let rest = line.strip_prefix("@@ ")?;
    let ranges = &rest[..rest.find(" @@")?];
    let mut ranges = ranges.split(' ');
    let old = range_count(ranges.next()?.strip_prefix('-')?)?;
    let new = range_count(ranges.next()?.strip_prefix('+')?)?;
    if ranges.next().is_some() {
        return None;
    }
    Some(format!("-{old} +{new}"))
}

/// A hunk before its id is assigned.
struct RawHunk {
    path: String,
    header: String,
    normalized: String,
    body: Vec<String>,
}

/// Split a patch into its hunks, in patch order.
fn raw_hunks(patch: &str) -> Vec<RawHunk> {
    let mut hunks: Vec<RawHunk> = Vec::new();
    let mut path: Option<String> = None;
    let mut in_hunk = false;
    for line in patch_lines(patch) {
        if line.starts_with("diff --git") {
            path = diff_header_path(line);
            in_hunk = false;
        } else if line.starts_with("@@") {
            in_hunk = false;
            if let (Some(path), Some(normalized)) = (path.clone(), normalized_hunk_header(line)) {
                hunks.push(RawHunk {
                    path,
                    header: line.to_string(),
                    normalized,
                    body: Vec::new(),
                });
                in_hunk = true;
            }
        } else if in_hunk {
            if let Some(hunk) = hunks.last_mut() {
                hunk.body.push(line.to_string());
            }
        }
    }
    hunks
}

/// Assign every hunk in `patch` a stable id.
///
/// The id is a short hash of three things and nothing else:
///
/// 1. the file path (the `b/` side of the `diff --git` line),
/// 2. the hunk header normalized to its line counts (`-7 +9`), so a hunk that
///    only moved within its file keeps its id,
/// 3. the hunk's body lines, verbatim, one per line.
///
/// Those are joined with newlines, hashed with FNV-1a 64, and rendered as `h`
/// plus the first 12 hex digits. Two identical hunks in one file would hash
/// alike, so the second and later occurrences fold their occurrence number into
/// the hashed material (`\n#1`, `\n#2`, …) — ids are unique within a patch.
///
/// `spa/src/core/diff.js`'s `patchHunks` is a port of this function, and
/// `bridge/tests/fixtures/hunk_ids.json` is the shared fixture that keeps the
/// two honest. Change one, change the other, and regenerate the fixture.
pub fn patch_hunks(patch: &str) -> Vec<PatchHunk> {
    let mut occurrences: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    raw_hunks(patch)
        .into_iter()
        .map(|raw| {
            let material = format!("{}\n{}\n{}", raw.path, raw.normalized, raw.body.join("\n"));
            let seen = occurrences.entry(material.clone()).or_insert(0);
            let hashed = if *seen == 0 {
                material.clone()
            } else {
                format!("{material}\n#{seen}")
            };
            *seen += 1;
            PatchHunk {
                hunk_id: format!("h{}", &fnv1a64_hex(&hashed)[..12]),
                path: raw.path,
                header: raw.header,
            }
        })
        .collect()
}

/// [`patch_hunks`]' ids alone, in patch order — the vocabulary a triage report
/// is checked against.
pub fn hunk_ids(patch: &str) -> Vec<String> {
    patch_hunks(patch)
        .into_iter()
        .map(|hunk| hunk.hunk_id)
        .collect()
}

/// A live, debounced stream of recomputed diffs for a worktree. Holds the fs
/// watcher and the worker thread alive; dropping it stops watching.
pub struct DiffWatcher {
    _watcher: notify::RecommendedWatcher,
}

/// Begin watching `worktree_path`; every burst of filesystem changes is debounced
/// by `debounce`, then a freshly recomputed [`WorktreeDiff`] is sent on the
/// returned channel. The first diff is sent immediately so subscribers start with
/// current state.
pub fn watch(
    worktree_path: &Path,
    base_branch: &str,
    debounce: Duration,
) -> Result<(DiffWatcher, mpsc::UnboundedReceiver<WorktreeDiff>), DiffError> {
    use notify::Watcher;

    let (raw_tx, raw_rx) = std::sync::mpsc::channel::<()>();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if res.is_ok() {
            let _ = raw_tx.send(());
        }
    })
    .map_err(notify_to_git)?;
    watcher
        .watch(worktree_path, notify::RecursiveMode::Recursive)
        .map_err(notify_to_git)?;

    let (diff_tx, diff_rx) = mpsc::unbounded_channel();

    // Send the current diff straight away so a subscriber starts from truth.
    if let Ok(initial) = diff_against_base(worktree_path, base_branch) {
        let _ = diff_tx.send(initial);
    }

    let worktree_path = worktree_path.to_path_buf();
    let base_branch = base_branch.to_string();
    std::thread::spawn(move || {
        // Block for the first event of a burst, then drain until quiet for
        // `debounce`, recompute once, and emit.
        while raw_rx.recv().is_ok() {
            while raw_rx.recv_timeout(debounce).is_ok() {}
            match diff_against_base(&worktree_path, &base_branch) {
                Ok(diff) => {
                    if diff_tx.send(diff).is_err() {
                        break; // receiver dropped
                    }
                }
                Err(_) => continue,
            }
        }
    });

    Ok((DiffWatcher { _watcher: watcher }, diff_rx))
}

/// notify and git2 errors don't share a type; carry the message through git2's.
fn notify_to_git(err: notify::Error) -> DiffError {
    DiffError::Git(git2::Error::from_str(&err.to_string()))
}

/// The primary checkout's uncommitted delta: HEAD's tree vs the working
/// directory and index, untracked included — staged + unstaged + new files.
/// This is the "main worktree" review surface; committed work is upstream's
/// business, not a review surface.
pub fn diff_against_head(repo_path: &Path) -> Result<WorktreeDiff, DiffError> {
    let repo = git2::Repository::open(repo_path)?;
    let head_tree = repo.head()?.peel_to_tree()?;
    diff_tree_to_dirty_workdir(&repo, Some(&head_tree))
}

/// [`diff_against_head`]'s counts alone, without rendering the patch.
pub fn stat_against_head(repo_path: &Path) -> Result<DiffStat, DiffError> {
    let repo = git2::Repository::open(repo_path)?;
    let head_tree = repo.head()?.peel_to_tree()?;
    stat_tree_to_dirty_workdir(&repo, Some(&head_tree))
}

/// Like [`diff_against_head`], but an unborn HEAD (a repo with no commits yet)
/// diffs against the empty tree instead of failing — the git-GUI status
/// surface must keep working in a brand-new repository.
pub fn diff_uncommitted(repo_path: &Path) -> Result<WorktreeDiff, DiffError> {
    let repo = git2::Repository::open(repo_path)?;
    let head_tree = head_tree_if_born(&repo)?;
    diff_tree_to_dirty_workdir(&repo, head_tree.as_ref())
}

/// [`diff_uncommitted`]'s counts alone, without rendering the patch.
pub fn stat_uncommitted(repo_path: &Path) -> Result<DiffStat, DiffError> {
    let repo = git2::Repository::open(repo_path)?;
    let head_tree = head_tree_if_born(&repo)?;
    stat_tree_to_dirty_workdir(&repo, head_tree.as_ref())
}

/// One changed path's line counts, without its patch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDelta {
    pub path: String,
    pub added: usize,
    pub deleted: usize,
    pub binary: bool,
}

/// One changed path's rendered patch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilePatch {
    pub path: String,
    pub patch: String,
}

/// Every uncommitted path's line counts, in one walk that renders no patch.
///
/// This is [`stat_uncommitted`]'s census per file rather than rolled up — the
/// same [`LARGE_FILE_BYTES`] threshold, the same off-disk count for untracked
/// content, the same excluded MCP config — so the list's totals ARE that stat.
pub fn uncommitted_file_deltas(repo_path: &Path) -> Result<Vec<FileDelta>, DiffError> {
    let repo = git2::Repository::open(repo_path)?;
    let head_tree = head_tree_if_born(&repo)?;
    let mut opts = dirty_workdir_options(false);
    let diff = repo.diff_tree_to_workdir_with_index(head_tree.as_ref(), Some(&mut opts))?;
    let mut deltas = Vec::new();
    for (index, delta) in diff.deltas().enumerate() {
        let path = delta_path(&delta);
        if is_mcp_config(&path) {
            continue;
        }
        deltas.push(if delta.status() == git2::Delta::Untracked {
            untracked_file_delta(repo.workdir(), path)
        } else {
            tracked_file_delta(&diff, index, path)?
        });
    }
    Ok(deltas)
}

/// An untracked file's lines, counted off disk because its content was never
/// loaded into the diff. A file with bytes but no countable line is what
/// libgit2 would print as `Binary files … differ`.
fn untracked_file_delta(workdir: Option<&Path>, path: String) -> FileDelta {
    let absolute = workdir.map(|root| root.join(&path));
    let added = absolute.as_deref().map(added_lines).unwrap_or(0);
    let bytes = absolute
        .and_then(|file| std::fs::symlink_metadata(file).ok())
        .map_or(0, |metadata| metadata.len());
    FileDelta {
        path,
        added,
        deleted: 0,
        binary: added == 0 && bytes > 0,
    }
}

/// A tracked path's lines, from libgit2's own count for that delta. A delta
/// libgit2 will not hand over line by line is binary.
fn tracked_file_delta(
    diff: &git2::Diff<'_>,
    index: usize,
    path: String,
) -> Result<FileDelta, DiffError> {
    let Some(patch) = git2::Patch::from_diff(diff, index)? else {
        return Ok(FileDelta {
            path,
            added: 0,
            deleted: 0,
            binary: true,
        });
    };
    let (_context, added, deleted) = patch.line_stats()?;
    Ok(FileDelta {
        path,
        added,
        deleted,
        binary: patch.delta().flags().is_binary(),
    })
}

/// The uncommitted patch of each of `paths`, rendered one file at a time.
///
/// An untracked file prints as all additions, a deleted one as all deletions,
/// and the machine-local MCP config prints not at all. Paths match literally,
/// never as globs, so a path holding `*` means the file with that name. A path
/// with no uncommitted change — and a binary one, which has no lines to print
/// — is absent from the answer.
pub fn patch_for_paths(repo_path: &Path, paths: &[String]) -> Result<Vec<FilePatch>, DiffError> {
    let repo = git2::Repository::open(repo_path)?;
    let head_tree = head_tree_if_born(&repo)?;
    let mut opts = dirty_workdir_options(true);
    opts.disable_pathspec_match(true);
    for path in paths {
        opts.pathspec(path);
    }
    let diff = repo.diff_tree_to_workdir_with_index(head_tree.as_ref(), Some(&mut opts))?;
    let mut rendered = Vec::new();
    for (index, delta) in diff.deltas().enumerate() {
        let path = delta_path(&delta);
        if is_mcp_config(&path) {
            continue;
        }
        let Some(mut patch) = git2::Patch::from_diff(&diff, index)? else {
            continue;
        };
        rendered.push(FilePatch {
            path,
            patch: String::from_utf8_lossy(&patch.to_buf()?).into_owned(),
        });
    }
    Ok(rendered)
}

/// HEAD's tree, or `None` in a repository that has no commits yet.
fn head_tree_if_born(repo: &git2::Repository) -> Result<Option<git2::Tree<'_>>, DiffError> {
    match repo.head() {
        Ok(head) => Ok(Some(head.peel_to_tree()?)),
        Err(e)
            if matches!(
                e.code(),
                git2::ErrorCode::UnbornBranch | git2::ErrorCode::NotFound
            ) =>
        {
            Ok(None)
        }
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use crate::git_fixture::{git_in, init_repo_with_readme};
    use crate::git_process::run_git;

    #[test]
    fn a_diffstat_goes_on_the_wire_as_its_three_counts() {
        let stat = DiffStat {
            files_changed: 4,
            insertions: 80,
            deletions: 9,
        };
        assert_eq!(
            stat.to_json(),
            serde_json::json!({ "files_changed": 4, "insertions": 80, "deletions": 9 })
        );
    }

    /// Everything the cheap stat and the rendered patch must agree on: a
    /// tracked modification, a tracked deletion, a staged addition, untracked
    /// text without a trailing newline, an untracked binary, an untracked file
    /// past [`LARGE_FILE_BYTES`], and the excluded MCP config.
    fn mixed_fixture() -> (tempfile::TempDir, PathBuf) {
        let (dir, repo) = init_repo();
        std::fs::write(repo.join("tracked-delete.txt"), "gone\nlines\n").unwrap();
        std::fs::write(repo.join("tracked-modify.txt"), "one\ntwo\n").unwrap();
        git_in(&repo, &["add", "."]);
        git_in(&repo, &["commit", "-m", "fixture"]);

        std::fs::write(repo.join("tracked-modify.txt"), "one\ntwo\nthree\n").unwrap();
        std::fs::remove_file(repo.join("tracked-delete.txt")).unwrap();
        std::fs::write(repo.join("staged-add.txt"), "staged\n").unwrap();
        git_in(&repo, &["add", "staged-add.txt"]);
        std::fs::write(repo.join("untracked.txt"), "alpha\nbeta\nno-newline").unwrap();
        std::fs::write(repo.join("untracked.bin"), [0u8, 1, 2, 0, 255, b'\n']).unwrap();
        std::fs::write(
            repo.join("huge.txt"),
            "x\n".repeat(LARGE_FILE_BYTES as usize / 2 + 1),
        )
        .unwrap();
        std::fs::create_dir_all(repo.join(".build")).unwrap();
        std::fs::write(repo.join(".build/mcp.json"), "{\n\"mcpServers\": {}\n}\n").unwrap();
        (dir, repo)
    }

    #[test]
    fn stat_equals_the_rendered_patch_numbers() {
        let (_dir, repo) = mixed_fixture();

        let stat = stat_uncommitted(&repo).unwrap();
        // Six changed paths; the MCP config is not one of them.
        assert_eq!(stat.files_changed, 6, "{stat:?}");
        // +1 modified line, +1 staged line, +3 untracked text lines; the binary
        // and the oversize file contribute nothing. -2 for the deletion.
        assert_eq!(stat.insertions, 5, "{stat:?}");
        assert_eq!(stat.deletions, 2, "{stat:?}");

        assert_eq!(stat, diff_uncommitted(&repo).unwrap().stat());
        assert_eq!(
            stat_against_head(&repo).unwrap(),
            diff_against_head(&repo).unwrap().stat()
        );
        assert_eq!(
            stat_against_base(&repo, "main").unwrap(),
            diff_against_base(&repo, "main").unwrap().stat()
        );
        assert_eq!(
            stat_against_merge_base(&repo, "main").unwrap(),
            diff_against_merge_base(&repo, "main").unwrap().stat()
        );
    }

    #[test]
    fn the_stat_path_never_runs_the_patch_printer() {
        let (_dir, repo) = mixed_fixture();
        assert_eq!(patch_prints_on_this_thread(), 0);

        stat_uncommitted(&repo).unwrap();
        stat_against_head(&repo).unwrap();
        stat_against_base(&repo, "main").unwrap();
        stat_against_merge_base(&repo, "main").unwrap();
        assert_eq!(
            patch_prints_on_this_thread(),
            0,
            "the stat path rendered a patch"
        );

        // The spy is wired up: the render path does move it.
        diff_uncommitted(&repo).unwrap();
        assert_eq!(patch_prints_on_this_thread(), 1);
    }

    #[test]
    fn a_conditional_key_avoids_patch_rendering_and_follows_metadata() {
        let (_dir, repo) = init_repo();
        std::fs::write(repo.join("README.md"), "changed\n").unwrap();
        let path = repo.join("README.md");
        let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.set_times(
            std::fs::FileTimes::new().set_modified(
                std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000),
            ),
        )
        .unwrap();

        let first = key_against_base(&repo, "main").unwrap();
        assert_eq!(patch_prints_on_this_thread(), 0);
        file.set_times(
            std::fs::FileTimes::new().set_modified(
                std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_001),
            ),
        )
        .unwrap();
        let touched = key_against_base(&repo, "main").unwrap();

        assert_ne!(first, touched);
        assert_eq!(patch_prints_on_this_thread(), 0);
    }

    /// The per-file census is the same census as the roll-up: the file list
    /// carries one entry per changed path, and the entries add up to the stat.
    #[test]
    fn file_deltas_carry_each_paths_lines_and_sum_to_the_stat() {
        let (_dir, repo) = mixed_fixture();

        let deltas = uncommitted_file_deltas(&repo).unwrap();
        let by_path = |path: &str| -> FileDelta {
            deltas
                .iter()
                .find(|delta| delta.path == path)
                .unwrap_or_else(|| panic!("{path} missing from {deltas:?}"))
                .clone()
        };

        assert!(
            !deltas.iter().any(|delta| delta.path.contains("mcp.json")),
            "the MCP config counts for nothing: {deltas:?}"
        );
        assert_eq!(by_path("tracked-modify.txt").added, 1);
        assert_eq!(by_path("tracked-modify.txt").deleted, 0);
        assert!(!by_path("tracked-modify.txt").binary);
        assert_eq!(by_path("tracked-delete.txt").deleted, 2);
        assert_eq!(by_path("staged-add.txt").added, 1);
        assert_eq!(by_path("untracked.txt").added, 3);
        assert!(by_path("untracked.bin").binary);
        assert_eq!(by_path("untracked.bin").added, 0);
        assert!(by_path("huge.txt").binary);

        let stat = stat_uncommitted(&repo).unwrap();
        assert_eq!(deltas.len(), stat.files_changed);
        assert_eq!(
            deltas.iter().map(|delta| delta.added).sum::<usize>(),
            stat.insertions
        );
        assert_eq!(
            deltas.iter().map(|delta| delta.deleted).sum::<usize>(),
            stat.deletions
        );
    }

    #[test]
    fn a_patch_is_rendered_for_the_asked_paths_alone() {
        let (_dir, repo) = mixed_fixture();

        let patches = patch_for_paths(
            &repo,
            &[
                "untracked.txt".to_string(),
                "tracked-delete.txt".to_string(),
            ],
        )
        .unwrap();

        let paths: Vec<&str> = patches.iter().map(|file| file.path.as_str()).collect();
        assert_eq!(paths, vec!["tracked-delete.txt", "untracked.txt"]);
        let untracked = &patches[1].patch;
        assert!(untracked.contains("diff --git a/untracked.txt b/untracked.txt"));
        assert!(untracked.contains("+alpha"));
        assert!(
            !untracked.contains("+one"),
            "a path nobody asked for: {untracked}"
        );
        assert!(patches[0].patch.contains("-gone"));
    }

    #[test]
    fn the_mcp_config_is_never_rendered_even_when_asked_for() {
        let (_dir, repo) = mixed_fixture();

        assert!(patch_for_paths(&repo, &[".build/mcp.json".to_string()])
            .unwrap()
            .is_empty());
    }

    /// A pathspec is a literal path, never a glob: asking for `*` asks for the
    /// file actually named `*`.
    #[test]
    fn a_pathspec_never_globs() {
        let (_dir, repo) = mixed_fixture();

        assert!(patch_for_paths(&repo, &["*".to_string()])
            .unwrap()
            .is_empty());
    }

    #[test]
    fn an_unchanged_path_renders_no_patch() {
        let (_dir, repo) = mixed_fixture();

        assert!(patch_for_paths(&repo, &["README.md".to_string()])
            .unwrap()
            .is_empty());
    }

    #[test]
    fn an_untracked_symlink_counts_as_its_target_path() {
        let (_dir, repo) = init_repo();
        std::os::unix::fs::symlink("README.md", repo.join("link.md")).unwrap();

        let rendered = diff_uncommitted(&repo).unwrap();
        assert_eq!(rendered.files().len(), 1, "{:?}", rendered.files());
        assert_eq!(stat_uncommitted(&repo).unwrap(), rendered.stat());
    }

    #[test]
    fn stat_excludes_the_scaffolded_mcp_config_even_once_it_is_tracked() {
        let (_dir, repo) = init_repo();
        std::fs::create_dir_all(repo.join(".build")).unwrap();
        std::fs::write(repo.join(".build/mcp.json"), "{\n}\n").unwrap();
        git_in(&repo, &["add", "."]);
        git_in(&repo, &["commit", "-m", "mcp"]);

        // A tracked change to the config contributes to neither count.
        std::fs::write(repo.join(".build/mcp.json"), "{\n\"a\": 1\n}\n").unwrap();
        std::fs::write(repo.join("visible.txt"), "real work\n").unwrap();

        let stat = stat_uncommitted(&repo).unwrap();
        assert_eq!(stat.files_changed, 1, "{stat:?}");
        assert_eq!(stat.insertions, 1, "{stat:?}");
        assert_eq!(stat.deletions, 0, "{stat:?}");
        assert_eq!(stat, diff_uncommitted(&repo).unwrap().stat());
    }

    /// The shared fixture with a two-line README, so every diff below is
    /// read against the same first commit.
    fn init_repo() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let repo = init_repo_with_readme(
            dir.path(),
            "repo",
            "# project
line
",
        );
        (dir, repo)
    }

    #[test]
    fn no_changes_is_an_empty_diff() {
        let (_dir, repo) = init_repo();
        let diff = diff_against_base(&repo, "main").unwrap();
        assert_eq!(diff.stat(), DiffStat::default());
        assert!(diff.files().is_empty());
        assert!(diff.patch().is_empty());
    }

    #[test]
    fn untracked_and_modified_files_are_summarized() {
        let (_dir, repo) = init_repo();
        // Modify a tracked file and add a new untracked one.
        std::fs::write(repo.join("README.md"), "# project\nline\nadded\n").unwrap();
        std::fs::write(repo.join("new.txt"), "hello\nworld\n").unwrap();

        let diff = diff_against_base(&repo, "main").unwrap();
        let stat = diff.stat();
        assert_eq!(stat.files_changed, 2);
        assert!(stat.insertions >= 3, "got {stat:?}");

        let paths: Vec<&str> = diff.files().iter().map(|f| f.path.as_str()).collect();
        assert!(paths.contains(&"README.md"));
        assert!(paths.contains(&"new.txt"));
        // The patch is the full review surface.
        assert!(diff.patch().contains("new.txt"));
        assert!(diff.patch().contains("+hello"));
    }

    #[test]
    fn dirty_diff_excludes_the_scaffolded_mcp_config() {
        let (_dir, repo) = init_repo();
        // Adoption scaffolds the machine-local MCP config into the worktree; it
        // is plumbing, not the user's work, and must never reach the review
        // surface.
        std::fs::create_dir_all(repo.join(".build")).unwrap();
        std::fs::write(repo.join(".build/mcp.json"), "{\"mcpServers\":{}}\n").unwrap();
        std::fs::write(repo.join("visible.txt"), "real work\n").unwrap();

        let diff = diff_against_base(&repo, "main").unwrap();
        let paths: Vec<&str> = diff.files().iter().map(|f| f.path.as_str()).collect();
        assert!(paths.contains(&"visible.txt"), "{paths:?}");
        assert!(!paths.contains(&".build/mcp.json"), "{paths:?}");
        assert_eq!(diff.stat().files_changed, 1, "{:?}", diff.stat());
        assert!(!diff.patch().contains("mcp.json"));
    }

    #[test]
    fn committed_changes_on_the_branch_are_included() {
        let (_dir, repo) = init_repo();
        let git = |args: &[&str]| git_in(&repo, args);
        git(&["checkout", "-b", "build/x"]);
        std::fs::write(repo.join("feature.rs"), "fn main() {}\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-m", "feature"]);

        // Even though it's committed, the delta from base must show it.
        let diff = diff_against_base(&repo, "main").unwrap();
        assert_eq!(diff.stat().files_changed, 1);
        assert_eq!(diff.files()[0].path, "feature.rs");
        assert_eq!(diff.files()[0].status, ChangeStatus::Added);
    }

    #[test]
    fn plan_scope_enforcement_flags_out_of_scope_writes() {
        let (_dir, repo) = init_repo();
        std::fs::create_dir_all(repo.join(".build")).unwrap();
        std::fs::write(repo.join(".build/plan.md"), "# plan\n").unwrap();

        // Only `.build/` touched → nothing out of scope.
        let in_scope = diff_against_base(&repo, "main").unwrap();
        assert!(!in_scope.touched_outside_plan_scope());
        assert!(in_scope.paths_outside(PLAN_SCOPE_PREFIX).is_empty());

        // Now the planning agent writes code it shouldn't have.
        std::fs::write(repo.join("src.rs"), "code\n").unwrap();
        let strayed = diff_against_base(&repo, "main").unwrap();
        assert!(strayed.touched_outside_plan_scope());
        assert_eq!(strayed.paths_outside(PLAN_SCOPE_PREFIX), vec!["src.rs"]);
    }

    #[test]
    fn stat_branch_against_base_reads_a_branch_that_is_not_checked_out() {
        let (_dir, repo) = init_repo();
        let git = |args: &[&str]| git_in(&repo, args);
        git(&["checkout", "-b", "build/x"]);
        std::fs::write(repo.join("feature.rs"), "one\ntwo\nthree\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-m", "feature"]);
        // Not the checked-out branch — main still is — so this reads the tree
        // alone, no working directory involved.
        git(&["checkout", "main"]);

        let stat = stat_branch_against_base(&repo, "build/x", "main").unwrap();
        assert_eq!(stat.files_changed, 1, "{stat:?}");
        assert_eq!(stat.insertions, 3, "{stat:?}");
        assert_eq!(stat.deletions, 0, "{stat:?}");

        // A branch measured against itself has nothing to say.
        let same = stat_branch_against_base(&repo, "main", "main").unwrap();
        assert_eq!(same, DiffStat::default(), "{same:?}");
    }

    #[test]
    fn merge_base_diff_sees_committed_staged_unstaged_and_untracked() {
        let (_dir, repo) = init_repo();
        let git = |args: &[&str]| git_in(&repo, args);
        git(&["checkout", "-b", "build/x"]);
        std::fs::write(repo.join("committed.txt"), "committed\n").unwrap();
        git(&["add", "committed.txt"]);
        git(&["commit", "-m", "committed"]);
        std::fs::write(repo.join("staged.txt"), "staged\n").unwrap();
        git(&["add", "staged.txt"]);
        std::fs::write(repo.join("README.md"), "# project\nline\nmodified\n").unwrap();
        std::fs::write(repo.join("untracked.txt"), "untracked\n").unwrap();

        let diff = diff_against_merge_base(&repo, "main").unwrap();
        let paths: Vec<&str> = diff.files().iter().map(|f| f.path.as_str()).collect();
        assert!(paths.contains(&"committed.txt"), "got {paths:?}");
        assert!(paths.contains(&"staged.txt"), "got {paths:?}");
        assert!(paths.contains(&"README.md"), "got {paths:?}");
        assert!(paths.contains(&"untracked.txt"), "got {paths:?}");
        assert!(diff.patch().contains("+untracked"));
    }

    #[test]
    fn merge_base_diff_ignores_base_movement() {
        let (_dir, repo) = init_repo();
        let git = |args: &[&str]| git_in(&repo, args);
        git(&["checkout", "-b", "build/x"]);
        std::fs::write(repo.join("feature.rs"), "fn main() {}\n").unwrap();
        git(&["add", "feature.rs"]);
        git(&["commit", "-m", "feature"]);

        // Advance main with an unrelated commit after the branch forked.
        git(&["checkout", "main"]);
        std::fs::write(repo.join("upstream.txt"), "upstream\n").unwrap();
        git(&["add", "upstream.txt"]);
        git(&["commit", "-m", "upstream"]);
        git(&["checkout", "build/x"]);

        let diff = diff_against_merge_base(&repo, "main").unwrap();
        assert_eq!(diff.files().len(), 1);
        assert_eq!(diff.files()[0].path, "feature.rs");
    }

    #[test]
    fn merge_base_diff_on_detached_head_works() {
        let (_dir, repo) = init_repo();
        let git = |args: &[&str]| git_in(&repo, args);
        let head_sha = run_git(&repo, &["rev-parse", "HEAD"])
            .unwrap()
            .trim()
            .to_string();
        git(&["checkout", "--detach", &head_sha]);
        std::fs::write(repo.join("dirty.txt"), "dirty\n").unwrap();

        let diff = diff_against_merge_base(&repo, "main").unwrap();
        let paths: Vec<&str> = diff.files().iter().map(|f| f.path.as_str()).collect();
        assert!(paths.contains(&"dirty.txt"), "got {paths:?}");
    }

    #[test]
    fn commit_range_diff_is_stable_after_later_commits_and_dirty_changes() {
        let (_dir, repo) = init_repo();
        let git = |args: &[&str]| run_git(&repo, args).unwrap().trim().to_string();
        let start = git(&["rev-parse", "HEAD"]);
        std::fs::write(repo.join("stage-one.txt"), "one\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-m", "stage one"]);
        let completion = git(&["rev-parse", "HEAD"]);

        let expected = diff_between_commits(&repo, &start, &completion).unwrap();
        std::fs::write(repo.join("stage-two.txt"), "two\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-m", "stage two"]);
        std::fs::write(repo.join("dirty.txt"), "dirty\n").unwrap();
        let after = diff_between_commits(&repo, &start, &completion).unwrap();

        assert_eq!(after, expected);
        assert!(after.patch().contains("stage-one.txt"));
        assert!(!after.patch().contains("stage-two.txt"));
        assert!(!after.patch().contains("dirty.txt"));
    }

    #[test]
    fn diff_against_head_is_empty_on_a_clean_checkout() {
        let (_dir, repo) = init_repo();
        let diff = diff_against_head(&repo).unwrap();
        assert_eq!(diff.stat(), DiffStat::default());
        assert!(diff.files().is_empty());
        assert!(diff.patch().is_empty());
    }

    #[test]
    fn diff_against_head_counts_staged_unstaged_and_untracked() {
        let (_dir, repo) = init_repo();
        let git = |args: &[&str]| git_in(&repo, args);
        // Unstaged modification.
        std::fs::write(repo.join("README.md"), "# project\nline\nmodified\n").unwrap();
        // Staged new file.
        std::fs::write(repo.join("staged.txt"), "staged\n").unwrap();
        git(&["add", "staged.txt"]);
        // Untracked new file.
        std::fs::write(repo.join("untracked.txt"), "untracked\n").unwrap();

        let diff = diff_against_head(&repo).unwrap();
        let paths: Vec<&str> = diff.files().iter().map(|f| f.path.as_str()).collect();
        assert!(paths.contains(&"README.md"), "got {paths:?}");
        assert!(paths.contains(&"staged.txt"), "got {paths:?}");
        assert!(paths.contains(&"untracked.txt"), "got {paths:?}");
        assert_eq!(diff.stat().files_changed, 3, "{:?}", diff.stat());
    }

    #[test]
    fn diff_against_head_excludes_the_scaffolded_mcp_config() {
        let (_dir, repo) = init_repo();
        std::fs::create_dir_all(repo.join(".build")).unwrap();
        std::fs::write(repo.join(".build/mcp.json"), "{\"mcpServers\":{}}\n").unwrap();
        std::fs::write(repo.join("visible.txt"), "real work\n").unwrap();

        let diff = diff_against_head(&repo).unwrap();
        let paths: Vec<&str> = diff.files().iter().map(|f| f.path.as_str()).collect();
        assert!(paths.contains(&"visible.txt"), "{paths:?}");
        assert!(!paths.contains(&".build/mcp.json"), "{paths:?}");
        assert_eq!(diff.stat().files_changed, 1, "{:?}", diff.stat());
        assert!(!diff.patch().contains("mcp.json"));
    }

    /// A two-file patch: an added file and a modification with two hunks.
    const HUNK_FIXTURE_PATCH: &str = "\
diff --git a/greeting.py b/greeting.py
new file mode 100644
index 0000000..e69de29
--- /dev/null
+++ b/greeting.py
@@ -0,0 +1,2 @@
+def hello():
+    return \"Hello!\"
diff --git a/app.py b/app.py
index 1111111..2222222 100644
--- a/app.py
+++ b/app.py
@@ -10,3 +10,4 @@ def main():
 keep_one
-old_line
+new_line
 keep_two
@@ -40,2 +41,3 @@ def other():
 context
+added
";

    #[test]
    fn every_hunk_of_a_patch_gets_its_own_id() {
        let hunks = patch_hunks(HUNK_FIXTURE_PATCH);
        assert_eq!(hunks.len(), 3, "{hunks:?}");
        assert_eq!(hunks[0].path, "greeting.py");
        assert_eq!(hunks[1].path, "app.py");
        assert_eq!(hunks[2].path, "app.py");
        assert_eq!(hunks[1].header, "@@ -10,3 +10,4 @@ def main():");
        let ids: std::collections::HashSet<&str> =
            hunks.iter().map(|hunk| hunk.hunk_id.as_str()).collect();
        assert_eq!(ids.len(), 3, "ids must be unique within a patch: {hunks:?}");
        for hunk in &hunks {
            assert_eq!(hunk.hunk_id.len(), 13, "{hunk:?}");
            assert!(hunk.hunk_id.starts_with('h'), "{hunk:?}");
        }
        assert_eq!(hunk_ids(HUNK_FIXTURE_PATCH).len(), 3);
    }

    #[test]
    fn a_hunk_that_only_moved_keeps_its_id() {
        // Same file, same body, different start lines and section heading: the
        // header normalizes to its counts, so the id must not move with it.
        let before = "\
diff --git a/app.py b/app.py
@@ -10,3 +10,4 @@ def main():
 keep_one
-old_line
+new_line
 keep_two
";
        let after = "\
diff --git a/app.py b/app.py
@@ -80,3 +91,4 @@ def something_else():
 keep_one
-old_line
+new_line
 keep_two
";
        assert_eq!(hunk_ids(before), hunk_ids(after));
    }

    #[test]
    fn the_same_body_in_another_file_or_another_shape_is_another_hunk() {
        let material = |path: &str, header: &str, body: &str| {
            format!("diff --git a/{path} b/{path}\n{header}\n{body}")
        };
        let base = material("app.py", "@@ -1,2 +1,3 @@", " ctx\n+added\n");
        let other_file = material("other.py", "@@ -1,2 +1,3 @@", " ctx\n+added\n");
        let other_counts = material("app.py", "@@ -1,2 +1,4 @@", " ctx\n+added\n");
        let other_body = material("app.py", "@@ -1,2 +1,3 @@", " ctx\n+different\n");
        assert_ne!(hunk_ids(&base), hunk_ids(&other_file));
        assert_ne!(hunk_ids(&base), hunk_ids(&other_counts));
        assert_ne!(hunk_ids(&base), hunk_ids(&other_body));
    }

    #[test]
    fn two_identical_hunks_in_one_file_still_get_distinct_ids() {
        let patch = "\
diff --git a/app.py b/app.py
@@ -1,1 +1,2 @@
 ctx
+added
@@ -30,1 +31,2 @@
 ctx
+added
";
        let ids = hunk_ids(patch);
        assert_eq!(ids.len(), 2);
        assert_ne!(ids[0], ids[1], "duplicate hunks must not collide: {ids:?}");
        // And the disambiguation is stable, not order-of-hashing luck.
        assert_eq!(ids, hunk_ids(patch));
    }

    #[test]
    fn a_malformed_hunk_header_is_not_a_hunk() {
        let patch = "\
diff --git a/app.py b/app.py
@@ not a hunk header @@
 ctx
@@ -1,1 +1,2 @@
 ctx
+added
";
        let hunks = patch_hunks(patch);
        assert_eq!(hunks.len(), 1, "{hunks:?}");
        assert_eq!(hunks[0].header, "@@ -1,1 +1,2 @@");
    }

    #[test]
    fn a_bare_range_counts_as_one_line() {
        // `@@ -1 +1 @@` is git's shorthand for a single-line range.
        assert_eq!(
            normalized_hunk_header("@@ -1 +1 @@").as_deref(),
            Some("-1 +1")
        );
        assert_eq!(
            normalized_hunk_header("@@ -12,7 +14,9 @@ fn x()").as_deref(),
            Some("-7 +9")
        );
        assert_eq!(normalized_hunk_header("@@ -a,b +c,d @@"), None);
    }

    #[test]
    fn hunk_ids_are_computed_from_a_real_worktree_patch() {
        let (_dir, repo) = init_repo();
        std::fs::write(repo.join("README.md"), "# project\nline\nadded\n").unwrap();
        let diff = diff_uncommitted(&repo).unwrap();
        let hunks = patch_hunks(diff.patch());
        assert_eq!(hunks.len(), 1, "{:?}\n{}", hunks, diff.patch());
        assert_eq!(hunks[0].path, "README.md");
    }

    /// The fixture both languages read. The SPA's `test/hunkIds.test.js` asserts
    /// the same ids from `spa/src/core/diff.js`, so a change to either
    /// implementation that does not change the other fails here or there.
    #[test]
    fn the_shared_fixture_pins_the_ids_both_languages_produce() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/hunk_ids.json");
        let raw = std::fs::read_to_string(path).expect("the shared hunk-id fixture");
        let fixture: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let cases = fixture["cases"].as_array().expect("cases");
        assert!(!cases.is_empty());
        for case in cases {
            let name = case["name"].as_str().unwrap();
            let patch = case["patch"].as_str().unwrap();
            let expected: Vec<serde_json::Value> = case["hunks"].as_array().unwrap().clone();
            let actual = patch_hunks(patch);
            assert_eq!(actual.len(), expected.len(), "{name}: {actual:?}");
            for (hunk, want) in actual.iter().zip(expected.iter()) {
                assert_eq!(hunk.hunk_id, want["hunk_id"].as_str().unwrap(), "{name}");
                assert_eq!(hunk.path, want["path"].as_str().unwrap(), "{name}");
                assert_eq!(hunk.header, want["header"].as_str().unwrap(), "{name}");
            }
        }
    }

    #[tokio::test]
    async fn watcher_pushes_a_recomputed_diff_on_change() {
        let (dir, repo) = init_repo();
        // Keep the tempdir alive for the whole test.
        let _keep = &dir;

        let (_watcher, mut rx) = watch(&repo, "main", Duration::from_millis(50)).unwrap();

        // First message is the immediate baseline (empty).
        let initial = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("baseline diff arrives")
            .unwrap();
        assert_eq!(initial.stat(), DiffStat::default());

        // Touch a file; expect a debounced, recomputed diff that sees it.
        std::fs::write(repo.join("changed.txt"), "x\n").unwrap();
        let updated = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("change diff arrives")
            .unwrap();
        assert!(
            updated.files().iter().any(|f| f.path == "changed.txt"),
            "watcher should report the new file, got {:?}",
            updated.files()
        );
    }
}

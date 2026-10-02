//! Git is the integration layer. The bridge never asks a harness what it did —
//! the worktree knows.
//!
//! This module renders `git diff` of a task's worktree against its base branch in
//! two shapes: a cheap **summary** (files changed, +/- lines) for the quiet
//! progress state while building, and the **full patch** for the review gate. It
//! also answers the plan-phase enforcement question — *did anything change
//! outside `.build/`?* — so the UI can flag a planning agent that wrote code.

use std::collections::HashMap;
use std::path::Path;

mod saved;
pub(crate) use saved::diff_between_saved_commits_bounded;

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

/// One changed path in the worktree's delta from base, and what it weighs.
///
/// The counts ride the row because a reader may hold the row long before the
/// hunks behind it: a `git` push and the client's cold pass carry the file
/// list with no patch at all, and the `+`/`−` beside each path — and the
/// totals in the review bar over them — are counted from here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedFile {
    pub path: String,
    pub status: ChangeStatus,
    pub additions: usize,
    pub deletions: usize,
    /// What this file's hunks say, as a key.
    ///
    /// It is taken over the patch text of this file alone, while that text is
    /// being printed, so it costs nothing beyond the walk. A reader holding
    /// the row without the body uses it for both things a content key is for:
    /// deciding whether a body it already has is still this file's, and
    /// deciding whether this file moved since the last time it was reviewed.
    /// Identity, never integrity — the same job `git.status`'s `content_key`
    /// does for an uncommitted file.
    pub content_key: String,
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
    diff_against_base_for(worktree_path, base_branch, DiffPaths::All)
}

/// [`diff_against_base`], narrowed to the paths a reader has open.
pub fn diff_against_base_for(
    worktree_path: &Path,
    base_branch: &str,
    paths: DiffPaths<'_>,
) -> Result<WorktreeDiff, DiffError> {
    let repo = git2::Repository::open(worktree_path)?;
    let base_tree = base_tree(&repo, base_branch)?;
    diff_tree_to_dirty_workdir(&repo, Some(&base_tree), paths)
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
    diff_against_merge_base_for(worktree_path, base_branch, DiffPaths::All)
}

/// [`diff_against_merge_base`], narrowed to the paths a reader has open.
pub fn diff_against_merge_base_for(
    worktree_path: &Path,
    base_branch: &str,
    paths: DiffPaths<'_>,
) -> Result<WorktreeDiff, DiffError> {
    let repo = git2::Repository::open(worktree_path)?;
    let merge_base_tree = merge_base_tree(&repo, base_branch)?;
    diff_tree_to_dirty_workdir(&repo, Some(&merge_base_tree), paths)
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

/// Render an immutable commit-to-commit range. Inputs must be full object ids,
/// not revspecs: callers resolve only persisted stage boundaries through this
/// helper, so later HEAD movement and dirty files cannot alter the result.
pub fn diff_between_commits(
    worktree_path: &Path,
    start_sha: &str,
    completion_sha: &str,
    paths: DiffPaths<'_>,
) -> Result<WorktreeDiff, DiffError> {
    let repo = git2::Repository::open(worktree_path)?;
    let start = repo.find_commit(git2::Oid::from_str(start_sha)?)?;
    let completion = repo.find_commit(git2::Oid::from_str(completion_sha)?)?;
    let start_tree = start.tree()?;
    let completion_tree = completion.tree()?;
    let mut opts = canonical_patch_options();
    paths.narrow(&mut opts);
    let diff =
        repo.diff_tree_to_tree(Some(&start_tree), Some(&completion_tree), Some(&mut opts))?;
    worktree_diff_from_git_diff(&diff)
}

/// Render saved review commits, using an empty tree when no base committed.
/// This opens the object database at `git_dir`, so a removed linked checkout
/// does not change the saved read while its common repository still exists.
pub fn diff_between_saved_commits(
    git_dir: &Path,
    base_sha: Option<&str>,
    head_sha: &str,
    paths: DiffPaths<'_>,
) -> Result<WorktreeDiff, DiffError> {
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
    let diff = repo.diff_tree_to_tree(base_tree.as_ref(), Some(&head_tree), Some(&mut opts))?;
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

/// How much of a changeset a read covers.
///
/// `All` is the whole delta — what a review surface opens with and what a
/// commit acts on. `Only` is the files a reader has on screen: the same
/// changeset, narrowed to the paths asked for, which is how one file's hunks
/// are fetched without the megabytes behind the ones nobody opened.
#[derive(Debug, Clone, Copy)]
pub enum DiffPaths<'a> {
    All,
    Only(&'a [String]),
}

impl DiffPaths<'_> {
    /// Narrow `opts` to these paths. Matching is literal, never glob: the
    /// paths come from a file list this bridge wrote, and a file named with a
    /// `*` in it is the file that name asks for.
    fn narrow(self, opts: &mut git2::DiffOptions) {
        let Self::Only(paths) = self else { return };
        opts.disable_pathspec_match(true);
        for path in paths {
            opts.pathspec(path);
        }
    }
}

/// Pin the patch path vocabulary expected by review clients and hunk parsing.
pub(crate) fn canonical_patch_options() -> git2::DiffOptions {
    let mut opts = git2::DiffOptions::new();
    // Review clients and hunk identity both consume Git's conventional a/b
    // paths. Repository and global diff prefix settings must not change that
    // wire format.
    opts.old_prefix("a/").new_prefix("b/");
    opts
}

/// The options both dirty-workdir paths share. `with_untracked_content` is the
/// one difference: the review surface loads new files so it can print them, the
/// stat surface never does — it counts their lines off disk instead.
///
/// The `a/` and `b/` prefixes are pinned: libgit2 otherwise honours the
/// machine's own `diff.mnemonicPrefix`, and a hunk id hashed over `i/` and
/// `w/` would not be the id every other device computes.
fn dirty_workdir_options(with_untracked_content: bool) -> git2::DiffOptions {
    let mut opts = canonical_patch_options();
    opts.include_untracked(true)
        .recurse_untracked_dirs(true)
        .show_untracked_content(with_untracked_content)
        .old_prefix("a")
        .new_prefix("b")
        .max_size(LARGE_FILE_BYTES as i64);
    opts
}

fn diff_tree_to_dirty_workdir(
    repo: &git2::Repository,
    old_tree: Option<&git2::Tree>,
    paths: DiffPaths<'_>,
) -> Result<WorktreeDiff, DiffError> {
    let mut opts = dirty_workdir_options(true);
    paths.narrow(&mut opts);
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

/// The complete dirty-worktree delta against an exact commit, or the empty
/// tree when `base` is absent. This is used by publication-aware review,
/// whose baseline is resolved from remote-tracking refs rather than a revspec.
pub fn diff_against_commit(
    worktree_path: &Path,
    base: Option<git2::Oid>,
) -> Result<WorktreeDiff, DiffError> {
    diff_against_commit_for(worktree_path, base, DiffPaths::All)
}

/// [`diff_against_commit`], narrowed to the paths a reader has open.
pub fn diff_against_commit_for(
    worktree_path: &Path,
    base: Option<git2::Oid>,
    paths: DiffPaths<'_>,
) -> Result<WorktreeDiff, DiffError> {
    let repo = git2::Repository::open(worktree_path)?;
    let tree = base
        .map(|oid| repo.find_commit(oid).and_then(|commit| commit.tree()))
        .transpose()?;
    diff_tree_to_dirty_workdir(&repo, tree.as_ref(), paths)
}

/// Cheap identity corresponding exactly to [`diff_against_commit`].
pub fn key_against_commit(
    worktree_path: &Path,
    base: Option<git2::Oid>,
) -> Result<String, DiffError> {
    let repo = git2::Repository::open(worktree_path)?;
    let tree = base
        .map(|oid| repo.find_commit(oid).and_then(|commit| commit.tree()))
        .transpose()?;
    dirty_diff_key(&repo, tree.as_ref())
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

    let mut files: Vec<ChangedFile> = diff
        .deltas()
        .map(|delta| ChangedFile {
            path: delta_path(&delta),
            status: map_status(delta.status()),
            additions: 0,
            deletions: 0,
            content_key: String::new(),
        })
        .filter(|file| !is_mcp_config(&file.path))
        .collect();
    let row_of: HashMap<String, usize> = files
        .iter()
        .enumerate()
        .map(|(index, file)| (file.path.clone(), index))
        .collect();
    // One running hash per file, folded over the bytes as they print.
    let mut keys: Vec<u64> = vec![FNV_OFFSET_BASIS; files.len()];

    // Stats are counted while printing (instead of `diff.stats()`) so the
    // excluded MCP config contributes to neither the patch nor the numbers —
    // and the same pass that rolls them up puts each line on its own file's
    // row, so per-file counts cost nothing beyond the walk already being made.
    let mut insertions = 0;
    let mut deletions = 0;
    let mut patch = String::new();
    diff.print(git2::DiffFormat::Patch, |delta, _hunk, line| {
        let path = delta_path(&delta);
        if is_mcp_config(&path) {
            return true;
        }
        let index = row_of.get(&path).copied();
        match (line.origin(), index.map(|index| &mut files[index])) {
            ('+', Some(file)) => {
                insertions += 1;
                file.additions += 1;
            }
            ('-', Some(file)) => {
                deletions += 1;
                file.deletions += 1;
            }
            _ => {}
        }
        let marked = matches!(line.origin(), '+' | '-' | ' ');
        if marked {
            patch.push(line.origin());
        }
        if let Some(index) = index {
            let hash = &mut keys[index];
            if marked {
                *hash = fnv1a64_fold(*hash, &[line.origin() as u8]);
            }
            *hash = fnv1a64_fold(*hash, line.content());
        }
        patch.push_str(&String::from_utf8_lossy(line.content()));
        true
    })?;
    for (file, hash) in files.iter_mut().zip(keys) {
        file.content_key = format!("{hash:016x}");
    }
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

// ---- content hashing -------------------------------------------------------

/// The FNV-1a 64 offset basis and prime.
const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// FNV-1a 64 over the UTF-8 bytes of `text`, as 16 lowercase hex digits.
///
/// Identity, not integrity: cache keys and change tokens, never a security
/// boundary.
pub(crate) fn fnv1a64_hex(text: &str) -> String {
    format!("{:016x}", fnv1a64_fold(FNV_OFFSET_BASIS, text.as_bytes()))
}

/// One more chunk of bytes into a running FNV-1a 64, so a hash can be taken
/// over text that is never assembled into one string.
fn fnv1a64_fold(mut hash: u64, bytes: &[u8]) -> u64 {
    for byte in bytes {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

/// The primary checkout's uncommitted delta: HEAD's tree vs the working
/// directory and index, untracked included — staged + unstaged + new files.
/// This is the "main worktree" review surface; committed work is upstream's
/// business, not a review surface.
pub fn diff_against_head(repo_path: &Path) -> Result<WorktreeDiff, DiffError> {
    diff_against_head_for(repo_path, DiffPaths::All)
}

/// [`diff_against_head`], narrowed to the paths a reader has open.
pub fn diff_against_head_for(
    repo_path: &Path,
    paths: DiffPaths<'_>,
) -> Result<WorktreeDiff, DiffError> {
    let repo = git2::Repository::open(repo_path)?;
    let head_tree = repo.head()?.peel_to_tree()?;
    diff_tree_to_dirty_workdir(&repo, Some(&head_tree), paths)
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
    diff_uncommitted_for(repo_path, DiffPaths::All)
}

/// [`diff_uncommitted`], narrowed to the paths a reader has open.
pub fn diff_uncommitted_for(
    repo_path: &Path,
    paths: DiffPaths<'_>,
) -> Result<WorktreeDiff, DiffError> {
    let repo = git2::Repository::open(repo_path)?;
    let head_tree = head_tree_if_born(&repo)?;
    diff_tree_to_dirty_workdir(&repo, head_tree.as_ref(), paths)
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

    /// The counts a reviewer reads beside each path, on the rows themselves.
    ///
    /// A stack drawn from the rows alone — no hunks fetched yet — has nothing
    /// else to count from: the `+`/`−` pair over each file and the totals in
    /// the review bar are these numbers, and a row that could not say would
    /// paint a file as empty while it loaded.
    #[test]
    fn a_changed_file_carries_its_own_line_counts() {
        let (_dir, repo) = mixed_fixture();
        let diff = diff_uncommitted(&repo).unwrap();
        let file = |path: &str| {
            diff.files()
                .iter()
                .find(|file| file.path == path)
                .unwrap_or_else(|| panic!("{path} is not among {:?}", diff.files()))
        };

        assert_eq!(
            (
                file("tracked-modify.txt").additions,
                file("tracked-modify.txt").deletions
            ),
            (1, 0)
        );
        assert_eq!(
            (
                file("tracked-delete.txt").additions,
                file("tracked-delete.txt").deletions
            ),
            (0, 2)
        );
        assert_eq!(
            (
                file("staged-add.txt").additions,
                file("staged-add.txt").deletions
            ),
            (1, 0)
        );
        // Untracked text prints whole, so its lines are additions like any
        // other; the binary and the oversize file print nothing and count
        // nothing.
        assert_eq!(file("untracked.txt").additions, 3);
        assert_eq!(
            (
                file("untracked.bin").additions,
                file("untracked.bin").deletions
            ),
            (0, 0)
        );

        // The rows ARE the roll-up: a bar that summed them must reach the same
        // numbers the stat carries, or two places on one screen disagree.
        let stat = diff.stat();
        assert_eq!(
            diff.files()
                .iter()
                .map(|file| file.additions)
                .sum::<usize>(),
            stat.insertions,
            "{:?}",
            diff.files()
        );
        assert_eq!(
            diff.files()
                .iter()
                .map(|file| file.deletions)
                .sum::<usize>(),
            stat.deletions,
            "{:?}",
            diff.files()
        );
    }

    /// Each file's key is its own hunks' — so a reader holding the rows
    /// without the bodies can tell which ONE file moved, and a body it
    /// already holds for an untouched file is still that file's body.
    #[test]
    fn a_files_key_moves_only_when_that_file_moves() {
        let (_dir, repo) = mixed_fixture();
        let key_of = |diff: &WorktreeDiff, path: &str| {
            diff.files()
                .iter()
                .find(|file| file.path == path)
                .map(|file| file.content_key.clone())
        };
        let before = diff_uncommitted(&repo).unwrap();

        std::fs::write(repo.join("staged-add.txt"), "staged\nand then some\n").unwrap();
        let after = diff_uncommitted(&repo).unwrap();

        assert_ne!(
            key_of(&before, "staged-add.txt"),
            key_of(&after, "staged-add.txt")
        );
        assert_eq!(
            key_of(&before, "tracked-modify.txt"),
            key_of(&after, "tracked-modify.txt"),
            "a file nobody touched keeps its key"
        );
        // And a narrowed read of one file agrees with the whole changeset's
        // row for it, or a body fetched under that key would never match.
        let asked = ["tracked-modify.txt".to_string()];
        let narrowed = diff_uncommitted_for(&repo, DiffPaths::Only(&asked)).unwrap();
        assert_eq!(
            key_of(&narrowed, "tracked-modify.txt"),
            key_of(&after, "tracked-modify.txt")
        );
    }

    /// The read behind one opened file: the same changeset, narrowed to the
    /// paths asked for and carrying nothing else.
    #[test]
    fn a_diff_narrowed_to_paths_carries_those_files_alone() {
        let (_dir, repo) = mixed_fixture();
        let asked = ["tracked-modify.txt".to_string()];

        let narrowed = diff_uncommitted_for(&repo, DiffPaths::Only(&asked)).unwrap();

        assert_eq!(
            narrowed
                .files()
                .iter()
                .map(|file| file.path.as_str())
                .collect::<Vec<_>>(),
            vec!["tracked-modify.txt"]
        );
        assert!(
            narrowed.patch().contains("tracked-modify.txt"),
            "{}",
            narrowed.patch()
        );
        assert!(
            !narrowed.patch().contains("staged-add.txt"),
            "{}",
            narrowed.patch()
        );
        // The narrowed stat is the narrowed files', so a caller can show what
        // it asked for without pretending to know the whole changeset.
        assert_eq!(narrowed.stat().files_changed, 1, "{:?}", narrowed.stat());
        assert_eq!(narrowed.stat().insertions, 1, "{:?}", narrowed.stat());
    }

    /// A path with no change in this changeset answers nothing rather than an
    /// error: the file list a client reads from can lag the tree by a push.
    #[test]
    fn a_narrowed_diff_of_an_unchanged_path_is_empty() {
        let (_dir, repo) = mixed_fixture();
        let asked = ["nothing-touched-this.txt".to_string()];

        let narrowed = diff_uncommitted_for(&repo, DiffPaths::Only(&asked)).unwrap();

        assert!(narrowed.files().is_empty(), "{:?}", narrowed.files());
        assert_eq!(narrowed.patch(), "");
    }

    /// A pathspec is a path, never a glob: a file literally named with a `*`
    /// is the file that name asks for, and a `*` matches nothing else.
    #[test]
    fn a_narrowed_path_is_matched_literally() {
        let (_dir, repo) = mixed_fixture();
        let asked = ["*.txt".to_string()];

        let narrowed = diff_uncommitted_for(&repo, DiffPaths::Only(&asked)).unwrap();

        assert!(narrowed.files().is_empty(), "{:?}", narrowed.files());
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
    fn review_patches_keep_canonical_prefixes_under_local_diff_config() {
        let mut patches_by_config = Vec::new();
        for (key, value) in [("diff.mnemonicPrefix", "true"), ("diff.noprefix", "true")] {
            let (_dir, repo) = init_repo();
            git_in(&repo, &["config", "--local", key, value]);
            let start = run_git(&repo, &["rev-parse", "HEAD"])
                .unwrap()
                .trim()
                .to_string();
            std::fs::write(repo.join("README.md"), "# project\nline\nreviewed\n").unwrap();

            let aggregate = diff_uncommitted(&repo).unwrap();
            assert!(
                aggregate
                    .patch()
                    .contains("diff --git a/README.md b/README.md"),
                "{key} changed the aggregate patch:\n{}",
                aggregate.patch()
            );
            let per_file = patch_for_paths(&repo, &["README.md".to_string()]).unwrap();
            assert_eq!(per_file.len(), 1);
            assert!(
                per_file[0]
                    .patch
                    .contains("diff --git a/README.md b/README.md"),
                "{key} changed the per-file patch:\n{}",
                per_file[0].patch
            );
            assert_eq!(per_file[0].patch, aggregate.patch());
            patches_by_config.push(aggregate.patch().to_string());

            git_in(&repo, &["add", "README.md"]);
            git_in(&repo, &["commit", "-m", "reviewed"]);
            let completion = run_git(&repo, &["rev-parse", "HEAD"])
                .unwrap()
                .trim()
                .to_string();
            let history = diff_between_commits(&repo, &start, &completion, DiffPaths::All).unwrap();
            assert!(
                history
                    .patch()
                    .contains("diff --git a/README.md b/README.md"),
                "{key} changed the history patch:\n{}",
                history.patch()
            );
        }

        assert_eq!(patches_by_config[0], patches_by_config[1]);
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
        let head = || {
            run_git(&repo, &["rev-parse", "HEAD"])
                .unwrap()
                .trim()
                .to_string()
        };
        let start = head();
        std::fs::write(repo.join("stage-one.txt"), "one\n").unwrap();
        git_in(&repo, &["add", "."]);
        git_in(&repo, &["commit", "-m", "stage one"]);
        let completion = head();

        let expected = diff_between_commits(&repo, &start, &completion, DiffPaths::All).unwrap();
        std::fs::write(repo.join("stage-two.txt"), "two\n").unwrap();
        git_in(&repo, &["add", "."]);
        git_in(&repo, &["commit", "-m", "stage two"]);
        std::fs::write(repo.join("dirty.txt"), "dirty\n").unwrap();
        let after = diff_between_commits(&repo, &start, &completion, DiffPaths::All).unwrap();

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
}

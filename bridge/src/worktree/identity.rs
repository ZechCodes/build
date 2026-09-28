use crate::isolation::{local_branch_ref, WorktreeError};
use std::path::{Path, PathBuf};
pub const BRANCH_PREFIX: &str = "build";

/// The ref a slug becomes in Build's namespace. The one place the formula is
/// written: what a caller reserves a branch under has to be the ref
/// [`WorktreeManager::create`] then cuts, and two spellings of one rule drift
/// apart silently.
pub fn branch_name_for(slug: &str) -> String {
    format!("{BRANCH_PREFIX}/{slug}")
}

/// The branch `path` has checked out, read from the checkout itself. `None`
/// when the path is not a repository or HEAD is detached.
pub fn checked_out_branch(path: &Path) -> Option<String> {
    let repo = git2::Repository::open(path).ok()?;
    let head = repo.head().ok()?;
    if !head.is_branch() {
        return None;
    }
    head.shorthand().map(str::to_string)
}

/// Derive a filesystem- and branch-safe slug from a free-text goal.
///
/// Lowercases, collapses any run of non-alphanumerics to a single hyphen, trims
/// hyphens, truncates, and falls back to `task` if nothing survives.
pub fn slugify(goal: &str) -> String {
    let mut slug = String::new();
    let mut prev_hyphen = false;
    for ch in goal.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            prev_hyphen = false;
        } else if !prev_hyphen && !slug.is_empty() {
            slug.push('-');
            prev_hyphen = true;
        }
    }
    let slug = slug.trim_end_matches('-');
    let slug: String = slug.chars().take(50).collect();
    let slug = slug.trim_end_matches('-').to_string();
    if slug.is_empty() {
        "task".to_string()
    } else {
        slug
    }
}

/// Whether git would hold a branch under this name.
///
/// Git's own ref grammar, plus the two narrowings `git check-ref-format
/// --branch` makes that `is_valid_name` alone does not: a leading `-` would be
/// read as a flag wherever a name reaches an argv slot, and `HEAD` names the
/// pointer rather than a branch. Everything past this guard is a spelling git
/// could hold a branch under — which is the precondition
/// [`crate::gitgui::branch_origin`] needs. It says nothing about whether the
/// branch exists.
pub fn is_ref_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && name != "HEAD"
        && git2::Reference::is_valid_name(&local_branch_ref(name))
}

/// Whether a caller-supplied branch name can be cut exactly as it was given.
///
/// A dispatch's `branch` is either a name or a description of one, and the two
/// are told apart here: git's own rules for a ref, narrowed to segments of
/// letters, digits, `.`, `_` and `-`. That narrowing is what makes the name safe
/// to fold into a directory as well as a ref — and it puts every sentence
/// ("Add CSV export, please") on the slugify path, where it belongs.
pub fn is_usable_branch_name(name: &str) -> bool {
    if name.is_empty() || name.len() > 200 {
        return false;
    }
    let segments: Vec<&str> = name.split('/').collect();
    let segment_is_usable = |segment: &&str| {
        !segment.is_empty()
            && !segment.starts_with('.')
            && !segment.starts_with('-')
            && !segment.ends_with(".lock")
            && segment
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    };
    segments.iter().all(segment_is_usable) && is_ref_name(name)
}

/// A git timestamp (seconds since the epoch) as RFC 3339 UTC — the one
/// timestamp format every surface of the bridge speaks.
pub fn rfc3339_from_unix(seconds: i64) -> Option<String> {
    time::OffsetDateTime::from_unix_timestamp(seconds)
        .ok()?
        .format(&time::format_description::well_known::Rfc3339)
        .ok()
}

const CHECKOUT_ID_PREFIX: &str = "wt-";
const CHECKOUT_ID_DIGITS: usize = 12;

/// The canonical form of a checkout root — the spelling every id, registry key
/// and cache entry is minted from. Falls back to the path as given when the
/// directory cannot answer (it is gone, or it does not exist yet), so a
/// vanished checkout and one still to be cut both key consistently.
pub fn canonical_root(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// The canonical spelling a path WILL have once it exists: the deepest ancestor
/// that does exist, canonicalized, with the missing segments joined back on.
///
/// A checkout's id is minted from its canonical path, so the row that stands
/// for one before `git worktree add` has run has to carry the id the finished
/// checkout will — and on macOS the directory a checkout is about to be made in
/// has two literal spellings.
pub fn canonical_planned_path(path: &Path) -> PathBuf {
    let mut missing: Vec<&std::ffi::OsStr> = Vec::new();
    let mut ancestor = path;
    loop {
        if let Ok(canonical) = std::fs::canonicalize(ancestor) {
            return missing
                .iter()
                .rev()
                .fold(canonical, |resolved, segment| resolved.join(segment));
        }
        match (ancestor.parent(), ancestor.file_name()) {
            (Some(parent), Some(name)) => {
                missing.push(name);
                ancestor = parent;
            }
            _ => return path.to_path_buf(),
        }
    }
}

/// The stable external-worktree id for a canonical absolute path.
pub fn external_worktree_id(path: &Path) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(path.display().to_string().as_bytes());
    let digest = hasher.finalize();
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("{CHECKOUT_ID_PREFIX}{}", &hex[..CHECKOUT_ID_DIGITS])
}

/// Whether `id` was minted by [`external_worktree_id`]. The one id shape whose
/// only liveness test is the scan, so the one a caller has to be able to tell
/// apart from a run, a plan or a row.
pub fn is_checkout_id(id: &str) -> bool {
    id.strip_prefix(CHECKOUT_ID_PREFIX).is_some_and(|hex| {
        hex.len() == CHECKOUT_ID_DIGITS && hex.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}

/// Branch stems that carry no meaningful goal on their own — adoption falls
/// back to the HEAD commit subject for these.
const GENERIC_BRANCH_STEMS: &[&str] = &[
    "main", "master", "dev", "develop", "wip", "tmp", "temp", "test", "testing", "scratch",
    "patch", "fix", "feature", "new", "branch",
];

/// The silently derived goal for an adopted worktree: the branch name verbatim,
/// unless the branch is generic — then the HEAD commit subject.
pub fn derive_adoption_goal(branch: &str, head_subject: &str) -> String {
    let segment = branch.rsplit('/').next().unwrap_or(branch).to_lowercase();
    let stem = strip_trailing_digit_run(&segment);
    let is_generic = stem.is_empty() || GENERIC_BRANCH_STEMS.contains(&stem.as_str());

    if !is_generic {
        return branch.to_string();
    }
    let subject = head_subject.trim();
    if !subject.is_empty() {
        subject.to_string()
    } else if !branch.is_empty() {
        branch.to_string()
    } else {
        "Adopted worktree".to_string()
    }
}

/// Strip one trailing run of ASCII digits, and the single `-`/`_` immediately
/// before that run, from a branch segment (`wip-2` -> `wip`, `test_3` -> `test`).
fn strip_trailing_digit_run(segment: &str) -> String {
    let chars: Vec<char> = segment.chars().collect();
    let mut end = chars.len();
    while end > 0 && chars[end - 1].is_ascii_digit() {
        end -= 1;
    }
    if end == chars.len() {
        return segment.to_string();
    }
    if end > 0 && (chars[end - 1] == '-' || chars[end - 1] == '_') {
        end -= 1;
    }
    chars[..end].iter().collect()
}

use super::WorktreeManager;

impl WorktreeManager {
    /// The directory a named branch lands in: its segments joined by hyphens,
    /// minus Build's own namespace, which every directory here is already
    /// inside. `build/csv-export` → `csv-export`, `feature/csv-export` →
    /// `feature-csv-export`, so two namespaces never claim one directory.
    pub(super) fn directory_name_for(&self, branch: &str) -> String {
        let mut segments: Vec<&str> = branch.split('/').collect();
        if segments.len() > 1 && segments[0] == BRANCH_PREFIX {
            segments.remove(0);
        }
        segments.join("-")
    }
    /// Build the branch name for a slug in Build's namespace.
    pub(super) fn branch_name(&self, slug: &str) -> String {
        branch_name_for(slug)
    }
    /// The checkout's canonical path, refused unless it resolves to somewhere
    /// under the worktrees root — a symlink or a bind mount pointing out of it
    /// is not the checkout that was recorded, whatever the recorded path spells.
    pub(super) fn canonical_managed_path(&self, path: &Path) -> Result<PathBuf, WorktreeError> {
        let checkout = std::fs::canonicalize(path)?;
        if !checkout.starts_with(std::fs::canonicalize(&self.worktrees_root)?) {
            return Err(WorktreeError::Refused(
                "refusing to trust a worktree outside its canonical managed path".to_string(),
            ));
        }
        Ok(checkout)
    }
}

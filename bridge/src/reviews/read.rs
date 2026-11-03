use super::model::ReviewDirectory;
use crate::api::v1::git::{DiffFileRow, DiffStat, FsFileResult, FsTreeResult};
use crate::body_page::{BodySpan, FileRange};
use serde::{Deserialize, Serialize};
use std::path::{Component, Path};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewReadMode {
    #[default]
    Changes,
    Commits,
    Commit,
    Since,
    Tree,
    Blob,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ReviewReadRequest {
    #[serde(default)]
    pub mode: ReviewReadMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<FileRange>,
    #[serde(default = "default_patch")]
    pub patch: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_snapshot_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

fn default_patch() -> bool {
    true
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum ReviewReadResult {
    Changes(ReviewChanges),
    Commits(ReviewCommits),
    Tree(FsTreeResult),
    Blob(FsFileResult),
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ReviewCommits {
    pub commits: Vec<ReviewCommit>,
    pub commits_truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ReviewCommit {
    pub oid: String,
    pub parents: Vec<String>,
    pub subject: String,
    pub author_name: String,
    /// Author timestamp in Unix seconds.
    pub time: i64,
    pub insertions: u64,
    pub deletions: u64,
    pub files_changed: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SinceBaseline {
    PreviousHead,
    Base,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ReviewChanges {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since_baseline: Option<SinceBaseline>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub rewritten: bool,
    pub stat: DiffStat,
    pub files: Vec<DiffFileRow>,
    pub files_truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch: Option<String>,
    pub truncated: bool,
    pub diff_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<BodySpan>,
}

pub fn read(
    directory: &ReviewDirectory,
    request: &ReviewReadRequest,
) -> Result<ReviewReadResult, String> {
    validate_request(request)?;
    if directory.status == super::model::ReviewDirectoryStatus::Unavailable || !directory.is_git {
        return Err("Source unavailable".into());
    }
    let git_dir = directory
        .common_git_dir
        .as_deref()
        .ok_or("Source unavailable")?;
    let repo =
        git2::Repository::open_bare(git_dir).map_err(|_| "Source unavailable".to_string())?;
    match request.mode {
        ReviewReadMode::Changes => read_changes(directory, request, git_dir),
        ReviewReadMode::Commits => commits::read_commits(directory, request, &repo),
        ReviewReadMode::Commit => commits::read_commit(directory, request, &repo, git_dir),
        ReviewReadMode::Since => {
            Err("invalid review read: since requires an earlier revision".into())
        }
        ReviewReadMode::Tree => read_tree(directory, request, &repo),
        ReviewReadMode::Blob => read_blob(directory, request, &repo),
    }
}

pub(crate) fn validate_request(request: &ReviewReadRequest) -> Result<(), String> {
    if (request.commit.is_some() != (request.mode == ReviewReadMode::Commit))
        || (request.from_snapshot_id.is_some() != (request.mode == ReviewReadMode::Since))
        || (request.mode != ReviewReadMode::Commits
            && (request.limit.is_some() || request.cursor.is_some()))
    {
        return Err("invalid review read: options do not match mode".into());
    }
    match request.mode {
        ReviewReadMode::Commits => validate_commit_list_options(request),
        ReviewReadMode::Changes | ReviewReadMode::Commit | ReviewReadMode::Since => {
            validate_change_options(request)
        }
        ReviewReadMode::Tree => validate_tree_options(request),
        ReviewReadMode::Blob => validate_blob_options(request),
    }
}

fn validate_commit_list_options(request: &ReviewReadRequest) -> Result<(), String> {
    if request.path.is_some()
        || !request.paths.is_empty()
        || request.range.is_some()
        || request.limit.is_some_and(|limit| limit == 0 || limit > 200)
    {
        return Err("invalid review read: invalid commits options".into());
    }
    Ok(())
}

fn validate_change_options(request: &ReviewReadRequest) -> Result<(), String> {
    if request.path.is_some()
        || (request.range.is_some() && !request.patch)
        || request.range.is_some_and(|range| range.raw == Some(true))
        || request.paths.len() > 100
    {
        return Err("invalid review read: invalid changes options".into());
    }
    for path in &request.paths {
        safe_path(path)?;
    }
    Ok(())
}

fn validate_tree_options(request: &ReviewReadRequest) -> Result<(), String> {
    if request.range.is_some() || !request.paths.is_empty() {
        return Err("invalid review read: invalid tree options".into());
    }
    if let Some(path) = request.path.as_deref().filter(|path| !path.is_empty()) {
        safe_path(path)?;
    }
    Ok(())
}

fn validate_blob_options(request: &ReviewReadRequest) -> Result<(), String> {
    if !request.paths.is_empty() {
        return Err("invalid review read: invalid blob options".into());
    }
    let path = request
        .path
        .as_deref()
        .ok_or("invalid review read: missing path")?;
    safe_path(path)?;
    Ok(())
}

fn read_changes(
    directory: &ReviewDirectory,
    request: &ReviewReadRequest,
    git_dir: &Path,
) -> Result<ReviewReadResult, String> {
    if request.path.is_some() {
        return Err("changes mode does not take path".into());
    }
    if request.range.is_some() && !request.patch {
        return Err("range requires patch".into());
    }
    let base = directory.base.as_ref().ok_or("Source unavailable")?;
    let Some(head) = directory.head.as_deref() else {
        return Ok(ReviewReadResult::Changes(ReviewChanges {
            rewritten: false,
            since_baseline: None,
            stat: DiffStat {
                files_changed: 0,
                insertions: 0,
                deletions: 0,
            },
            files: Vec::new(),
            files_truncated: false,
            patch: request.patch.then(String::new),
            truncated: false,
            diff_key: format!("{}:unborn", base.oid),
            range: None,
        }));
    };
    let paths = if request.paths.is_empty() {
        crate::diff::DiffPaths::All
    } else {
        crate::diff::DiffPaths::Only(&request.paths)
    };
    let base_oid =
        (base.kind != super::model::ReviewBaseKind::EmptyTree).then_some(base.oid.as_str());
    let diff = crate::diff::diff_between_saved_commits_bounded(
        git_dir,
        base_oid,
        head,
        paths,
        request.patch,
        request.range.map(FileRange::body),
    )
    .map_err(|_| "Source unavailable".to_string())?;
    let stat = diff.stat;
    let files = diff
        .files
        .iter()
        .map(|file| DiffFileRow {
            path: file.path.clone(),
            status: format!("{:?}", file.status),
            additions: file.additions as u64,
            deletions: file.deletions as u64,
            content_key: file.content_key.clone(),
        })
        .collect();
    Ok(ReviewReadResult::Changes(ReviewChanges {
        rewritten: false,
        since_baseline: None,
        stat: DiffStat {
            files_changed: stat.files_changed as u64,
            insertions: stat.insertions as u64,
            deletions: stat.deletions as u64,
        },
        files,
        files_truncated: diff.files_truncated,
        patch: diff.patch,
        truncated: diff.truncated,
        diff_key: format!("{}:{head}", base.oid),
        range: diff.range,
    }))
}

fn saved_tree<'repo>(
    directory: &ReviewDirectory,
    repo: &'repo git2::Repository,
) -> Result<Option<git2::Tree<'repo>>, String> {
    let Some(head) = directory.head.as_deref() else {
        return Ok(None);
    };
    let oid = git2::Oid::from_str(head).map_err(|_| "Source unavailable".to_string())?;
    repo.find_commit(oid)
        .and_then(|commit| commit.tree())
        .map(Some)
        .map_err(|_| "Source unavailable".into())
}

fn safe_path(path: &str) -> Result<&Path, String> {
    let path = Path::new(path);
    if path
        .components()
        .all(|part| matches!(part, Component::Normal(_)))
        && !path.as_os_str().is_empty()
    {
        Ok(path)
    } else {
        Err("invalid review read: invalid path".into())
    }
}

fn read_tree(
    directory: &ReviewDirectory,
    request: &ReviewReadRequest,
    repo: &git2::Repository,
) -> Result<ReviewReadResult, String> {
    if request.range.is_some() || !request.paths.is_empty() {
        return Err("tree mode takes only path".into());
    }
    let path = request.path.as_deref().unwrap_or("");
    let root = saved_tree(directory, repo)?;
    let tree = match (root, path.is_empty()) {
        (None, true) => {
            return Ok(ReviewReadResult::Tree(FsTreeResult {
                path: path.into(),
                entries: Vec::new(),
            }))
        }
        (None, false) => return Err("not a directory".into()),
        (Some(root), true) => root,
        (Some(root), false) => {
            let entry = root
                .get_path(safe_path(path)?)
                .map_err(|_| "not a directory".to_string())?;
            if entry.kind() != Some(git2::ObjectType::Tree) {
                return Err("not a directory".into());
            }
            repo.find_tree(entry.id())
                .map_err(|_| "Source unavailable".to_string())?
        }
    };
    let mut dirs = Vec::new();
    let mut rest = Vec::new();
    let odb = repo.odb().map_err(|_| "Source unavailable".to_string())?;
    for entry in tree.iter() {
        let name = entry.name().ok_or("invalid Git tree name")?.to_owned();
        let kind = if entry.kind() == Some(git2::ObjectType::Tree) {
            "dir"
        } else if entry.filemode() == 0o120000 {
            "symlink"
        } else if entry.kind() == Some(git2::ObjectType::Commit) {
            "submodule"
        } else {
            "file"
        };
        let size = if kind == "file" {
            Some(
                odb.read_header(entry.id())
                    .map_err(|_| "Source unavailable".to_string())?
                    .0 as u64,
            )
        } else {
            None
        };
        let row = crate::api::v1::git::FsTreeEntry {
            name: name.clone(),
            kind: kind.into(),
            size,
        };
        if kind == "dir" {
            dirs.push(row)
        } else {
            rest.push(row)
        }
    }
    dirs.sort_by_key(|entry| entry.name.to_lowercase());
    rest.sort_by_key(|entry| entry.name.to_lowercase());
    dirs.extend(rest);
    Ok(ReviewReadResult::Tree(FsTreeResult {
        path: path.into(),
        entries: dirs,
    }))
}

fn read_blob(
    directory: &ReviewDirectory,
    request: &ReviewReadRequest,
    repo: &git2::Repository,
) -> Result<ReviewReadResult, String> {
    if !request.paths.is_empty() {
        return Err("blob mode takes only path".into());
    }
    let path = request.path.as_deref().ok_or("missing path")?;
    let safe = safe_path(path)?;
    let tree = saved_tree(directory, repo)?.ok_or("No commits yet")?;
    let entry = tree
        .get_path(safe)
        .map_err(|_| "file not found".to_string())?;
    if entry.filemode() == 0o120000 {
        return Err("symlink content is not opened".into());
    }
    if entry.kind() == Some(git2::ObjectType::Commit) {
        return Err("submodule content is not opened".into());
    }
    if entry.kind() != Some(git2::ObjectType::Blob) {
        return Err("not a file".into());
    }
    let blob = repo
        .find_blob(entry.id())
        .map_err(|_| "Source unavailable".to_string())?;
    let bytes = blob.content();
    let mime = crate::app::mime_hint(safe, &bytes[..bytes.len().min(8192)]);
    let (content, truncated, range) = match request.range {
        Some(range) if range.raw == Some(true) && !is_blob_media(mime) => {
            return Err(
                "invalid review read: raw range must name an image, audio, or video file".into(),
            );
        }
        Some(range) if range.raw == Some(true) && bytes.len() > 64 * 1_048_576 => {
            (Vec::new(), true, None)
        }
        Some(range) => {
            let offset = usize::try_from(range.offset)
                .unwrap_or(usize::MAX)
                .min(bytes.len());
            let end = offset
                .saturating_add(range.body().capacity())
                .min(bytes.len());
            let mut page = bytes[offset..end].to_vec();
            if range.raw != Some(true) {
                page.truncate(crate::body_page::page_len(&page, end == bytes.len()));
            }
            let span = BodySpan {
                offset: offset as u64,
                end: (offset + page.len()) as u64,
                total: bytes.len() as u64,
                version: Some(blob.id().to_string()),
            };
            (page, false, Some(span))
        }
        None => {
            let limit = if mime.starts_with("audio/") || mime.starts_with("video/") {
                32 * 1_048_576
            } else {
                crate::body_page::BODY_PAGE_MAX_BYTES as usize
            };
            (
                bytes[..bytes.len().min(limit)].to_vec(),
                bytes.len() > limit,
                None,
            )
        }
    };
    Ok(ReviewReadResult::Blob(FsFileResult {
        path: path.into(),
        size: bytes.len() as u64,
        truncated,
        mime: mime.into(),
        content_b64: crate::encoding::b64encode(&content),
        editable: false,
        encoding: None,
        revision: None,
        range,
    }))
}

fn is_blob_media(mime: &str) -> bool {
    (mime.starts_with("image/") && mime != "image/svg+xml")
        || mime.starts_with("audio/")
        || mime.starts_with("video/")
}

mod commits;
#[cfg(test)]
pub(crate) use commits::install_commit_validation_gate;
pub use commits::read_since;
pub use commits::validate_commit;

#[cfg(test)]
mod tests;

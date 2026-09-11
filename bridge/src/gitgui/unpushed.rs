use serde_json::{json, Value};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
enum PublishedBase {
    PushTarget {
        oid: Option<git2::Oid>,
        label: String,
    },
    PublishedAncestor(git2::Oid),
    Empty,
}

impl PublishedBase {
    fn oid(&self) -> Option<git2::Oid> {
        match self {
            Self::PushTarget { oid, .. } => *oid,
            Self::PublishedAncestor(oid) => Some(*oid),
            Self::Empty => None,
        }
    }

    fn json(&self) -> Value {
        match self {
            Self::PushTarget { label, .. } => json!({ "kind": "push_target", "label": label }),
            Self::PublishedAncestor(_) => json!({ "kind": "published_ancestor", "label": null }),
            Self::Empty => json!({ "kind": "empty", "label": null }),
        }
    }

    fn push_target_exists(&self) -> bool {
        matches!(self, Self::PushTarget { .. })
    }
}

fn direct_remote_tips(repo: &git2::Repository) -> Result<Vec<git2::Oid>, String> {
    let mut tips = Vec::new();
    let refs = repo
        .references_glob("refs/remotes/*/*")
        .map_err(|e| e.to_string())?;
    for reference in refs {
        let reference = reference.map_err(|e| e.to_string())?;
        if reference.kind() == Some(git2::ReferenceType::Direct) {
            if let Some(oid) = reference.target() {
                tips.push(oid);
            }
        }
    }
    tips.sort_unstable();
    tips.dedup();
    Ok(tips)
}

fn nearest_published_ancestor(
    repo: &git2::Repository,
    head: git2::Oid,
    tips: &[git2::Oid],
) -> Result<Option<git2::Oid>, String> {
    if tips.is_empty() {
        return Ok(None);
    }
    let mut walk = repo.revwalk().map_err(|e| e.to_string())?;
    walk.set_sorting(git2::Sort::TOPOLOGICAL)
        .map_err(|e| e.to_string())?;
    walk.push(head).map_err(|e| e.to_string())?;
    for tip in tips {
        walk.hide(*tip).map_err(|e| e.to_string())?;
    }
    let unpublished = walk
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    if unpublished.is_empty() {
        return Ok(Some(head));
    }
    let unpublished_set = unpublished
        .iter()
        .copied()
        .collect::<std::collections::HashSet<_>>();
    for oid in unpublished {
        let commit = repo.find_commit(oid).map_err(|e| e.to_string())?;
        for parent in commit.parent_ids() {
            if !unpublished_set.contains(&parent) && repo.find_commit(parent).is_ok() {
                return Ok(Some(parent));
            }
        }
    }
    Ok(None)
}

fn published_base(repo: &git2::Repository) -> Result<PublishedBase, String> {
    let head = match repo.head().ok().and_then(|head| head.target()) {
        Some(head) => head,
        None => return Ok(PublishedBase::Empty),
    };
    if let Ok(branch) = repo.head().and_then(|head| {
        if !head.is_branch() {
            return Err(git2::Error::from_str("detached"));
        }
        head.shorthand()
            .map(str::to_string)
            .ok_or_else(|| git2::Error::from_str("unnamed branch"))
    }) {
        if let Some(remote) = push_remote(repo, &branch) {
            let label = format!("{remote}/{branch}");
            if let Ok(reference) = repo.find_reference(&format!("refs/remotes/{label}")) {
                if let Some(target) = reference.target() {
                    let base = repo.merge_base(head, target).ok();
                    return Ok(PublishedBase::PushTarget { oid: base, label });
                }
            }
        }
    }
    let tips = direct_remote_tips(repo)?;
    Ok(match nearest_published_ancestor(repo, head, &tips)? {
        Some(oid) => PublishedBase::PublishedAncestor(oid),
        None => PublishedBase::Empty,
    })
}

/// The same destination precedence used by workspace finish. Reading config
/// and remote refs is local and cannot mutate or contact the repository.
fn push_remote(repo: &git2::Repository, branch: &str) -> Option<String> {
    let config = repo.config().ok()?;
    config
        .get_string(&format!("branch.{branch}.pushRemote"))
        .ok()
        .or_else(|| config.get_string("remote.pushDefault").ok())
        .or_else(|| config.get_string(&format!("branch.{branch}.remote")).ok())
        .filter(|remote| remote != "." && !remote.trim().is_empty())
        .or_else(|| {
            repo.find_remote("origin")
                .ok()
                .map(|_| "origin".to_string())
        })
}

fn file_rows(diff: &crate::diff::WorktreeDiff) -> Vec<Value> {
    diff.files()
        .iter()
        .map(|file| json!({ "path": file.path, "status": format!("{:?}", file.status) }))
        .collect()
}

fn file_edited_at(
    repo_path: &Path,
    diff: &crate::diff::WorktreeDiff,
) -> serde_json::Map<String, Value> {
    diff.files()
        .iter()
        .filter_map(|file| {
            crate::diff::file_edited_at(repo_path, &file.path)
                .map(|at| (file.path.clone(), json!(at)))
        })
        .collect()
}

pub fn unpushed_payload(repo_path: &Path, if_diff_key: Option<&str>) -> Result<Value, String> {
    let repo = git2::Repository::open(repo_path).map_err(|e| e.to_string())?;
    let base = published_base(&repo)?;
    let delta_key =
        crate::diff::key_against_commit(repo_path, base.oid()).map_err(|e| e.to_string())?;
    let key = crate::diff::fnv1a64_hex(&format!("{:?}\0{}", base, delta_key));
    if if_diff_key == Some(&key) {
        return Ok(json!({ "unchanged": true, "diff_key": key }));
    }
    let diff =
        crate::diff::diff_against_commit(repo_path, base.oid()).map_err(|e| e.to_string())?;
    let files = file_rows(&diff);
    Ok(json!({
        "patch": diff.patch(),
        "stat": diff.stat().to_json(),
        "files": files,
        "file_edited_at": file_edited_at(repo_path, &diff),
        "diff_key": key,
        "published": base.push_target_exists(),
        "base": base.json(),
    }))
}

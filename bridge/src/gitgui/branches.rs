use super::history::{head_identity, open_repo, HeadIdentity};
use super::mutations::run_git;
use super::status::repo_state_label;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BranchOrigin {
    Local,
    Remote {
        remote: String,
        tracking_ref: String,
    },
    Absent,
}

/// The reference `name` names, when one is there and is a branch.
///
/// A symbolic reference is a pointer at a branch, not a branch: every clone
/// carries `refs/remotes/origin/HEAD`, and treating it as one would offer a
/// `HEAD` branch no remote can serve. It reads here exactly as a missing ref
/// does, so the listing and [`branch_origin`] never disagree about what a
/// remote branch is.
fn direct_reference<'repo>(
    repo: &'repo git2::Repository,
    name: &str,
) -> Result<Option<git2::Reference<'repo>>, git2::Error> {
    match repo.find_reference(name) {
        Ok(reference) if reference.kind() == Some(git2::ReferenceType::Direct) => {
            Ok(Some(reference))
        }
        Ok(_) => Ok(None),
        Err(error) if error.code() == git2::ErrorCode::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// Where the ref behind `branch` lives — the one answer to "is this branch
/// here, on a remote, or nowhere", which decides whether a checkout needs a
/// fetch first and whether teardown owns the branch afterwards.
///
/// Only a missing ref means [`BranchOrigin::Absent`]. Every other git failure
/// is returned: callers guarantee `branch` is a name git could hold a branch
/// under, so a spec error here is a bug and not an answer.
pub fn branch_origin(repo: &git2::Repository, branch: &str) -> Result<BranchOrigin, git2::Error> {
    if direct_reference(repo, &format!("refs/heads/{branch}"))?.is_some() {
        return Ok(BranchOrigin::Local);
    }
    for remote in remotes_in_fetch_precedence(repo)? {
        let tracking_ref = format!("refs/remotes/{remote}/{branch}");
        if direct_reference(repo, &tracking_ref)?.is_some() {
            return Ok(BranchOrigin::Remote {
                remote,
                tracking_ref,
            });
        }
    }
    Ok(BranchOrigin::Absent)
}

/// A checkoutable reference kind. Remote-tracking refs create a same-name
/// local tracking branch; exact full refs keep every selection unambiguous.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefKind {
    Branch,
    RemoteBranch,
    Tag,
}

impl RefKind {
    fn as_str(self) -> &'static str {
        match self {
            RefKind::Branch | RefKind::RemoteBranch => "branch",
            RefKind::Tag => "tag",
        }
    }
}

/// One exact branch or tag offered by [`ref_list`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefRow {
    pub kind: RefKind,
    pub name: String,
    pub full_ref: String,
    pub current: bool,
    pub remote: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u64,
    pub behind: u64,
}

impl RefRow {
    pub fn into_json(self) -> Value {
        json!({
            "kind": self.kind.as_str(),
            "name": self.name,
            "full_ref": self.full_ref,
            "current": self.current,
            "remote": self.remote,
            "upstream": self.upstream,
            "ahead": self.ahead,
            "behind": self.behind,
        })
    }
}

/// HEAD as represented by the refs picker. A detached checkout identifies its
/// commit rather than guessing which of possibly several equal tags selected
/// it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CurrentRef {
    Branch { name: String, full_ref: String },
    Detached { commit: String },
}

impl CurrentRef {
    pub fn into_json(self) -> Value {
        match self {
            CurrentRef::Branch { name, full_ref } => {
                json!({ "kind": "branch", "name": name, "full_ref": full_ref })
            }
            CurrentRef::Detached { commit } => {
                json!({ "kind": "detached", "commit": commit })
            }
        }
    }
}

/// Every exact local branch, remote-only branch, and tag, plus HEAD's current identity.
pub struct RefListing {
    pub current: CurrentRef,
    pub refs: Vec<RefRow>,
}

impl RefListing {
    pub fn into_json(self) -> Value {
        json!({
            "current": self.current.into_json(),
            "refs": self.refs.into_iter().map(RefRow::into_json).collect::<Vec<_>>(),
        })
    }
}

/// A local branch's position against its own upstream. A branch with no
/// upstream is level with nothing.
#[derive(Default)]
struct BranchSync {
    upstream: Option<String>,
    ahead: u64,
    behind: u64,
}

fn upstream_sync(
    repo: &git2::Repository,
    branch: &git2::Branch,
    head: &git2::Commit,
) -> Result<BranchSync, String> {
    let Ok(upstream) = branch.upstream() else {
        return Ok(BranchSync::default());
    };
    let name = upstream.name().ok().flatten().map(str::to_string);
    let (ahead, behind) = match upstream.get().target() {
        Some(upstream_oid) => repo
            .graph_ahead_behind(head.id(), upstream_oid)
            .map_err(|e| e.to_string())?,
        None => (0, 0),
    };
    Ok(BranchSync {
        upstream: name,
        ahead: ahead as u64,
        behind: behind as u64,
    })
}

/// The order a branch that exists only on remotes is fetched from: `origin`
/// when the repository has one, then the rest in git's own (sorted) order.
fn remotes_in_fetch_precedence(repo: &git2::Repository) -> Result<Vec<String>, git2::Error> {
    let remotes = repo.remotes()?;
    let mut names: Vec<String> = remotes.iter().flatten().map(str::to_string).collect();
    names.sort_by_key(|name| name != "origin");
    Ok(names)
}

fn current_ref(identity: &HeadIdentity) -> CurrentRef {
    match identity {
        HeadIdentity::Branch { name, full_ref } => CurrentRef::Branch {
            name: name.clone(),
            full_ref: full_ref.clone(),
        },
        HeadIdentity::Detached { commit } => CurrentRef::Detached {
            commit: commit.to_string(),
        },
    }
}

/// `git.refs`: exact branches and tags suitable for an unambiguous ref picker.
/// Cached remote-tracking refs are included without fetching; selecting one
/// creates its local tracking branch.
pub fn ref_list(repo_path: &Path) -> Result<RefListing, String> {
    let repo = open_repo(repo_path)?;
    let identity = head_identity(&repo)?;
    let current_full_ref = match &identity {
        HeadIdentity::Branch { full_ref, .. } => Some(full_ref.as_str()),
        HeadIdentity::Detached { .. } => None,
    };
    let mut refs = Vec::new();
    let mut local_names = HashSet::new();
    for item in repo
        .branches(Some(git2::BranchType::Local))
        .map_err(|e| e.to_string())?
    {
        let (branch, _) = item.map_err(|e| e.to_string())?;
        if branch.get().kind() != Some(git2::ReferenceType::Direct) {
            continue;
        }
        let Some(name) = branch.name().map_err(|e| e.to_string())? else {
            continue;
        };
        if !branch_name_is_switchable(name) {
            continue;
        }
        let full_ref = format!("refs/heads/{name}");
        let head = branch.get().peel_to_commit().map_err(|e| e.to_string())?;
        let sync = upstream_sync(&repo, &branch, &head)?;
        local_names.insert(name.to_string());
        refs.push(RefRow {
            kind: RefKind::Branch,
            name: name.to_string(),
            full_ref: full_ref.clone(),
            current: current_full_ref == Some(full_ref.as_str()),
            remote: None,
            upstream: sync.upstream,
            ahead: sync.ahead,
            behind: sync.behind,
        });
    }

    for remote in remotes_in_fetch_precedence(&repo).map_err(|e| e.to_string())? {
        let prefix = format!("refs/remotes/{remote}/");
        for reference in repo
            .references_glob(&format!("{prefix}*"))
            .map_err(|e| e.to_string())?
        {
            let reference = reference.map_err(|e| e.to_string())?;
            if reference.kind() != Some(git2::ReferenceType::Direct) {
                continue;
            }
            let Some(full_ref) = reference.name() else {
                continue;
            };
            let Some(name) = full_ref.strip_prefix(&prefix) else {
                continue;
            };
            if local_names.contains(name) || !branch_name_is_switchable(name) {
                continue;
            }
            local_names.insert(name.to_string());
            refs.push(RefRow {
                kind: RefKind::RemoteBranch,
                name: name.to_string(),
                full_ref: full_ref.to_string(),
                current: false,
                remote: Some(remote.clone()),
                upstream: None,
                ahead: 0,
                behind: 0,
            });
        }
    }

    for reference in repo
        .references_glob("refs/tags/*")
        .map_err(|e| e.to_string())?
    {
        let reference = reference.map_err(|e| e.to_string())?;
        if reference.kind() != Some(git2::ReferenceType::Direct) {
            continue;
        }
        let Some(full_ref) = reference.name() else {
            continue;
        };
        let Some(name) = full_ref.strip_prefix("refs/tags/") else {
            continue;
        };
        refs.push(RefRow {
            kind: RefKind::Tag,
            name: name.to_string(),
            full_ref: full_ref.to_string(),
            current: false,
            remote: None,
            upstream: None,
            ahead: 0,
            behind: 0,
        });
    }

    // An unborn branch has no refs/heads/* entry yet, but it is still the
    // checkout's current ref and must remain visible in the picker.
    if let HeadIdentity::Branch { name, full_ref } = &identity {
        if branch_name_is_switchable(name) && !refs.iter().any(|row| row.full_ref == *full_ref) {
            refs.push(RefRow {
                kind: RefKind::Branch,
                name: name.clone(),
                full_ref: full_ref.clone(),
                current: true,
                remote: None,
                upstream: None,
                ahead: 0,
                behind: 0,
            });
        }
    }

    refs.sort_by(|a, b| {
        b.current
            .cmp(&a.current)
            .then_with(|| (a.kind != RefKind::Branch).cmp(&(b.kind != RefKind::Branch)))
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(RefListing {
        current: current_ref(&identity),
        refs,
    })
}

/// Reject a client-supplied branch name before it reaches an argv slot: an
/// explicit leading-dash guard (so it can never be read as a flag even where
/// git accepts no `--`, e.g. `git switch`) plus `git check-ref-format
/// --branch`, git's own ref-name grammar.
fn branch_name_is_switchable(branch: &str) -> bool {
    !branch.is_empty() && !branch.starts_with('-') && branch != "HEAD"
}

fn validate_branch_name(repo_path: &Path, branch: &str) -> Result<(), String> {
    if !branch_name_is_switchable(branch) {
        return Err(format!("invalid branch name: {branch}"));
    }
    run_git(repo_path, &["check-ref-format", "--branch", branch])
        .map_err(|_| format!("invalid branch name: {branch}"))?;
    Ok(())
}

fn checkout_ref_target(full_ref: &str) -> Result<(RefKind, &str), String> {
    let target = if let Some(name) = full_ref.strip_prefix("refs/heads/") {
        (RefKind::Branch, name)
    } else if let Some(name) = full_ref.strip_prefix("refs/tags/") {
        (RefKind::Tag, name)
    } else if let Some(name) = full_ref.strip_prefix("refs/remotes/") {
        (RefKind::RemoteBranch, name)
    } else {
        return Err("ref must be a full local branch or tag name".to_string());
    };
    if target.1.is_empty() {
        return Err(format!("invalid ref name: {full_ref}"));
    }
    Ok(target)
}

fn validate_checkout_ref(repo_path: &Path, full_ref: &str) -> Result<(RefKind, String), String> {
    let (kind, name) = checkout_ref_target(full_ref)?;
    run_git(repo_path, &["check-ref-format", full_ref])
        .map_err(|_| format!("invalid ref name: {full_ref}"))?;
    if kind == RefKind::Branch {
        // Full ref grammar permits refs/heads/-, but `git switch -- -` means
        // "the previous checkout" rather than the branch literally named -.
        // Branch-mode validation rejects that shorthand and every other name
        // switch cannot safely address.
        validate_branch_name(repo_path, name)
            .map_err(|_| format!("invalid branch ref: {full_ref}"))?;
    }
    let repo = open_repo(repo_path)?;
    let reference = repo
        .find_reference(full_ref)
        .map_err(|error| match error.code() {
            git2::ErrorCode::NotFound => format!("unknown ref: {full_ref}"),
            _ => format!("cannot read ref {full_ref}: {error}"),
        })?;
    if reference.kind() != Some(git2::ReferenceType::Direct) {
        return Err(format!("ref is not directly checkoutable: {full_ref}"));
    }
    reference
        .peel_to_commit()
        .map_err(|_| format!("ref does not point to a commit: {full_ref}"))?;
    if kind == RefKind::RemoteBranch {
        let remote = remotes_in_fetch_precedence(&repo)
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|remote| full_ref.starts_with(&format!("refs/remotes/{remote}/")))
            .ok_or_else(|| format!("unknown remote branch ref: {full_ref}"))?;
        let branch = full_ref
            .strip_prefix(&format!("refs/remotes/{remote}/"))
            .unwrap_or("");
        validate_branch_name(repo_path, branch)
            .map_err(|_| format!("invalid remote branch ref: {full_ref}"))?;
        if repo.find_reference(&format!("refs/heads/{branch}")).is_ok() {
            return Err(format!("local branch already exists: {branch}"));
        }
        return Ok((kind, branch.to_string()));
    }
    Ok((kind, name.to_string()))
}

/// The reason `git.checkout` refuses to switch branches away from a non-clean
/// state, naming the actual in-progress operation so the message matches what
/// the user must finish or abort. A "conflicted" working tree has no operation
/// to abort — the unmerged files themselves must be resolved.
pub(super) fn checkout_refusal_message(state: &str) -> String {
    match state {
        "merging" => "finish or abort the in-progress merge first",
        "rebasing" => "finish or abort the in-progress rebase first",
        "cherry-picking" => "finish or abort the in-progress cherry-pick first",
        "reverting" => "finish or abort the in-progress revert first",
        "bisecting" => "finish or abort the in-progress bisect first",
        "conflicted" => "resolve the conflicted files first",
        _ => "finish or abort the in-progress operation first",
    }
    .to_string()
}

/// `git.checkout_ref`: checkout one exact ref from [`ref_list`]. Local branch
/// refs attach HEAD, remote-only refs create a local tracking branch, and tags detach HEAD.
/// Ordinary `git switch` semantics preserve compatible local edits and refuse
/// when switching would overwrite them. No force, discard, merge, or stash
/// option is used.
pub fn checkout_ref(repo_path: &Path, full_ref: &str) -> Result<(), String> {
    let (kind, name) = validate_checkout_ref(repo_path, full_ref)?;
    let repo = open_repo(repo_path)?;
    let state = repo_state_label(&repo)?;
    drop(repo);
    if state != "clean" {
        return Err(checkout_refusal_message(state));
    }
    match kind {
        RefKind::Branch => run_git(repo_path, &["switch", "--no-overwrite-ignore", "--", &name]),
        RefKind::Tag => run_git(
            repo_path,
            &[
                "switch",
                "--no-overwrite-ignore",
                "--detach",
                "--",
                full_ref,
            ],
        ),
        RefKind::RemoteBranch => run_git(
            repo_path,
            &[
                "switch",
                "--no-overwrite-ignore",
                "--track",
                "-c",
                &name,
                "--",
                full_ref,
            ],
        ),
    }
    .map(|_| ())
}

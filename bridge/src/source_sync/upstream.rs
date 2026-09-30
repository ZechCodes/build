//! What a base branch follows, as git reads it for `git pull` (#268): the
//! remote in `branch.<base>.remote` and the branch in `branch.<base>.merge`,
//! and without them `origin` (or the only remote) and a branch of the base's
//! own name. `remote.pushDefault` says where pushes go and is not read.

/// The remote a base follows and its branch there.
pub(super) struct Upstream {
    pub(super) remote: String,
    pub(super) branch: String,
}

/// What `base` follows, or `None` when it follows nothing on a remote.
pub(super) fn upstream_of(repo: &git2::Repository, base: &str) -> Option<Upstream> {
    let config = repo.config().ok()?;
    let read = |key: &str| {
        config
            .get_string(&format!("branch.{base}.{key}"))
            .ok()
            .filter(|value| !value.trim().is_empty())
    };
    let Some(remote) = read("remote") else {
        return default_remote(repo).map(|remote| Upstream {
            remote,
            branch: base.to_string(),
        });
    };
    if remote == "." {
        return None;
    }
    let branch = read("merge")
        .and_then(|merge| merge.strip_prefix("refs/heads/").map(str::to_string))
        .unwrap_or_else(|| base.to_string());
    Some(Upstream { remote, branch })
}

fn default_remote(repo: &git2::Repository) -> Option<String> {
    if repo.find_remote("origin").is_ok() {
        return Some("origin".to_string());
    }
    let remotes = repo.remotes().ok()?;
    (remotes.len() == 1)
        .then(|| remotes.get(0).map(str::to_string))
        .flatten()
}

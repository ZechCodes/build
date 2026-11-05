//! Keep the device vault outside every tree Build copies, backs up or exports.
use super::{canonicalize_state_path, BridgeConfig};
use std::path::Path;

pub(super) fn validate(config: &BridgeConfig) -> Result<(), String> {
    for (name, path) in [
        ("BRIDGE_TASKS_DIR", &config.tasks_dir),
        ("BRIDGE_PROJECTS_DIR", &config.projects_dir),
        ("BRIDGE_WORKTREES", &config.worktrees),
    ] {
        validate_tree(name, path, &config.state_root)?;
    }
    if let Some(repo) = &config.repo {
        validate_tree("BRIDGE_REPO", Path::new(repo), &config.state_root)?;
    }
    for (path, _) in &config.environment_projects {
        validate_tree("BRIDGE_PROJECTS", path, &config.state_root)?;
    }
    Ok(())
}

/// Also used when loading an older persisted update job. No directory is made
/// and no key/value file is opened while resolving these paths.
pub(crate) fn validate_tree(name: &str, path: &Path, state_root: &Path) -> Result<(), String> {
    let vault = canonicalize_state_path(&state_root.join("secrets"))
        .map_err(|error| format!("cannot resolve device vault path: {error}"))?;
    let tree =
        canonicalize_state_path(path).map_err(|error| format!("cannot resolve {name}: {error}"))?;
    if tree.starts_with(&vault) || vault.starts_with(&tree) {
        return Err(format!("{name} overlaps the device vault"));
    }
    Ok(())
}

/// Persisted update jobs predate config validation and carry no channel. Refuse
/// overlap with either supported device vault before moving any task tree.
pub(crate) fn validate_update_tasks(home: &Path, tasks: &Path) -> Result<(), String> {
    for root in [home.join(".build"), home.join(".local/state/build-dev")] {
        validate_tree("update task store", tasks, &root)?;
    }
    Ok(())
}

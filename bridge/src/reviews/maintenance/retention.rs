use super::*;
use crate::workspace::{Workspace, WorkspaceStatus, MANIFEST_FILE};

pub(super) fn retained(review: &Review) -> Result<bool, String> {
    let Some(metadata) = &review.pull_request else {
        // Legacy snapshot history can also refer to an owned receiver.
        return Ok(true);
    };
    match metadata.status {
        PullRequestStatus::Merged => Ok(false),
        PullRequestStatus::Closed => retained_workspace(review),
        _ => Ok(true),
    }
}

fn retained_workspace(review: &Review) -> Result<bool, String> {
    let Some(binding) = review.bindings.first() else {
        return Ok(false);
    };
    for root in binding.working_repository.ancestors() {
        let bytes = match std::fs::read(root.join(MANIFEST_FILE)) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.to_string()),
        };
        let workspace: Workspace =
            serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
        if workspace.id != review.workspace_id || workspace.root != root {
            continue;
        }
        return Ok(workspace.managed
            && workspace.status == WorkspaceStatus::Ready
            && review.bindings.iter().all(|binding| {
                workspace.directories.iter().any(|directory| {
                    directory.id == binding.directory_id
                        && directory.source_id == binding.source_id
                        && directory.path == binding.working_repository
                        && directory.source_path == binding.source_repository
                        && directory.is_git
                        && directory.path.is_dir()
                })
            }));
    }
    Ok(false)
}

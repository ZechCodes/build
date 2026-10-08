//! Read only the registered publication ref and selected source target.
use super::{comparison_base, import_commit, received_head, source_base};
use crate::reviews::model::ReviewBranchBinding;
use crate::reviews::receivers::{validate_binding_receiver, validate_registered_receiver};

#[derive(Debug, Clone)]
pub struct ReceivedCommit {
    pub head: String,
    pub target_head: String,
    pub comparison_base: String,
}

pub fn observe_received(binding: &ReviewBranchBinding) -> Result<ReceivedCommit, String> {
    validate_registered_receiver(binding)?;
    let head = received_head(binding)?.ok_or("review receiving branch is missing")?;
    let repository = git2::Repository::open_bare(&binding.receiving_repository)
        .map_err(|error| error.to_string())?;
    repository
        .find_commit(git2::Oid::from_str(&head).map_err(|error| error.to_string())?)
        .map_err(|error| format!("review receiving ref is not a commit: {error}"))?;
    // Source identity/base failures retain the previous snapshot. Historical
    // reads need only the receiver, but new comparisons require this target.
    validate_binding_receiver(binding)?;
    let target_head = source_base(binding)?;
    let target = git2::Oid::from_str(&target_head).map_err(|error| error.to_string())?;
    if repository.find_commit(target).is_err() {
        import_commit(
            &binding.receiving_repository,
            &binding.source_repository,
            &target_head,
        )?;
    }
    let comparison_base = comparison_base(binding, &head, &target_head)?;
    Ok(ReceivedCommit {
        head,
        target_head,
        comparison_base,
    })
}

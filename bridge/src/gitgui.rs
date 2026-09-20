//! Stable facade for browser git-GUI helpers.

mod branches;
mod history;
mod mutations;
mod network;
mod patches;
mod status;
pub(crate) use status::counted_status_shape;
#[cfg(test)]
mod tests;
mod unpushed;

pub use branches::{
    branch_delete, branch_list, branch_origin, checkout, checkout_ref, ref_list, BranchListing,
    BranchOrigin, BranchRow, CurrentRef, RefKind, RefListing, RefRow,
};
pub use history::{
    log_page, show_commit, truncate_at_utf8_boundary, LogHighlight, GIT_BODY_MAX_BYTES,
    GIT_SHOW_MAX_PATCH_BYTES, GIT_STATUS_MAX_FILES, GIT_SUBJECT_MAX_BYTES,
};
pub use mutations::{
    commit_staged, discard_paths, merge_abort, stage_paths, stash_pop, stash_push, unstage_paths,
};
pub use network::{fetch, pull, push, GIT_NETWORK_TIMEOUT_SECS};
pub use patches::{file_patches, GIT_DIFF_MAX_ANSWER_BYTES, GIT_DIFF_MAX_PATHS};
pub use status::{status_payload, status_payload_unless};
pub use unpushed::{
    aggregate_work_summary, unpushed_file_diff, unpushed_key, unpushed_payload, unpushed_summary,
    work_summary, WorkSummary,
};

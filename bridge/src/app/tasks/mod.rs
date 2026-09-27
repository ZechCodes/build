pub(in crate::app) mod documents;
pub(in crate::app) mod scheduler;
pub(in crate::app) mod sessions;
pub(in crate::app) mod views;

/// Stable wire error for callers that still know the retired Task/Plan API.
/// Stored records remain readable, but they can no longer start background
/// planning or implementation work.
pub(in crate::app) const TASKS_RETIRED_ERROR: &str =
    "Tasks and planning have been retired; use a workspace directly";

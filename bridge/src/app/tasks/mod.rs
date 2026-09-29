pub(in crate::app) mod activity;
pub(in crate::app) mod documents;
pub(in crate::app) mod sessions;
pub(in crate::app) mod views;

/// The refusal a stored Task's agents and conversation answer with. Stored
/// records remain readable, but nothing can start planning or
/// implementation work on them again.
pub(in crate::app) const TASKS_RETIRED_ERROR: &str =
    "Tasks and planning have been retired; use a workspace directly";

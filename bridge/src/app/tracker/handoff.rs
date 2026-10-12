//! What travels with an assignment besides the task, and the retry that
//! must not deliver it twice.
//!
//! A client that loses the answer to `tasks.assign` asks again. With the
//! same `operation_id` the second request is the first one: the task already
//! records a `dispatched` event carrying that id, and that dispatch is the
//! answer. Nothing is delivered and no agent is made a second time.

/// The note said under a hand-off, and the operation id its delivery is
/// queued under — the client's, when it named one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(in crate::app) struct Handoff {
    pub(in crate::app) note: Option<String>,
    pub(in crate::app) operation_id: Option<String>,
}

impl Handoff {
    /// A hand-off with only a note, which is every caller that predates
    /// operation ids.
    pub(in crate::app) fn noted(note: Option<String>) -> Self {
        Self {
            note,
            operation_id: None,
        }
    }
}

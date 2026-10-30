//! Shared application execution. Both transport adapters use these same read
//! executors, bounded queues, ordered terminal lanes and queued-read folding.
//! The legacy carrier exports the handler as a compatibility seam while its
//! session and output boundary is migrated separately.

pub(crate) mod dispatch;
mod io;
pub(crate) mod sql;
#[cfg(test)]
pub(crate) mod sql_measurements;

mod context;
mod invocation;
pub mod session;
pub use crate::timing::Priority as RequestPriority;
pub use context::CallContext;
use context::CallContext as SessionSender;
pub(crate) use context::PreparedOutput;
pub use dispatch::FrameHandler;
pub(crate) use dispatch::PreparedObservation;
pub use invocation::{Invocation, RequestMetadata};

mod service;
pub use service::{Call, CallError, ExecutionService};

/// Read interest can stop reads and controls without cancelling admitted writes.
pub(crate) fn requires_read_interest(method: &str) -> bool {
    use crate::api::effects::{effect, Effect};
    match effect(method) {
        Some(
            Effect::SqlRead
            | Effect::Control
            | Effect::SnapshotRead
            | Effect::ExternalRead
            | Effect::LegacyRead,
        ) => true,
        Some(Effect::Write) | None => false,
    }
}

#[cfg(any(test, feature = "testing"))]
pub mod testing;

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

#[cfg(any(test, feature = "testing"))]
pub mod testing;

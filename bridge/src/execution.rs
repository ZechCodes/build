//! Shared application execution. Both transport adapters use these same read
//! executors, bounded queues, ordered terminal lanes and queued-read folding.
//! The legacy carrier exports the handler as a compatibility seam while its
//! session and output boundary is migrated separately.

pub(crate) mod dispatch;
mod io;
pub(crate) mod sql;
#[cfg(test)]
pub(crate) mod sql_measurements;

use crate::carrier::SessionSender;
pub use dispatch::FrameHandler;
pub(crate) use dispatch::PreparedObservation;

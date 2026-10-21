//! Durable bridge update state and the small coordinator around an updater helper.

mod admission;
pub mod installer;
pub mod provenance;
pub mod release;
mod service;
mod status;

#[cfg(test)]
pub(crate) use admission::test_support as admission_test_support;
pub use admission::{AdmissionGate, AdmissionLease};
pub use service::{BusyProbe, UpdateBackend, UpdateConfig, UpdateError, UpdateService};
pub use status::{HelperResult, InstallWhen, Release, UpdateState, UpdateStatus};

#[cfg(test)]
mod tests;

//! Durable bridge update state and the small coordinator around an updater helper.

pub mod installer;
pub mod provenance;
pub mod release;
mod service;
mod status;

pub use service::{BusyProbe, UpdateBackend, UpdateConfig, UpdateError, UpdateService};
pub use status::{HelperResult, InstallWhen, Release, UpdateState, UpdateStatus};

#[cfg(test)]
mod tests;

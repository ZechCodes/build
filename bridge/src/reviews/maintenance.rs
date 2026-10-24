//! Resumable receiver reclamation. Only the daemon starts this service; never RPC.

use crate::reviews::model::PullRequestStatus;
use crate::reviews::records::Review;
use crate::store::Store;
use std::path::{Path, PathBuf};

mod deletion;
mod leases;
mod refs;
mod retention;
mod service;
mod size;
pub use service::MaintenanceHandle;
pub(crate) use service::{spawn, Registration};

#[derive(Default)]
struct Report {
    freed_bytes: u64,
    removed: usize,
    collected: usize,
}

mod worker;
use worker::Worker;

#[cfg(test)]
pub(crate) fn turn_for_test(store: Store, root: PathBuf) -> Result<(), String> {
    Worker::new(store, root).turn().map(|_| ())
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod workflow_tests;

//! Exact, serializable release receipts. Planning reads Git; only release mutates it.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct HistoryPins {
    pub refs: Vec<PinRef>,
    pub markers: Vec<PinMarker>,
    /// Exact snapshot namespaces whose ownership metadata was unavailable at
    /// admission. Resolve only these recorded directories when Git returns.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deferred_snapshots: Vec<DeferredSnapshotPins>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PinRef {
    pub repository: PathBuf,
    pub name: String,
    pub expected: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PinMarker {
    pub path: PathBuf,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeferredSnapshotPins {
    pub task_id: String,
    pub snapshot_id: String,
    pub directory: super::model::ReviewDirectory,
}

mod planning;
mod release;
pub(crate) use planning::plan;
pub(crate) use release::{release, release_available};

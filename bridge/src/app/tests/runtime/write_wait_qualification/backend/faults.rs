//! OFFSOURCE physical-clock controls, separate from all qualification attempts.
//! No source materialization/build/runtime evidence accompanies this draft.
use super::super::client_watch::{ClientWatchGuard, TrafficCohort, WatchReport};
use super::super::rpc::{ReplyProof, RpcHarness};
use super::super::trial_complete::{self, ArtifactSink, Slot, TrialName};
use super::*;
use crate::app::AppState;
use crate::timing::write_wait::{IntervalCategory, WaitKind, WriteSnapshot, WriteWaitRegistry};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
mod context;
mod evidence;
mod hold;
mod overload;
mod plan;
mod post_ack;
mod pre_ack;
mod preparation;
mod refusal;
pub(in crate::app::tests::runtime::write_wait_qualification) use refusal::validate_candidate;
mod scripts;
mod validate;
pub(in crate::app::tests::runtime::write_wait_qualification) use evidence::{
    FaultArtifacts, FaultReport, FaultRun, RetainedFault,
};
pub(in crate::app::tests::runtime::write_wait_qualification) use overload::{
    run_overload, OverloadRun,
};
pub(in crate::app::tests::runtime::write_wait_qualification) use plan::overload_plan;
pub(in crate::app::tests::runtime::write_wait_qualification) use post_ack::post_ack_required;
pub(in crate::app::tests::runtime::write_wait_qualification) use pre_ack::{
    pre_ack_app, pre_ack_exclusion,
};
pub(in crate::app::tests::runtime::write_wait_qualification) use scripts::{
    run_physical, PhysicalCase, PhysicalRun,
};
const PHYSICAL_HOLD: Duration = Duration::from_secs(6);
const CLEANUP_BOUND: Duration = Duration::from_secs(20);

use crate::app::tests::runtime::write_wait_qualification::fault_tests::provenance::BoundFault;

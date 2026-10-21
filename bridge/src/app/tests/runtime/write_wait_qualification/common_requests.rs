//! OFF-SOURCE concrete common-write request mapping and native effect checks.
//! Builds requests only; never dispatches/prepares a file/ref per operation.
//! Strong saved evidence must be extracted from the real Store/thread/session
//! log/libgit checkout. An expected value echoed as saved evidence is invalid.
use super::trial_complete::{
    Envelope, OracleAssertion, OracleWitness, Route, Slot, Traffic, TrialName, WitnessScalar,
    WriteClass,
};
use crate::mcp::BridgeAction;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

#[path = "common_requests/model.rs"]
mod model;
pub use model::{
    ActiveTarget, ActorExpectation, ActualRequest, CommonRequest, CommonTargets, ExpectedEffect,
    ProjectTarget, RootGitRefs, SavedEffect,
};
#[path = "common_requests/builder.rs"]
mod builder;
pub use builder::build_common_requests;
#[path = "common_requests/verify.rs"]
mod verify;

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn body(kind: &str, original: &str) -> Result<String, String> {
    let value = format!("Qualification {kind} for {original}: retain this exact durable effect and original identity.");
    if !(64..=256).contains(&value.len()) {
        return Err("common payload must remain64..256 UTF8 bytes".into());
    }
    Ok(value)
}
fn file_scope(target: &ActiveTarget) -> Value {
    json!({"workspace_id":target.workspace,"source_id":target.source,"entity_id":target.owner})
}
fn task_for_owner(targets: &CommonTargets, owner: usize) -> String {
    let target = &targets.active[owner];
    let siblings = targets
        .active
        .iter()
        .enumerate()
        .filter(|(_, candidate)| candidate.project == target.project)
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    let position = siblings.iter().position(|index| *index == owner).unwrap();
    targets.projects[target.project].ordinary_tasks[position].clone()
}
fn ensure(condition: bool, message: &str) -> Result<(), String> {
    if condition {
        Ok(())
    } else {
        Err(message.into())
    }
}
fn actor_matches(actual: &crate::tracker::Actor, expected: &ActorExpectation) -> bool {
    match expected {
        ActorExpectation::User => matches!(actual, crate::tracker::Actor::User),
        ActorExpectation::Agent(agent) => actual.agent_id() == Some(agent.as_str()),
    }
}
fn assertion(field: &str, value: WitnessScalar) -> OracleAssertion {
    OracleAssertion {
        field: field.into(),
        actual: value.clone(),
        expected: value,
    }
}

#[path = "common_requests/accepted.rs"]
mod accepted;

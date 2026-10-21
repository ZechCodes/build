//! Overload-only quarantine. Native cancelled rows never become accepted tokens.
use super::super::rpc::ReplyProof;
use super::super::trial_complete::{ControlRefusal, Observation, Route, Slot, Traffic};
use super::saved_evidence::AcceptedRefusals;
use crate::timing::write_wait::{CompletedInterval, WriteId, WriteSnapshot};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;
#[cfg(test)]
pub(super) mod test_support;
#[cfg(test)]
mod tests;
mod verify;

#[derive(Clone, Debug)]
struct Expected {
    slot: Slot,
    method: String,
}
struct Candidate {
    refusal: ControlRefusal,
    raw: Value,
    intended: Instant,
    sent: Instant,
    decoded: Instant,
}
#[derive(Default)]
struct State {
    candidates: BTreeMap<String, Vec<Candidate>>,
    quarantined: BTreeMap<String, Vec<WriteSnapshot>>,
    errors: Vec<String>,
    finalized: bool,
    verified: Option<Arc<VerifiedRefusals>>,
}
pub(super) struct RefusalAudit {
    expected: BTreeMap<String, Expected>,
    state: Mutex<State>,
}
/// Only final verification below constructs these private fields. This is not
/// NativeCompletion, an ACK, an accepted save, or a normal-workload capability.
#[derive(Debug)]
pub(super) struct VerifiedRefusals {
    expected: BTreeMap<String, Expected>,
    rows: BTreeMap<String, VerifiedRefusal>,
}
#[derive(Debug)]
pub(super) struct VerifiedRefusal {
    original: String,
    method: String,
    write_id: WriteId,
    intended: Instant,
    sent: Instant,
    decoded: Instant,
    intake: Instant,
    classified: Instant,
    raw: Value,
    raw_artifact: String,
}
impl VerifiedRefusals {
    pub fn get(&self, original: &str) -> Option<&VerifiedRefusal> {
        self.rows.get(original)
    }
    pub fn originals(&self) -> impl Iterator<Item = &str> {
        self.rows.keys().map(String::as_str)
    }
    pub fn expected_method(&self, original: &str) -> Option<&str> {
        self.expected.get(original).map(|row| row.method.as_str())
    }
    pub fn matches_slot(&self, slot: &Slot) -> bool {
        self.expected
            .get(&slot.original_id)
            .is_some_and(|expected| {
                serde_json::to_value(&expected.slot)
                    .ok()
                    .zip(serde_json::to_value(slot).ok())
                    .is_some_and(|(a, b)| a == b)
            })
    }
}
impl VerifiedRefusal {
    pub fn original_id(&self) -> &str {
        &self.original
    }
    pub fn method(&self) -> &str {
        &self.method
    }
    pub fn write_id(&self) -> WriteId {
        self.write_id
    }
    pub fn intended_at(&self) -> Instant {
        self.intended
    }
    pub fn sent_at(&self) -> Instant {
        self.sent
    }
    pub fn decoded_at(&self) -> Instant {
        self.decoded
    }
    pub fn intake_at(&self) -> Instant {
        self.intake
    }
    pub fn classified_at(&self) -> Instant {
        self.classified
    }
    pub fn raw_reply(&self) -> &Value {
        &self.raw
    }
    pub fn raw_response_artifact(&self) -> &str {
        &self.raw_artifact
    }
}
impl RefusalAudit {
    pub fn new(expected: impl IntoIterator<Item = (Slot, String)>) -> Result<Arc<Self>, String> {
        let mut rows = BTreeMap::new();
        for (slot, method) in expected {
            if slot.traffic != Traffic::Write || slot.route != Route::SpaRpc || method.is_empty() {
                return Err(
                    "only immutable SPA semantic slots may refuse before carrier Full".into(),
                );
            }
            let original = slot.original_id.clone();
            if rows.insert(original, Expected { slot, method }).is_some() {
                return Err("duplicate overload refusal eligibility".into());
            }
        }
        Ok(Arc::new(Self {
            expected: rows,
            state: Mutex::new(State::default()),
        }))
    }
    /// Retain before deciding. Only cancelled rows of immutable SPA originals
    /// are withheld from the strict completion consumer; no proof is minted.
    pub fn quarantine(&self, row: &WriteSnapshot) -> bool {
        if !row.cancelled || !self.expected.contains_key(&row.original_id) {
            return false;
        }
        let mut state = self.state.lock().unwrap();
        if state.finalized {
            state.verified = None;
            state
                .errors
                .push("native refusal row arrived after final audit".into());
        }
        state
            .quarantined
            .entry(row.original_id.clone())
            .or_default()
            .push(row.clone());
        true
    }
    /// Actual bounded-Pump terminal rows for one declared original. No full
    /// registry snapshot or synthesized terminal evidence on this progress path.
    pub fn terminal_rows(&self, original: &str) -> Vec<WriteSnapshot> {
        self.state
            .lock()
            .unwrap()
            .quarantined
            .get(original)
            .cloned()
            .unwrap_or_default()
    }
    pub fn candidate(
        &self,
        slot: &Slot,
        method: &str,
        reply: &ReplyProof,
        refusal: ControlRefusal,
        intended: Instant,
    ) -> Result<(), String> {
        let mut state = self.state.lock().unwrap();
        let mismatch = state.finalized
            || self.expected.get(&slot.original_id).is_none_or(|expected| {
                expected.method != method
                    || serde_json::to_value(&expected.slot).ok() != serde_json::to_value(slot).ok()
            })
            || reply.original_id != slot.original_id
            || reply.method != method
            || reply.reply["id"] != slot.original_id
            || refusal.decoded_response != reply.reply;
        state
            .candidates
            .entry(slot.original_id.clone())
            .or_default()
            .push(Candidate {
                refusal,
                raw: reply.reply.clone(),
                intended,
                sent: reply.sent_at,
                decoded: reply.final_reply_at,
            });
        if mismatch {
            state.verified = None;
            let error = "overload refusal candidate differs from immutable SPA request".to_owned();
            state.errors.push(error.clone());
            return Err(error);
        }
        Ok(())
    }
    pub fn verified(&self) -> Option<Arc<VerifiedRefusals>> {
        self.state.lock().unwrap().verified.clone()
    }
    pub fn confirm(&self, slot: &Slot, method: &str) -> Result<(), String> {
        let verified = self
            .verified()
            .ok_or("final closed native refusal token missing")?;
        if !verified.matches_slot(slot)
            || verified.expected_method(&slot.original_id) != Some(method)
            || verified
                .get(&slot.original_id)
                .is_none_or(|proof| proof.method() != method)
        {
            return Err("final closed refusal token does not match original slot/method".into());
        }
        Ok(())
    }
    pub fn finish(
        &self,
        rows: &[WriteSnapshot],
        intervals: &[CompletedInterval],
        observe: &Observation,
        accepted: Option<&AcceptedRefusals>,
    ) -> Result<Arc<VerifiedRefusals>, String> {
        let mut state = self.state.lock().unwrap();
        if state.finalized {
            state.verified = None;
            return Err("overload refusal final audit repeated".into());
        }
        state.finalized = true;
        let result = verify::all(&self.expected, &state, rows, intervals, observe, accepted);
        match result {
            Ok(verified) => {
                let verified = Arc::new(verified);
                state.verified = Some(Arc::clone(&verified));
                Ok(verified)
            }
            Err(error) => {
                state.errors.push(error.clone());
                Err(error)
            }
        }
    }
}

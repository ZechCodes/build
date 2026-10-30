//! One attempt's bounded early ICE queue. Session and peer authority live in its
//! owner; this module never interprets keys, session identifiers or generations.

use std::io::{self, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;

use serde_json::Value;

/// Includes the single end marker and candidates drained into another hold.
pub const MAX_PENDING_ICE_CANDIDATES: usize = 128;
pub const MAX_PENDING_ICE_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum EarlyIceError {
    #[error("early ICE attempt closed")]
    Closed,
    #[error("early ICE setup deadline expired")]
    Deadline,
    #[error("early ICE candidate limit exceeded")]
    CandidateLimit,
    #[error("early ICE serialized byte limit exceeded")]
    ByteLimit,
    #[error("early ICE requires a candidate object or the null end marker")]
    MalformedCandidate,
}

#[derive(Debug, Default)]
struct Budget {
    candidates: AtomicUsize,
    bytes: AtomicUsize,
}

/// Keep this reservation in any downstream relay/native hold, and drop it only
/// after physical delivery or cancellation. Draining alone releases no budget.
#[derive(Debug)]
#[must_use = "keep the reservation until ICE is physically delivered or cancelled"]
pub struct IceReservation {
    budget: Arc<Budget>,
    bytes: usize,
}

impl Drop for IceReservation {
    fn drop(&mut self) {
        self.budget.candidates.fetch_sub(1, Ordering::Relaxed);
        self.budget.bytes.fetch_sub(self.bytes, Ordering::Relaxed);
    }
}

/// A candidate and its retained budget. This lease is accounting, not authority:
/// its owner must still fence delivery by the original attempt and deadline.
#[derive(Debug)]
#[must_use = "retain the candidate's budget until physical delivery or cancellation"]
pub struct RetainedIce {
    candidate: Value,
    reservation: IceReservation,
}

impl RetainedIce {
    pub fn candidate(&self) -> &Value {
        &self.candidate
    }

    /// Move into a downstream hold without cloning the JSON or freeing budget.
    pub fn into_parts(self) -> (Value, IceReservation) {
        (self.candidate, self.reservation)
    }
}

/// Owned by one immutable peer attempt. Null is the sole end marker; normalize
/// native empty-candidate controls to null before pushing them here. A repeated
/// null is idempotent for this attempt, even after its first marker is delivered.
/// All failures are terminal and erase queued candidates. Outstanding drained
/// leases belong to the owner, which must cancel its downstream holds as well.
#[derive(Debug)]
pub struct PendingIce {
    deadline: Instant,
    pending: Vec<RetainedIce>,
    budget: Arc<Budget>,
    end_seen: bool,
    failure: Option<EarlyIceError>,
}

impl PendingIce {
    pub fn new(deadline: Instant) -> Self {
        Self {
            deadline,
            pending: Vec::new(),
            budget: Arc::default(),
            end_seen: false,
            failure: None,
        }
    }

    pub fn push(&mut self, candidate: Value, now: Instant) -> Result<(), EarlyIceError> {
        self.check(now)?;
        if candidate.is_null() && self.end_seen {
            return Ok(());
        }
        if !valid_candidate(&candidate) {
            return self.fail(EarlyIceError::MalformedCandidate);
        }
        if self.budget.candidates.load(Ordering::Relaxed) >= MAX_PENDING_ICE_CANDIDATES {
            return self.fail(EarlyIceError::CandidateLimit);
        }
        let available = MAX_PENDING_ICE_BYTES - self.budget.bytes.load(Ordering::Relaxed);
        let bytes = match serialized_bytes(&candidate, available) {
            Ok(bytes) => bytes,
            Err(error) => return self.fail(error),
        };
        self.end_seen |= candidate.is_null();
        // Only this mutable queue reserves; leases on other threads only release.
        // Independent relaxed counters may temporarily overcount, never admit
        // more retained candidates or bytes than their respective bounds.
        self.budget.candidates.fetch_add(1, Ordering::Relaxed);
        self.budget.bytes.fetch_add(bytes, Ordering::Relaxed);
        self.pending.push(RetainedIce {
            candidate,
            reservation: IceReservation {
                budget: self.budget.clone(),
                bytes,
            },
        });
        Ok(())
    }

    pub fn drain(&mut self, now: Instant) -> Result<Vec<RetainedIce>, EarlyIceError> {
        self.check(now)?;
        Ok(std::mem::take(&mut self.pending))
    }

    pub fn close(&mut self) {
        let _ = self.fail::<()>(EarlyIceError::Closed);
    }

    fn check(&mut self, now: Instant) -> Result<(), EarlyIceError> {
        if let Some(failure) = self.failure {
            return Err(failure);
        }
        if now >= self.deadline {
            return self.fail(EarlyIceError::Deadline);
        }
        Ok(())
    }

    fn fail<T>(&mut self, error: EarlyIceError) -> Result<T, EarlyIceError> {
        let failure = *self.failure.get_or_insert(error);
        self.pending.clear();
        Err(failure)
    }
}

fn valid_candidate(candidate: &Value) -> bool {
    candidate.is_null()
        || candidate
            .as_object()
            .and_then(|object| object.get("candidate"))
            .and_then(Value::as_str)
            .is_some_and(|text| !text.is_empty())
}

/// Count the serialized UTF-8, including JSON escaping, without allocating a
/// second candidate-sized string. Stop serialization at the remaining budget.
fn serialized_bytes(candidate: &Value, available: usize) -> Result<usize, EarlyIceError> {
    let mut counter = ByteCounter {
        bytes: 0,
        available,
    };
    serde_json::to_writer(&mut counter, candidate).map_err(|_| EarlyIceError::ByteLimit)?;
    Ok(counter.bytes)
}

struct ByteCounter {
    bytes: usize,
    available: usize,
}

impl Write for ByteCounter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.available - self.bytes {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "early ICE byte limit",
            ));
        }
        self.bytes += bytes.len();
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::{Duration, Instant};

    fn candidate(index: usize) -> Value {
        json!({ "candidate": format!("candidate:{index}"), "sdpMid": "0" })
    }

    fn queue(now: Instant) -> PendingIce {
        PendingIce::new(now + Duration::from_secs(10))
    }

    #[test]
    fn drain_preserves_candidate_and_end_marker_order() {
        let now = Instant::now();
        let mut pending = queue(now);
        for value in [candidate(1), candidate(2), Value::Null] {
            pending.push(value, now).unwrap();
        }
        let leases = pending.drain(now).unwrap();
        let values: Vec<_> = leases
            .iter()
            .map(|lease| lease.candidate().clone())
            .collect();
        assert_eq!(values, vec![candidate(1), candidate(2), Value::Null]);
        assert!(pending.drain(now).unwrap().is_empty());
    }

    #[test]
    fn end_marker_is_idempotent_across_drain_and_delivery() {
        let now = Instant::now();
        let mut pending = queue(now);
        pending.push(Value::Null, now).unwrap();
        pending.push(Value::Null, now).unwrap();
        assert_eq!(pending.drain(now).unwrap().len(), 1);
        pending.push(Value::Null, now).unwrap();
        assert!(pending.drain(now).unwrap().is_empty());
    }

    #[test]
    fn the_128_frame_limit_includes_the_end_marker() {
        let now = Instant::now();
        let mut pending = queue(now);
        for index in 0..127 {
            pending.push(candidate(index), now).unwrap();
        }
        pending.push(Value::Null, now).unwrap();
        assert_eq!(
            pending.push(candidate(128), now),
            Err(EarlyIceError::CandidateLimit)
        );
        assert!(pending.pending.is_empty());
        assert_eq!(
            pending.drain(now).unwrap_err(),
            EarlyIceError::CandidateLimit
        );
    }

    #[test]
    fn draining_into_a_relay_hold_keeps_the_count_reserved() {
        let now = Instant::now();
        let mut pending = queue(now);
        for index in 0..128 {
            pending.push(candidate(index), now).unwrap();
        }
        let leases = pending.drain(now).unwrap();
        assert_eq!(leases.len(), 128);
        assert_eq!(
            pending.push(candidate(129), now),
            Err(EarlyIceError::CandidateLimit)
        );
    }

    #[test]
    fn physical_delivery_releases_each_candidate_reservation() {
        let now = Instant::now();
        let mut pending = queue(now);
        for index in 0..128 {
            pending.push(candidate(index), now).unwrap();
        }
        let mut leases = pending.drain(now).unwrap();
        let (value, reservation) = leases.pop().unwrap().into_parts();
        assert_eq!(value, candidate(127));
        drop(value);
        assert_eq!(pending.budget.candidates.load(Ordering::Relaxed), 128);
        drop(reservation);
        pending.push(candidate(129), now).unwrap();
        assert_eq!(pending.drain(now).unwrap()[0].candidate(), &candidate(129));
    }

    fn full_budget_candidate() -> Value {
        let overhead = serde_json::to_vec(&json!({ "candidate": "" }))
            .unwrap()
            .len();
        let value = json!({ "candidate": "é".repeat((MAX_PENDING_ICE_BYTES - overhead) / 2) });
        assert_eq!(
            serde_json::to_vec(&value).unwrap().len(),
            MAX_PENDING_ICE_BYTES
        );
        value
    }

    #[test]
    fn the_byte_cap_counts_utf8_and_json_overhead() {
        let now = Instant::now();
        let mut pending = queue(now);
        pending.push(full_budget_candidate(), now).unwrap();
        assert_eq!(
            pending.push(Value::Null, now),
            Err(EarlyIceError::ByteLimit)
        );
        assert!(pending.pending.is_empty());
        assert_eq!(
            pending.push(candidate(1), now),
            Err(EarlyIceError::ByteLimit)
        );
    }

    #[test]
    fn a_single_oversized_candidate_fails_without_retaining_it() {
        let now = Instant::now();
        let mut pending = queue(now);
        let oversized = json!({ "candidate": "é".repeat(MAX_PENDING_ICE_BYTES) });
        assert_eq!(pending.push(oversized, now), Err(EarlyIceError::ByteLimit));
        assert!(pending.pending.is_empty());
        assert_eq!(pending.budget.bytes.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn draining_keeps_bytes_reserved_until_physical_delivery() {
        let now = Instant::now();
        let mut pending = queue(now);
        pending.push(full_budget_candidate(), now).unwrap();
        let leases = pending.drain(now).unwrap();
        assert_eq!(
            pending.push(Value::Null, now),
            Err(EarlyIceError::ByteLimit)
        );
        drop(leases);
        assert_eq!(
            pending.push(Value::Null, now),
            Err(EarlyIceError::ByteLimit)
        );
    }

    #[test]
    fn releasing_a_delivery_recovers_its_byte_budget() {
        let now = Instant::now();
        let mut pending = queue(now);
        pending.push(full_budget_candidate(), now).unwrap();
        drop(pending.drain(now).unwrap());
        pending.push(candidate(1), now).unwrap();
    }

    #[test]
    fn deadline_is_original_finite_and_failure_is_permanent() {
        let now = Instant::now();
        let mut pending = queue(now);
        pending
            .push(candidate(1), now + Duration::from_secs(9))
            .unwrap();
        assert_eq!(
            pending.drain(now + Duration::from_secs(10)).unwrap_err(),
            EarlyIceError::Deadline
        );
        assert!(pending.pending.is_empty());
        assert_eq!(
            pending.push(candidate(2), now),
            Err(EarlyIceError::Deadline)
        );
    }

    #[test]
    fn push_cannot_admit_a_candidate_at_the_deadline() {
        let now = Instant::now();
        let mut pending = PendingIce::new(now);
        assert_eq!(
            pending.push(candidate(1), now),
            Err(EarlyIceError::Deadline)
        );
    }

    #[test]
    fn close_clears_the_queue_and_prevents_further_frames() {
        let now = Instant::now();
        let mut pending = queue(now);
        pending.push(candidate(1), now).unwrap();
        pending.close();
        assert!(pending.pending.is_empty());
        assert_eq!(pending.drain(now).unwrap_err(), EarlyIceError::Closed);
        assert_eq!(pending.push(Value::Null, now), Err(EarlyIceError::Closed));
    }

    #[test]
    fn only_candidate_objects_and_the_canonical_null_end_marker_are_accepted() {
        let now = Instant::now();
        for malformed in [
            json!("candidate"),
            json!({}),
            json!({ "candidate": "" }),
            json!({ "candidate": 3 }),
        ] {
            let mut pending = queue(now);
            assert_eq!(
                pending.push(malformed, now),
                Err(EarlyIceError::MalformedCandidate)
            );
        }
    }
}

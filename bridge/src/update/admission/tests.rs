use super::*;
use crate::test_support::off_lock::OffLockGate;
use std::sync::mpsc;
use std::time::{Duration, Instant};

#[test]
fn idle_probe_refuses_new_writers_without_waiting_for_its_callback() {
    let gate = AdmissionGate::new(false);
    let (probe, held_probe) = OffLockGate::new();
    let claiming = Arc::clone(&gate);
    let claim = std::thread::spawn(move || {
        claiming.claim_idle(|| {
            probe.arrive();
            true
        })
    });
    held_probe.wait_for_arrival();
    let (finished, observed) = mpsc::channel();
    let writer = std::thread::spawn(move || {
        let started = Instant::now();
        finished
            .send((gate.try_enter().is_none(), started.elapsed()))
            .unwrap();
    });
    let answer = observed.recv_timeout(Duration::from_millis(100));
    drop(held_probe);
    assert!(!claim.join().unwrap());
    writer.join().unwrap();
    let (refused, elapsed) = answer.expect("writer admission must not wait for the busy callback");
    assert!(refused, "new non-read work must not race an idle claim");
    assert!(elapsed <= Duration::from_millis(100));
}

#[test]
fn idle_probe_reopen_invalidates_its_result_before_a_later_claim() {
    let gate = AdmissionGate::new(false);
    let (first_probe, held_first) = OffLockGate::new();
    let first_gate = Arc::clone(&gate);
    let first = std::thread::spawn(move || {
        first_gate.claim_idle(|| {
            first_probe.arrive();
            false
        })
    });
    held_first.wait_for_arrival();
    let (reopened, observed) = mpsc::channel();
    let reopening = Arc::clone(&gate);
    let reopen = std::thread::spawn(move || {
        reopening.reopen();
        reopened.send(()).unwrap();
    });
    let promptly = observed.recv_timeout(Duration::from_millis(100));
    if promptly.is_err() {
        drop(held_first);
        first.join().unwrap();
        reopen.join().unwrap();
        panic!("reopen waited for the busy callback");
    }
    reopen.join().unwrap();
    let (next_probe, held_next) = OffLockGate::new();
    let next_gate = Arc::clone(&gate);
    let next = std::thread::spawn(move || {
        next_gate.claim_idle(|| {
            next_probe.arrive();
            false
        })
    });
    held_next.wait_for_arrival();
    drop(held_first);
    let stale_closed = first.join().unwrap();
    let writer_refused = gate.try_enter().is_none();
    drop(held_next);
    let current_closed = next.join().unwrap();
    assert!(
        !stale_closed,
        "a cancelled probe must not close a later claim"
    );
    assert!(
        writer_refused,
        "the later claim must retain its write reservation"
    );
    assert!(current_closed);
    assert!(gate.try_enter().is_none());
    gate.reopen();
    assert!(gate.try_enter().is_some());
}

#[test]
fn idle_probe_panic_restores_admission_without_poisoning_the_gate() {
    let gate = AdmissionGate::new(false);
    let failed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        gate.claim_idle(|| panic!("controlled busy probe panic"))
    }));
    assert!(failed.is_err());
    assert!(gate.try_enter().is_some());
}

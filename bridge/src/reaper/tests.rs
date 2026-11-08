use super::*;
use crate::harness::{AgentStatus, HarnessError, Turn};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;

struct HeldEnd {
    entered: mpsc::Sender<()>,
    release: Mutex<mpsc::Receiver<()>>,
    ended: AtomicBool,
}

impl AgentSession for HeldEnd {
    fn send_turn(&self, _: &Turn) -> Result<(), HarnessError> {
        Ok(())
    }
    fn status(&self) -> AgentStatus {
        if self.ended.load(Ordering::SeqCst) {
            AgentStatus::Ended { code: Some(0) }
        } else {
            AgentStatus::Working
        }
    }
    fn quiet_for(&self) -> Duration {
        Duration::ZERO
    }
    fn exited_within(&self, _: Duration) -> bool {
        self.ended.load(Ordering::SeqCst)
    }
    fn backdate_last_output(&self, _: Duration) {}
    fn end(&self) {
        self.entered.send(()).unwrap();
        self.release.lock().unwrap().recv().ok();
        self.ended.store(true, Ordering::SeqCst);
    }
}

#[test]
fn retirement_is_reaped_observes_a_held_end_without_waiting() {
    let (entered, arrived) = mpsc::channel();
    let (release, blocked) = mpsc::channel();
    let receipt = Retirement::begin(Arc::new(HeldEnd {
        entered,
        release: Mutex::new(blocked),
        ended: AtomicBool::new(false),
    }));
    arrived.recv_timeout(Duration::from_secs(5)).unwrap();
    let (observed, observation) = mpsc::channel();
    let reading = receipt.clone();
    let probe = std::thread::spawn(move || observed.send(reading.is_reaped()).unwrap());
    let pending = observation.recv_timeout(Duration::from_secs(5));
    release.send(()).unwrap();
    assert!(receipt.wait(Duration::from_secs(5)));
    probe.join().unwrap();
    assert!(!pending.unwrap());
    assert!(receipt.is_reaped());
}

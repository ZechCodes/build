//! A stable config epoch for off-lock membership refresh. Writers announce
//! before SQL/rename and finish after publication; checks run under the store
//! writer mutex, so a later config write cannot be overwritten by an old view.
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Condvar, Mutex,
};

#[derive(Clone, Default)]
pub(in crate::app) struct MembershipRevision(Arc<State>);
#[derive(Default)]
struct State {
    version: Mutex<u64>,
    ready: Condvar,
}
pub(in crate::app) struct MembershipWrite {
    revision: MembershipRevision,
    finished: AtomicBool,
}
impl MembershipRevision {
    pub(in crate::app) fn snapshot(&self) -> u64 {
        *self.0.version.lock().unwrap()
    }
    pub(in crate::app) fn begin(&self) -> Result<MembershipWrite, String> {
        let mut version = self.0.version.lock().unwrap();
        if *version & 1 != 0 {
            return Err("configuration write in progress; retry".into());
        }
        *version = version.wrapping_add(1);
        Ok(MembershipWrite {
            revision: self.clone(),
            finished: AtomicBool::new(false),
        })
    }
    pub(in crate::app) fn current(&self, expected: u64) -> bool {
        expected & 1 == 0 && self.snapshot() == expected
    }
    /// Wait only off-lock. A config slot publishes memory before waking us;
    /// callers then recapture if their original epoch was superseded.
    pub(in crate::app) fn wait_current(&self, expected: u64) -> bool {
        let mut version = self.0.version.lock().unwrap();
        while *version & 1 != 0 {
            version = self.0.ready.wait(version).unwrap();
        }
        *version == expected && expected & 1 == 0
    }
}
impl MembershipWrite {
    pub(in crate::app) fn finish(&self) {
        if !self.finished.swap(true, Ordering::AcqRel) {
            let mut version = self.revision.0.version.lock().unwrap();
            *version = version.wrapping_add(1);
            drop(version);
            self.revision.0.ready.notify_all();
        }
    }
}
impl Drop for MembershipWrite {
    fn drop(&mut self) {
        self.finish();
    }
}

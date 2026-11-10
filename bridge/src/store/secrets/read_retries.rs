//! Refused metadata reads retain only bounded scope hints, never request data.
use crate::api::v1::secrets::SecretScope;
use crate::secrets::Result;
use rusqlite::Connection;
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

type Wake = Arc<dyn Fn() + Send + Sync>;
#[derive(Default)]
pub(crate) struct SecretReadRetries {
    pending: Mutex<BTreeSet<Option<String>>>,
    wake: Mutex<Option<Wake>>,
}
impl SecretReadRetries {
    pub(crate) fn set_wake(&self, wake: Wake) {
        *self.wake.lock().unwrap() = Some(wake);
    }
    pub(super) fn record(&self, connection: &Connection, scope: &SecretScope) -> Result<()> {
        let (known, count): (bool, i64) = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM device_project_ids WHERE project_id=?1),COUNT(*) FROM device_project_ids",
            [&scope.project_id], |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let key = (known && scope.environment_id.is_none()).then(|| scope.project_id.clone());
        {
            let mut pending = self.pending.lock().unwrap();
            if !pending.contains(&None) && !pending.contains(&key) {
                if key.is_none() || pending.len() >= count as usize {
                    pending.clear();
                    pending.insert(None);
                } else {
                    pending.insert(key);
                }
            }
        }
        // Wake even for an old snapshot that observes dirty after clear/drain.
        let wake = self.wake.lock().unwrap().clone();
        if let Some(wake) = wake {
            wake();
        }
        Ok(())
    }
    #[cfg(test)]
    pub(crate) fn pending_count(&self) -> usize {
        self.pending.lock().unwrap().len()
    }
    pub(crate) fn take(&self) -> BTreeSet<Option<String>> {
        std::mem::take(&mut *self.pending.lock().unwrap())
    }
}

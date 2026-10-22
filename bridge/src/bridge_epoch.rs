//! One boot identity shared by every authenticated sender in this process.

use std::sync::{Arc, OnceLock};

/// The daemon's boot identity is independent of sessions, carriers and stores.
/// In particular, recreating an intake or app state cannot rotate this fence.
pub(crate) fn current() -> &'static str {
    static EPOCH: OnceLock<Arc<str>> = OnceLock::new();
    EPOCH.get_or_init(new_boot)
}

/// Fresh boot construction also lets tests model a restart without changing
/// the shared process identity underneath concurrently running tests.
pub(crate) fn new_boot() -> Arc<str> {
    uuid::Uuid::new_v4().to_string().into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_process_epoch_is_stable_and_a_new_boot_is_distinct() {
        let epoch = current();
        assert_eq!(epoch, current());
        assert_eq!(epoch.len(), 36);
        assert_ne!(epoch, new_boot().as_ref());
        assert_ne!(new_boot(), new_boot());
    }
}

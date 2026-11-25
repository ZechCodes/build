//! Browser reachability from relay signaling or the hosted HTTPS endpoint.
//! Each transport owns its flag. The aggregate wakes presence and resume
//! when either transport becomes reachable.

use std::sync::{Arc, Mutex};

use tokio::sync::watch;

/// A shared "this device is findable" flag. Cloning shares the flag, so the
/// relay client and the presence reporter hold the same one.
#[derive(Clone, Debug)]
pub struct Reachability {
    state: Arc<ReachabilityState>,
    source: usize,
}

#[derive(Debug)]
struct ReachabilityState {
    sources: Mutex<[bool; 2]>,
    reachable: watch::Sender<bool>,
}

impl Default for Reachability {
    fn default() -> Self {
        Self {
            state: Arc::new(ReachabilityState {
                sources: Mutex::new([false; 2]),
                reachable: watch::channel(false).0,
            }),
            source: 0,
        }
    }
}

impl Reachability {
    /// A device with no authenticated relay or acknowledged hosted endpoint.
    pub fn unreachable() -> Self {
        Self::default()
    }

    /// A distinct HTTPS source of the same browser reachability.
    pub fn via_hosted(&self) -> Self {
        Self {
            state: self.state.clone(),
            source: 1,
        }
    }

    pub fn reached(&self) {
        self.set(true);
    }

    pub fn lost(&self) {
        self.set(false);
    }

    fn set(&self, reachable: bool) {
        let mut sources = self.state.sources.lock().unwrap();
        sources[self.source] = reachable;
        let any = sources.iter().any(|source| *source);
        self.state.reachable.send_if_modified(|last| {
            *last != any && {
                *last = any;
                true
            }
        });
    }

    /// Whether a browser could reach this device right now.
    pub fn is_reachable(&self) -> bool {
        *self.state.reachable.borrow()
    }

    /// A receiver that wakes on every change of the answer — unreachable to
    /// reachable or back, never for a repeat of the same one. It starts having
    /// seen the answer as it stands.
    pub fn watch(&self) -> watch::Receiver<bool> {
        self.state.reachable.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosted_reachability_and_relay_each_keep_the_device_reachable() {
        let relay = Reachability::unreachable();
        let hosted = relay.via_hosted();
        let mut changes = relay.watch();

        for (first, second) in [(&relay, &hosted), (&hosted, &relay)] {
            first.reached();
            assert!(changes.has_changed().unwrap());
            assert!(*changes.borrow_and_update());

            second.reached();
            first.lost();
            assert!(first.is_reachable(), "the remaining source is still live");
            assert!(
                !changes.has_changed().unwrap(),
                "partial loss must not wake readers"
            );

            second.lost();
            assert!(changes.has_changed().unwrap());
            assert!(!*changes.borrow_and_update());
        }
    }

    #[test]
    fn a_device_is_unreachable_until_a_relay_has_authenticated_it() {
        let reachability = Reachability::unreachable();
        assert!(!reachability.is_reachable());
        reachability.reached();
        assert!(reachability.is_reachable());
    }

    #[test]
    fn losing_the_socket_makes_the_device_unreachable_again() {
        let reachability = Reachability::unreachable();
        reachability.reached();
        reachability.lost();
        assert!(!reachability.is_reachable());
    }

    #[test]
    fn a_clone_is_the_same_flag_so_both_halves_read_one_answer() {
        let held_by_the_relay_client = Reachability::unreachable();
        let held_by_the_beat = held_by_the_relay_client.clone();
        held_by_the_relay_client.reached();
        assert!(held_by_the_beat.is_reachable());
        held_by_the_relay_client.lost();
        assert!(!held_by_the_beat.is_reachable());
    }

    #[tokio::test]
    async fn a_watcher_wakes_on_each_change_and_not_on_a_repeat() {
        let reachability = Reachability::unreachable();
        let mut watching = reachability.watch();
        assert!(!watching.has_changed().unwrap());
        reachability.lost();
        assert!(
            !watching.has_changed().unwrap(),
            "lost while lost is no change"
        );
        reachability.reached();
        watching.changed().await.unwrap();
        assert!(*watching.borrow_and_update());
        reachability.reached();
        assert!(
            !watching.has_changed().unwrap(),
            "reached while reached is no change"
        );
        reachability.lost();
        watching.changed().await.unwrap();
        assert!(!*watching.borrow_and_update());
    }

    #[test]
    fn the_default_is_unreachable() {
        assert!(!Reachability::default().is_reachable());
    }
}

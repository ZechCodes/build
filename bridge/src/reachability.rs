//! Whether this device can be reached — the one fact the relay socket and the
//! heartbeat have to agree on.
//!
//! `online` in the account's device list is read as "I can dial this machine".
//! It used to answer a different question: the heartbeat task was started once
//! at boot and posted to the api over its own HTTPS connection, knowing nothing
//! about the relay socket a browser actually finds the device on. So a bridge
//! whose relay socket had been severed kept beating, the api kept saying
//! online, the browser kept dialling, and the relay kept answering that it
//! holds no such device.
//!
//! This is the flag that closes that gap. The relay client raises it when the
//! relay has authenticated the socket and drops it the moment that socket ends,
//! however it ends; `presence.rs` beats only while it is raised. Presence
//! stays the api's (`planning/v2/Strict P2P Transport Spec.md` rule 6) — the
//! relay still reports nothing about any device — but what the device reports
//! about itself is now reachability rather than mere liveness.
//!
//! The flag is also something to wait on ([`Reachability::watch`]): the beat
//! goes out the moment the relay authenticates the socket rather than at its
//! next tick, which on a cold start was a whole interval away (#321).

use std::sync::Arc;

use tokio::sync::watch;

/// A shared "this device is findable" flag. Cloning shares the flag, so the
/// relay client and the presence reporter hold the same one.
#[derive(Clone, Debug)]
pub struct Reachability(Arc<watch::Sender<bool>>);

impl Default for Reachability {
    fn default() -> Self {
        Self(Arc::new(watch::channel(false).0))
    }
}

impl Reachability {
    /// A device nobody has reached yet. The default is the under-reporting one
    /// on purpose: a bridge that has never been authenticated by a relay cannot
    /// be dialled, and saying so costs one heartbeat window, while the opposite
    /// mistake costs a browser that dials a machine that is not there.
    pub fn unreachable() -> Self {
        Self::default()
    }

    /// The relay authenticated this device's socket: it can be dialled.
    pub fn reached(&self) {
        self.0
            .send_if_modified(|reachable| !std::mem::replace(reachable, true));
    }

    /// That socket has ended, however it ended — closed, severed, timed out, or
    /// the future running it dropped. Nothing is posted to say so: the beats
    /// stop, and every reader past the api's window sees the device go.
    pub fn lost(&self) {
        self.0
            .send_if_modified(|reachable| std::mem::replace(reachable, false));
    }

    /// Whether a browser could reach this device right now.
    pub fn is_reachable(&self) -> bool {
        *self.0.borrow()
    }

    /// A receiver that wakes on every change of the answer — unreachable to
    /// reachable or back, never for a repeat of the same one. It starts having
    /// seen the answer as it stands.
    pub fn watch(&self) -> watch::Receiver<bool> {
        self.0.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

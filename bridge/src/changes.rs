//! Push invalidation: the bridge telling every connected browser that
//! something changed, instead of waiting to be polled.
//!
//! Two events go out on the browser's own session, in the frame shape it
//! already parses for terminal pushes (a payload with a `type`):
//!
//! ```text
//! {"type":"board.changed"}                    feed-level state moved
//! {"type":"entity.changed","id":"run-7"}      one entity's detail moved
//! ```
//!
//! `board.changed` says the feed is stale — a task lifecycle transition, the
//! inbox/attention map, a capture, an agent coming or going. `entity.changed`
//! says one issue/branch/run's thread, stages, git state or diff is stale. A
//! client that holds both refetches what it is showing; nothing about WHAT
//! changed rides the wire, so the events stay content-free like every other
//! signal Build sends about work it cannot read.
//!
//! **Terminal output is not a change.** It has its own push path
//! (`term.output` / `term.reset`), and routing a byte storm through here would
//! turn a repainting TUI into an invalidation storm.
//!
//! # The two halves, and why they are separate
//!
//! Noting a change happens deep inside mutations that run holding the app
//! mutex. Sending one encrypts a frame per subscriber. So [`ChangeBus::note`]
//! only inserts a key into a set behind a leaf mutex — no I/O, no encryption,
//! nothing that can block on anything but itself — and [`ChangeBus::flush`],
//! driven by a task that holds no app lock, does the sending.
//!
//! That split is also the coalescer. A flush collapses everything noted since
//! the last one into ONE event per key, and the driver
//! ([`ChangeBus::spawn_flusher`]) flushes at most once per
//! [`ChangeBus::window`]: the first change on an idle bus goes out at once,
//! and a burst behind it costs one event per key per window rather than one
//! per mutation.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

use crate::carrier::SessionSender;

/// How long changes collapse together before the next flush. Short enough that
/// a browser reacts as if it were watching, long enough that a mutation storm
/// (a stage sweep, an agent writing a file a second) costs a handful of frames.
pub const DEFAULT_COALESCE_WINDOW: Duration = Duration::from_millis(250);

/// The most keys one un-flushed window holds before it gives up on precision.
///
/// Only reachable with no flusher running (a bridge built outside an async
/// runtime), where the set would otherwise grow with every distinct entity for
/// the life of the process. Past the cap the whole batch collapses to
/// [`ChangeKey::Board`], which already tells a client to refetch everything.
const PENDING_KEY_CAP: usize = 512;

/// What changed, at the grain a browser refetches in.
///
/// `Ord` (and the variant order) is the wire order of one flush: the feed
/// first, then entities by id — so a client that reloads the feed and the
/// entity it is showing does it in that order.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ChangeKey {
    /// Feed-level state: task lifecycle, inbox/attention, capture, agent
    /// liveness — anything the board reads.
    Board,
    /// One issue/branch/run: its thread, stages, git state or diff.
    Entity(String),
}

impl ChangeKey {
    /// The payload a browser receives for this key.
    pub fn payload(&self) -> Value {
        match self {
            ChangeKey::Board => json!({ "type": "board.changed" }),
            ChangeKey::Entity(id) => json!({ "type": "entity.changed", "id": id }),
        }
    }
}

/// The change events a browser session can be told about, announced in the
/// `session.hello` greeting so a client knows what it may hear.
pub const ANNOUNCED_EVENTS: [&str; 2] = ["board.changed", "entity.changed"];

/// What one un-flushed window holds.
#[derive(Default)]
struct Pending {
    /// Noted since the last flush. A set, so a thousand notes of one key are
    /// one event.
    keys: BTreeSet<ChangeKey>,
    /// Past [`PENDING_KEY_CAP`] the window gave up naming entities and stands
    /// as a bare [`ChangeKey::Board`] until it is flushed. Latched, so the
    /// notes that keep arriving cannot start refilling the set behind it.
    collapsed: bool,
}

/// Every browser session that asked for push invalidation, and the changes
/// waiting to reach them.
pub struct ChangeBus {
    /// One entry per subscribed session, keyed by its session id. Dropped when
    /// a push fails: a sender that cannot send has no connection left.
    subscribers: Mutex<Vec<SessionSender>>,
    pending: Mutex<Pending>,
    /// Set whenever `pending` gains its first key, so the flusher wakes on the
    /// change rather than on a tick.
    wake: tokio::sync::Notify,
    window: Duration,
}

impl ChangeBus {
    /// A bus that coalesces over `window`.
    pub fn new(window: Duration) -> Arc<Self> {
        Arc::new(ChangeBus {
            subscribers: Mutex::new(Vec::new()),
            pending: Mutex::new(Pending::default()),
            wake: tokio::sync::Notify::new(),
            window,
        })
    }

    /// How long changes collapse together before the next flush.
    pub fn window(&self) -> Duration {
        self.window
    }

    /// Start hearing change events on this session. Re-subscribing a session id
    /// replaces its sender — a reconnected browser keeps one subscription, not
    /// two.
    pub fn subscribe(&self, sender: &SessionSender) {
        let mut subscribers = self.subscribers.lock().unwrap();
        subscribers.retain(|existing| existing.session_id() != sender.session_id());
        subscribers.push(sender.clone());
    }

    /// Stop hearing them — the session closed.
    pub fn unsubscribe(&self, session_id: &str) {
        self.subscribers
            .lock()
            .unwrap()
            .retain(|existing| existing.session_id() != session_id);
    }

    /// How many sessions are subscribed.
    pub fn subscriber_count(&self) -> usize {
        self.subscribers.lock().unwrap().len()
    }

    /// Record that something changed.
    ///
    /// SAFE UNDER THE APP MUTEX, and the reason this type exists: it takes one
    /// leaf mutex, inserts into a set, and returns. No encryption, no channel a
    /// slow reader can fill, no I/O of any kind — nothing a caller holding the
    /// app lock could block the whole daemon on.
    pub fn note(&self, key: ChangeKey) {
        let mut pending = self.pending.lock().unwrap();
        if pending.collapsed {
            return;
        }
        if pending.keys.len() >= PENDING_KEY_CAP {
            pending.collapsed = true;
            pending.keys.clear();
            pending.keys.insert(ChangeKey::Board);
        } else {
            pending.keys.insert(key);
        }
        drop(pending);
        self.wake.notify_one();
    }

    /// The feed is stale.
    pub fn note_board(&self) {
        self.note(ChangeKey::Board);
    }

    /// This entity is stale — and so is the feed, which shows a row for it.
    pub fn note_entity(&self, id: &str) {
        self.note(ChangeKey::Entity(id.to_string()));
        self.note(ChangeKey::Board);
    }

    /// Whether anything is waiting to go out.
    pub fn has_pending(&self) -> bool {
        !self.pending.lock().unwrap().keys.is_empty()
    }

    /// Send one event per distinct key noted since the last flush, and drop
    /// every subscriber whose connection is gone. Returns the number of events
    /// sent per subscriber (0 when nothing was pending).
    ///
    /// MUST NOT run holding the app mutex: it encrypts a frame per subscriber
    /// per key.
    pub fn flush(&self) -> usize {
        let keys: Vec<ChangeKey> = {
            let mut pending = self.pending.lock().unwrap();
            if pending.keys.is_empty() {
                return 0;
            }
            std::mem::take(&mut *pending).keys.into_iter().collect()
        };
        let payloads: Vec<Value> = keys.iter().map(ChangeKey::payload).collect();
        let mut subscribers = self.subscribers.lock().unwrap();
        subscribers.retain(|subscriber| {
            for payload in &payloads {
                if !subscriber.push(payload.clone()) {
                    return false;
                }
            }
            true
        });
        payloads.len()
    }

    /// Drive [`flush`](Self::flush) forever: wake on the first note, send, then
    /// hold the window open before the next send. Notes taken while the window
    /// is open leave the wake latched, so the flush after it goes out the
    /// instant the window closes.
    ///
    /// Spawned once per daemon, on a task that holds no app lock.
    pub async fn run(bus: Arc<Self>) {
        loop {
            bus.wake.notified().await;
            bus.flush();
            tokio::time::sleep(bus.window).await;
        }
    }

    /// Start [`run`](Self::run) if there is a runtime to start it on. A bridge
    /// built outside one (the synchronous unit tests) simply never flushes; see
    /// [`PENDING_KEY_CAP`].
    pub fn spawn_flusher(bus: Arc<Self>) {
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(ChangeBus::run(bus));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Move a paused clock forward by `step` and let the flusher act on it.
    ///
    /// `advance` wakes the timers it passes; the yield is what gives the task
    /// they woke a turn to run before the assertion looks. Without it the test
    /// would race the scheduler instead of the clock — the same race, moved.
    async fn settle(step: Duration) {
        tokio::time::advance(step).await;
        tokio::task::yield_now().await;
    }

    /// Drain everything a subscriber was pushed, decrypted.
    fn drained(
        rx: &mut tokio::sync::mpsc::UnboundedReceiver<crate::carrier::OutboundEnvelope>,
        key: &str,
    ) -> Vec<Value> {
        let mut seen = Vec::new();
        while let Ok(message) = rx.try_recv() {
            seen.push(SessionSender::decrypt_push(key, &message));
        }
        seen
    }

    #[test]
    fn a_noted_change_reaches_every_subscriber_on_flush() {
        let bus = ChangeBus::new(DEFAULT_COALESCE_WINDOW);
        let (one, mut one_rx, one_key) = SessionSender::observable("s-one");
        let (two, mut two_rx, two_key) = SessionSender::observable("s-two");
        bus.subscribe(&one);
        bus.subscribe(&two);

        bus.note_board();
        assert_eq!(bus.flush(), 1);

        assert_eq!(
            drained(&mut one_rx, &one_key),
            vec![json!({ "type": "board.changed" })]
        );
        assert_eq!(
            drained(&mut two_rx, &two_key),
            vec![json!({ "type": "board.changed" })]
        );
    }

    #[test]
    fn an_entity_change_names_the_entity_and_stales_the_feed() {
        let bus = ChangeBus::new(DEFAULT_COALESCE_WINDOW);
        let (sender, mut rx, key) = SessionSender::observable("s-1");
        bus.subscribe(&sender);

        bus.note_entity("run-7");
        bus.flush();

        assert_eq!(
            drained(&mut rx, &key),
            vec![
                json!({ "type": "board.changed" }),
                json!({ "type": "entity.changed", "id": "run-7" }),
            ]
        );
    }

    #[test]
    fn a_burst_of_notes_collapses_to_one_event_per_key() {
        let bus = ChangeBus::new(DEFAULT_COALESCE_WINDOW);
        let (sender, mut rx, key) = SessionSender::observable("s-1");
        bus.subscribe(&sender);

        for _ in 0..200 {
            bus.note_board();
            bus.note_entity("run-7");
            bus.note_entity("run-8");
        }
        assert_eq!(bus.flush(), 3, "one event per distinct key");

        assert_eq!(
            drained(&mut rx, &key),
            vec![
                json!({ "type": "board.changed" }),
                json!({ "type": "entity.changed", "id": "run-7" }),
                json!({ "type": "entity.changed", "id": "run-8" }),
            ]
        );
    }

    #[test]
    fn a_flush_with_nothing_noted_sends_nothing() {
        let bus = ChangeBus::new(DEFAULT_COALESCE_WINDOW);
        let (sender, mut rx, key) = SessionSender::observable("s-1");
        bus.subscribe(&sender);

        assert_eq!(bus.flush(), 0);
        assert!(drained(&mut rx, &key).is_empty());
    }

    #[test]
    fn an_unsubscribed_session_hears_nothing_more() {
        let bus = ChangeBus::new(DEFAULT_COALESCE_WINDOW);
        let (sender, mut rx, key) = SessionSender::observable("s-1");
        bus.subscribe(&sender);
        bus.unsubscribe("s-1");

        bus.note_board();
        bus.flush();

        assert!(drained(&mut rx, &key).is_empty());
        assert_eq!(bus.subscriber_count(), 0);
    }

    #[test]
    fn resubscribing_a_session_id_keeps_one_subscription() {
        let bus = ChangeBus::new(DEFAULT_COALESCE_WINDOW);
        let (first, _first_rx, _first_key) = SessionSender::observable("s-1");
        let (second, mut rx, key) = SessionSender::observable("s-1");
        bus.subscribe(&first);
        bus.subscribe(&second);

        bus.note_board();
        bus.flush();

        assert_eq!(bus.subscriber_count(), 1);
        assert_eq!(
            drained(&mut rx, &key),
            vec![json!({ "type": "board.changed" })]
        );
    }

    #[test]
    fn a_dead_subscriber_is_dropped() {
        let bus = ChangeBus::new(DEFAULT_COALESCE_WINDOW);
        let (sender, rx, _key) = SessionSender::observable("s-gone");
        bus.subscribe(&sender);
        drop(rx); // the connection went away

        bus.note_board();
        bus.flush();

        assert_eq!(bus.subscriber_count(), 0);
    }

    #[test]
    fn an_unflushed_window_past_the_cap_collapses_to_the_board() {
        let bus = ChangeBus::new(DEFAULT_COALESCE_WINDOW);
        let (sender, mut rx, key) = SessionSender::observable("s-1");
        bus.subscribe(&sender);

        for n in 0..(PENDING_KEY_CAP + 50) {
            bus.note(ChangeKey::Entity(format!("run-{n}")));
        }
        assert_eq!(bus.flush(), 1);

        assert_eq!(
            drained(&mut rx, &key),
            vec![json!({ "type": "board.changed" })]
        );
    }

    /// The driver's contract: the first change on an idle bus goes out at once,
    /// and everything noted behind it inside the window is one more flush, not
    /// one per mutation.
    ///
    /// On a paused clock, not a real one: the assertions are about which side
    /// of the window a flush falls on, and read against the wall clock they
    /// were a race — a 30 ms sleep that overran the 150 ms window under a
    /// loaded machine turned "the storm waits" into a failure about nothing.
    /// `advance` moves the clock by exactly what the contract talks about.
    #[tokio::test(start_paused = true)]
    async fn the_flusher_sends_at_most_one_batch_per_window() {
        let bus = ChangeBus::new(Duration::from_millis(150));
        let (sender, mut rx, key) = SessionSender::observable("s-1");
        bus.subscribe(&sender);
        ChangeBus::spawn_flusher(Arc::clone(&bus));

        bus.note_board();
        settle(Duration::from_millis(30)).await;
        assert_eq!(
            drained(&mut rx, &key),
            vec![json!({ "type": "board.changed" })],
            "an idle bus sends the first change straight away"
        );

        for _ in 0..500 {
            bus.note_board();
            bus.note_entity("run-7");
        }
        settle(Duration::from_millis(30)).await;
        assert!(
            drained(&mut rx, &key).is_empty(),
            "the window is still open — the storm waits"
        );

        settle(Duration::from_millis(200)).await;
        assert_eq!(
            drained(&mut rx, &key),
            vec![
                json!({ "type": "board.changed" }),
                json!({ "type": "entity.changed", "id": "run-7" }),
            ],
            "a thousand notes are two events"
        );
    }
}

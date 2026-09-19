//! Presence — bridge → api, one signed beat every 30 s (`planning/v2/Strict P2P
//! Transport Spec.md` rule 6, "Presence is the api's").
//!
//! The relay used to report a device online because it held its socket. That
//! made liveness a property of the hosted relay; it is a property of the device.
//! So the daemon says it itself: a POST to `/api/devices/heartbeat` carrying
//! nothing but a device id, a timestamp and an Ed25519 signature over
//! [`heartbeat_challenge`], which `buildapp/presence.py` rebuilds byte for byte.
//! The api stores only `last_seen_at` and derives `online` from a 90 s window,
//! so a bridge that is killed, unplugged or partitioned needs no goodbye — it
//! simply stops beating.
//!
//! What the beat asserts is that this device can be *reached*, not that its
//! process is running: it goes out only while the daemon holds a relay socket
//! the relay has authenticated ([`crate::reachability::Reachability`]). A bridge
//! that is alive and can still reach the api, but whose relay socket has been
//! severed, cannot be dialled by any browser — so it says nothing, and the api
//! lets it go a window later. Saying "online" for that bridge is what had a
//! browser dialling a machine the relay held no socket for, over and over.
//!
//! Best effort and unkillable, in that order: a refused, unreachable or hung
//! api is logged and the loop beats on. Nothing above this waits on it.
//!
//! And nothing above it notices if it stops, which is why the loop is
//! supervised rather than merely spawned. On 2026-09-19 one of Zech's machines
//! posted its last beat at 19:37:18Z and none after, through six relay
//! reconnects over the next three hours: the daemon was plainly alive and its
//! beat was not, and the handle `main.rs` held was never looked at again. A
//! beat that ends, panics, or simply goes quiet is now replaced.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;

use crate::reachability::Reachability;
use crate::relay::DeviceIdentity;
use crate::transport;

/// How often a bridge says it is alive. Three beats fit in the api's 90 s
/// window, so one lost post is not a device going away.
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);

/// How many intervals the supervisor lets pass with no attempt at all before it
/// replaces the beat. The same three the api's window is made of: a beat that
/// has to be replaced is replaced before the device could go offline for want
/// of it.
const MISSED_BEATS_BEFORE_REPLACING: u32 = 3;

/// The path the beat is posted to.
const HEARTBEAT_PATH: &str = "/api/devices/heartbeat";

/// The payload POSTed to `/api/devices/heartbeat`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeartbeatRequest {
    pub device_id: String,
    /// Unix seconds; the api rejects beats outside its freshness window.
    pub timestamp: i64,
    /// Ed25519 signature (padded b64) over [`heartbeat_challenge`].
    pub signature_b64: String,
}

/// The canonical message signed for a beat. Binds the device and the moment, so
/// a captured signature is neither another device's nor replayable once the
/// api's window has passed. Mirrors `buildapp.presence.heartbeat_challenge`;
/// `bridge/tests/fixtures/presence_challenge.txt` is the fixture both test
/// suites read.
pub fn heartbeat_challenge(device_id: &str, timestamp: i64) -> String {
    format!("heartbeat.{device_id}.{timestamp}")
}

/// Build a fully signed [`HeartbeatRequest`] at `timestamp` (unix seconds).
pub fn build_heartbeat(
    identity: &DeviceIdentity,
    timestamp: i64,
) -> Result<HeartbeatRequest, String> {
    let challenge = heartbeat_challenge(&identity.device_id, timestamp);
    let signature_b64 =
        transport::sign_message_b64(&identity.identity_private_key_b64, challenge.as_bytes())
            .map_err(|e| e.to_string())?;
    Ok(HeartbeatRequest {
        device_id: identity.device_id.clone(),
        timestamp,
        signature_b64,
    })
}

/// The task that beats, and the supervisor that keeps it beating. Owns nothing
/// the rest of the daemon needs, so it is started and forgotten; dropping the
/// handle leaves it running, aborting it stops the beats (and the api sees the
/// device go offline a window later).
pub struct PresenceReporter;

impl PresenceReporter {
    /// Beat to `api_url` as `identity` while `reachable` says a browser could
    /// get here, once now and every [`HEARTBEAT_INTERVAL`] after.
    pub fn start(
        api_url: &str,
        identity: &DeviceIdentity,
        reachable: &Reachability,
    ) -> JoinHandle<()> {
        Self::start_every(api_url, identity, reachable, HEARTBEAT_INTERVAL)
    }

    /// [`start`](Self::start) with the interval spelled out — the tests beat in
    /// milliseconds.
    pub fn start_every(
        api_url: &str,
        identity: &DeviceIdentity,
        reachable: &Reachability,
        interval: Duration,
    ) -> JoinHandle<()> {
        let beat = Beat {
            url: format!("{}{HEARTBEAT_PATH}", api_url.trim_end_matches('/')),
            identity: identity.clone(),
            client: beating_client(interval),
            reachable: reachable.clone(),
            attempted: Arc::new(Mutex::new(Instant::now())),
        };
        tokio::spawn(supervise(beat, interval))
    }
}

/// Keep one [`Beat`] running for the life of the daemon.
///
/// The beat loop is written not to end, so every way it can end is a fault: a
/// panic inside it, or a wedge that leaves the task alive and silent. Both read
/// the same from out here — no beats — and both get the same answer, a new
/// beat. Presence stopping is allowed to be news about the device; it is not
/// allowed to be a permanent state of this daemon.
async fn supervise(beat: Beat, interval: Duration) {
    let quiet_for = interval * MISSED_BEATS_BEFORE_REPLACING;
    let watched = beat.clone();
    keep_running(
        "heartbeat",
        interval,
        move || {
            let beat = beat.clone();
            // A new generation is not born quiet: the watchdog's clock starts
            // with it, not with whatever the last one left behind.
            beat.attempt_noted();
            beat.run(interval)
        },
        move || watched.silent_for(quiet_for),
    )
    .await
}

/// Run `start()`, and run it again every time what it started stops being a
/// task that is doing its job — it finished, or `quiet` says it has gone silent
/// while still alive. Checked once per `interval`, and never returns.
///
/// Spawned rather than awaited in place, because a panic is one of the ways a
/// task stops: awaiting it here would take this loop down with it, which is the
/// failure being guarded against.
async fn keep_running<Start, Work>(
    what: &str,
    interval: Duration,
    start: Start,
    quiet: impl Fn() -> bool,
) where
    Start: Fn() -> Work,
    Work: std::future::Future<Output = ()> + Send + 'static,
{
    loop {
        let running = tokio::spawn(start());
        let fault = loop {
            tokio::time::sleep(interval).await;
            if running.is_finished() {
                break "ended";
            }
            if quiet() {
                break "went quiet";
            }
        };
        running.abort();
        eprintln!("presence: the {what} {fault}; starting another");
    }
}

/// The client one beat is posted with. Its request timeout is the beat
/// interval, because a beat still in flight when the next one is due has
/// already lost its race: an api that accepts the connection and then answers
/// nothing — a black-holed connection after a firewall drops its state — would
/// otherwise park the loop inside one `send()` for good, and presence would go
/// stale on a bridge that is perfectly healthy. Dropping the beat costs one
/// interval; two of the three that fit in the api's window are still left.
fn beating_client(interval: Duration) -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(interval)
        .build()
        // Only fails on a TLS backend that could not be initialised, which
        // would fail the next `Client::new()` too; a bridge that cannot build a
        // client cannot beat at all.
        .unwrap_or_else(|_| reqwest::Client::new())
}

#[derive(Clone)]
struct Beat {
    url: String,
    identity: DeviceIdentity,
    client: reqwest::Client,
    /// The relay socket this device is findable on. Not a condition on the
    /// loop: an unreachable device keeps ticking and starts beating again the
    /// moment its socket is back.
    reachable: Reachability,
    /// When this beat last came round, beaten or skipped. The supervisor reads
    /// it: a loop that has stopped ticking is as dead as one that has returned,
    /// and from outside the two are the same silence.
    attempted: Arc<Mutex<Instant>>,
}

impl Beat {
    /// This beat came round. Recorded before the reachability check, because
    /// what the supervisor watches for is the loop stopping, not the device
    /// being away.
    fn attempt_noted(&self) {
        *self.attempted.lock().unwrap() = Instant::now();
    }

    /// Whether this beat has not come round for `quiet_for`.
    fn silent_for(&self, quiet_for: Duration) -> bool {
        self.attempted.lock().unwrap().elapsed() > quiet_for
    }

    async fn run(self, interval: Duration) {
        let mut ticker = tokio::time::interval(interval);
        // A beat that took longer than the interval (a slow api, a suspended
        // laptop) must not become a burst of catch-up beats: the api would
        // refuse them as replays and learn nothing new.
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            // The first tick is immediate, so the api knows within a second of
            // the relay authenticating this device that it is here.
            ticker.tick().await;
            self.attempt_noted();
            // Silence is the whole report for a device nothing can reach: the
            // api needs no "offline" post, and one it could not act on from a
            // bridge with no way in is exactly the lie this loop used to tell.
            if !self.reachable.is_reachable() {
                continue;
            }
            if let Err(error) = self.send().await {
                eprintln!("presence: heartbeat dropped: {error}");
            }
        }
    }

    async fn send(&self) -> Result<(), String> {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_secs() as i64;
        let beat = build_heartbeat(&self.identity, timestamp)?;
        let response = self
            .client
            .post(&self.url)
            .json(&beat)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(format!("api refused the heartbeat: {}", response.status()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> (DeviceIdentity, String) {
        let keypair = transport::generate_identity_keypair();
        (
            DeviceIdentity {
                device_id: "dev-1".to_string(),
                identity_private_key_b64: keypair.private_key_b64,
            },
            keypair.public_key_b64,
        )
    }

    /// The exact string `buildapp.presence.heartbeat_challenge` produces, and
    /// both fields bound into it.
    #[test]
    fn the_challenge_matches_the_api_contract_and_binds_both_fields() {
        let base = heartbeat_challenge("dev-1", 1_750_000_000);
        assert_eq!(base, "heartbeat.dev-1.1750000000");
        assert_ne!(base, heartbeat_challenge("dev-2", 1_750_000_000));
        assert_ne!(base, heartbeat_challenge("dev-1", 1_750_000_001));
    }

    #[test]
    fn a_beat_carries_the_device_and_a_signature_the_api_can_verify() {
        let (identity, public_key_b64) = identity();
        let beat = build_heartbeat(&identity, 1_750_000_000).expect("signs");
        assert_eq!(
            (beat.device_id.as_str(), beat.timestamp),
            ("dev-1", 1_750_000_000)
        );
        assert!(transport::verify_message_b64(
            &public_key_b64,
            heartbeat_challenge("dev-1", 1_750_000_000).as_bytes(),
            &beat.signature_b64,
        )
        .is_ok());
    }

    #[test]
    fn a_beat_signed_for_another_moment_does_not_verify() {
        let (identity, public_key_b64) = identity();
        let beat = build_heartbeat(&identity, 1_750_000_000).expect("signs");
        assert!(transport::verify_message_b64(
            &public_key_b64,
            heartbeat_challenge("dev-1", 1_750_000_001).as_bytes(),
            &beat.signature_b64,
        )
        .is_err());
    }

    #[test]
    fn three_beats_fit_in_the_api_window() {
        // The api's ONLINE_WINDOW is 90 s; keep them in step.
        assert_eq!(HEARTBEAT_INTERVAL * 3, Duration::from_secs(90));
    }
}

#[cfg(test)]
mod supervision_tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use super::keep_running;

    const INTERVAL: Duration = Duration::from_millis(50);

    /// Start the supervisor, let it watch for `rounds` intervals, and report how
    /// many times it started the work.
    async fn starts_over<Work>(
        work: impl Fn() -> Work + Send + 'static,
        quiet: impl Fn() -> bool + Send + 'static,
        rounds: u32,
    ) -> usize
    where
        Work: std::future::Future<Output = ()> + Send + 'static,
    {
        let starts = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&starts);
        let supervising = tokio::spawn(async move {
            keep_running(
                "beat",
                INTERVAL,
                move || {
                    counted.fetch_add(1, Ordering::SeqCst);
                    work()
                },
                quiet,
            )
            .await
        });
        tokio::time::sleep(INTERVAL * rounds + INTERVAL / 2).await;
        supervising.abort();
        starts.load(Ordering::SeqCst)
    }

    /// The fault the daemon actually suffered: a beat task that is simply no
    /// longer there. `main.rs` held its handle and never looked at it, so on
    /// 2026-09-19 one machine went three hours without a heartbeat while its
    /// relay loop reconnected six times.
    #[tokio::test(start_paused = true)]
    async fn a_beat_that_ends_is_started_again() {
        let starts = starts_over(|| std::future::ready(()), || false, 4).await;
        assert!(
            starts >= 4,
            "one start per interval for a beat that keeps ending, got {starts}"
        );
    }

    /// The other half of the same silence: the task is alive and posting
    /// nothing. Nobody outside can tell that from a task that died, so it is not
    /// told apart — it is replaced.
    #[tokio::test(start_paused = true)]
    async fn a_beat_that_goes_quiet_while_alive_is_replaced() {
        let starts = starts_over(std::future::pending::<()>, || true, 4).await;
        assert!(
            starts >= 4,
            "a live but silent beat is replaced every interval, got {starts}"
        );
    }

    /// And a beat that is doing its job is left alone: the supervisor must not
    /// become a restart loop that never lets one beat finish a post.
    #[tokio::test(start_paused = true)]
    async fn a_beat_that_is_beating_is_left_alone() {
        let starts = starts_over(std::future::pending::<()>, || false, 6).await;
        assert_eq!(starts, 1, "a healthy beat is started once and left running");
    }
}

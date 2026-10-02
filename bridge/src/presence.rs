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
//! supervised rather than merely spawned. On 2026-09-19 one of the maintainer's
//! machines posted its last beat at 19:37:18Z and none after, through six relay
//! reconnects over the next three hours: the daemon was plainly alive and its
//! beat was not, and the handle `main.rs` held was never looked at again. A
//! beat that ends, panics, or simply goes quiet is now replaced.

use std::future::Future;
use std::pin::Pin;
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
/// of it. Counted from when the beat said it would next come round, less the
/// one interval that is: a beat waiting out an api's `Retry-After` is not
/// silent, it is on time.
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
            identity: identity.clone(),
            post: post_to_api(
                format!("{}{HEARTBEAT_PATH}", api_url.trim_end_matches('/')),
                beating_client(interval),
            ),
            reachable: reachable.clone(),
            due: Arc::new(Mutex::new(Instant::now() + interval)),
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
    let slack = interval * (MISSED_BEATS_BEFORE_REPLACING - 1);
    let watched = beat.clone();
    keep_running(
        "heartbeat",
        interval,
        move || {
            let beat = beat.clone();
            // A new generation is not born quiet: the watchdog's clock starts
            // with it, not with whatever the last one left behind.
            beat.came_round(interval);
            beat.run(interval)
        },
        move || watched.overdue_by(slack),
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
        crate::logline::say(format!("presence: the {what} {fault}; starting another"));
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

/// Where a signed beat goes: the api, in the daemon; a recorder, in the tests
/// that hold the clock still.
type Post = Arc<dyn Fn(HeartbeatRequest) -> Posting + Send + Sync>;
type Posting = Pin<Box<dyn Future<Output = Result<(), Dropped>> + Send>>;

#[derive(Clone)]
struct Beat {
    identity: DeviceIdentity,
    post: Post,
    /// The relay socket this device is findable on. Not a condition on the
    /// loop: an unreachable device keeps ticking, and beats the moment its
    /// socket is authenticated rather than at its next tick.
    reachable: Reachability,
    /// When this beat said it would next come round, beaten or skipped: an
    /// interval after it last did, or the end of the wait it chose after a
    /// failure. The supervisor reads it: a loop that has stopped ticking is as
    /// dead as one that has returned, and from outside the two are the same
    /// silence — but a loop waiting out a `Retry-After` it was given is
    /// neither, and is left to wait (#131 review).
    due: Arc<Mutex<Instant>>,
}

impl Beat {
    /// This beat came round, and will again within `next_in`. Recorded before
    /// the reachability check, because what the supervisor watches for is the
    /// loop stopping, not the device being away.
    fn came_round(&self, next_in: Duration) {
        *self.due.lock().unwrap() = Instant::now() + next_in;
    }

    /// Whether this beat is more than `slack` past when it said it would next
    /// come round.
    fn overdue_by(&self, slack: Duration) -> bool {
        Instant::now() > *self.due.lock().unwrap() + slack
    }

    async fn run(self, interval: Duration) {
        let mut ticker = tokio::time::interval(interval);
        // A beat that took longer than the interval (a slow api, a suspended
        // laptop) must not become a burst of catch-up beats: the api would
        // refuse them as replays and learn nothing new.
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let mut retries = Retries::every(interval);
        let mut reaches = self.reachable.watch();
        let mut last_sent: Option<tokio::time::Instant> = None;
        // When the retry a failure chose comes round. Until then a socket
        // coming back waits with it; after it, the retry has been and gone
        // (skipped, most often, because the same blip took the socket down)
        // and the socket coming back is beaten at once (#321 review).
        let mut retry_due: Option<tokio::time::Instant> = None;
        loop {
            tokio::select! {
                // The first tick is immediate: a device the relay has already
                // authenticated says so at once.
                _ = ticker.tick() => {}
                // And one it authenticates later says so then, not at the next
                // tick. On a cold start the beat is started before the relay is
                // dialled, so its first tick always found the device
                // unreachable, and the api heard nothing for a whole interval
                // after the socket was up: a just-approved device sat on "no
                // devices online" for 30 s (#321).
                changed = reaches.changed() => {
                    // This beat holds the sender, so the channel cannot close.
                    let reachable = changed.is_ok() && *reaches.borrow_and_update();
                    // A beat waiting out a refusal keeps waiting: the api asked
                    // for that, and a socket coming back does not answer it. A
                    // beat sent less than a second ago is already this
                    // second's, and the api would refuse its twin as a replay.
                    let sent_this_second = last_sent
                        .is_some_and(|sent| sent.elapsed() < Duration::from_secs(1));
                    let retry_pending =
                        retry_due.is_some_and(|due| tokio::time::Instant::now() < due);
                    if !reachable || retry_pending || sent_this_second {
                        continue;
                    }
                    ticker.reset();
                }
            }
            self.came_round(interval);
            // Silence is the whole report for a device nothing can reach: the
            // api needs no "offline" post, and one it could not act on from a
            // bridge with no way in is exactly the lie this loop used to tell.
            if !self.reachable.is_reachable() {
                continue;
            }
            last_sent = Some(tokio::time::Instant::now());
            match self.send().await {
                Ok(()) => {
                    retries.landed();
                    retry_due = None;
                }
                Err(dropped) => {
                    // A refusal that passes — the api between two pods, a
                    // busy one, none at all — is asked again soon rather than
                    // a whole interval later, and less soon each time it is
                    // refused again (#131).
                    let again = dropped
                        .passes
                        .then(|| retries.after_failure(dropped.retry_after, jitter()));
                    if !dropped.passes {
                        retries.landed();
                    }
                    retry_due = again.map(|again| tokio::time::Instant::now() + again);
                    if let Some(again) = again {
                        ticker.reset_after(again);
                        self.came_round(again);
                    }
                    crate::logline::say(format!(
                        "presence: heartbeat dropped: {}{}",
                        dropped.reason,
                        again.map_or_else(String::new, |again| format!(
                            "; again in {:.1}s",
                            again.as_secs_f64()
                        ))
                    ));
                }
            }
        }
    }

    async fn send(&self) -> Result<(), Dropped> {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| Dropped::for_good(e.to_string()))?
            .as_secs() as i64;
        let beat = build_heartbeat(&self.identity, timestamp).map_err(Dropped::for_good)?;
        (self.post)(beat).await
    }
}

/// Post each beat to `url` with `client`.
fn post_to_api(url: String, client: reqwest::Client) -> Post {
    Arc::new(move |beat| {
        let (url, client) = (url.clone(), client.clone());
        Box::pin(async move { post_beat(&client, &url, &beat).await })
    })
}

async fn post_beat(
    client: &reqwest::Client,
    url: &str,
    beat: &HeartbeatRequest,
) -> Result<(), Dropped> {
    let response = client
        .post(url)
        .json(beat)
        .send()
        .await
        .map_err(|e| Dropped::in_passing(e.to_string()))?;
    let status = response.status();
    if status.is_success() {
        return Ok(());
    }
    let reason = format!("api refused the heartbeat: {status}");
    // The ingress's 404 while the api's one pod is replaced (a deploy, every
    // time), a rate limit, a server error: nothing about this device.
    let passes = status == reqwest::StatusCode::NOT_FOUND
        || status == reqwest::StatusCode::TOO_MANY_REQUESTS
        || status.is_server_error();
    let retry_after = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| retry_after(value, SystemTime::now()));
    Err(Dropped {
        reason,
        passes,
        retry_after,
    })
}

/// The first retry of a beat that failed in passing comes a sixth of an
/// interval after it — five seconds at the daemon's thirty.
const FIRST_RETRY_PER_INTERVAL: u32 = 6;

/// The longest a `Retry-After` is waited out. Longer than this and the device
/// would go offline for the api's own say-so; past it the beat asks anyway.
const LONGEST_RETRY_AFTER: Duration = Duration::from_secs(600);

/// When a beat that failed in passing is tried again.
///
/// Soon at first, because the common failure is the api's half-minute between
/// two pods, and a device should stay online through it. Then less soon each
/// time it fails again — doubling from a sixth of an interval to the interval
/// itself, so a failure that goes on costs the api what a healthy device does,
/// one beat an interval — and never in step with every other device refused
/// at the same moment: each wait is jittered by a quarter either way. An api
/// that names its own `Retry-After` is not asked before it, up to
/// [`LONGEST_RETRY_AFTER`]. A beat that lands starts the schedule over.
#[derive(Debug, Clone)]
struct Retries {
    interval: Duration,
    /// Failures in passing since the last beat that landed.
    failed: u32,
}

impl Retries {
    fn every(interval: Duration) -> Retries {
        Retries {
            interval,
            failed: 0,
        }
    }

    /// A beat landed, or was refused on its merits: the next failure in
    /// passing starts from the soonest retry again.
    fn landed(&mut self) {
        self.failed = 0;
    }

    /// How long to wait after one more failure in passing. `jitter` is
    /// uniform in `[0, 1)`.
    fn after_failure(&mut self, retry_after: Option<Duration>, jitter: f64) -> Duration {
        let first = self.interval / FIRST_RETRY_PER_INTERVAL;
        let doubled = first.saturating_mul(1 << self.failed.min(16));
        self.failed = self.failed.saturating_add(1);
        let jittered = doubled.min(self.interval).mul_f64(0.75 + jitter / 2.0);
        match retry_after {
            Some(asked) => jittered.max(asked.min(LONGEST_RETRY_AFTER)),
            None => jittered,
        }
    }
}

/// A number uniform in `[0, 1)`, from the operating system's random source
/// (a version 4 UUID is 122 bits of it). Only bytes 9 to 15 are taken, which
/// the version's nibble (byte 6) and the variant's two bits (byte 8) leave
/// alone, and 53 of their 56 bits — as many as an `f64` holds exactly.
pub(crate) fn jitter() -> f64 {
    let uuid = uuid::Uuid::new_v4().into_bytes();
    let mut bits = [0u8; 8];
    bits[..7].copy_from_slice(&uuid[9..16]);
    (u64::from_le_bytes(bits) >> 3) as f64 / (1u64 << 53) as f64
}

/// What a `Retry-After` header asks for, as a wait from `now`: a number of
/// seconds, or an HTTP date. A date already past asks for no wait.
pub(crate) fn retry_after(value: &str, now: SystemTime) -> Option<Duration> {
    let value = value.trim();
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    let at = httpdate::parse_http_date(value).ok()?;
    Some(at.duration_since(now).unwrap_or(Duration::ZERO))
}

/// A beat that did not land, and whether trying again soon could land it.
struct Dropped {
    reason: String,
    passes: bool,
    /// How long the api asked to be left alone, when it said.
    retry_after: Option<Duration>,
}

impl Dropped {
    fn in_passing(reason: String) -> Dropped {
        Dropped {
            reason,
            passes: true,
            retry_after: None,
        }
    }

    fn for_good(reason: String) -> Dropped {
        Dropped {
            reason,
            passes: false,
            retry_after: None,
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

    /// At the daemon's 30 s: 5 s, 10 s, 20 s, then the interval for as long
    /// as the failure lasts, each a quarter either way; and from the soonest
    /// again once a beat lands.
    #[test]
    fn a_failure_that_goes_on_is_retried_less_soon_up_to_the_interval() {
        let mut retries = Retries::every(HEARTBEAT_INTERVAL);
        let unjittered: Vec<u64> = (0..6)
            .map(|_| retries.after_failure(None, 0.5).as_secs())
            .collect();
        assert_eq!(unjittered, [5, 10, 20, 30, 30, 30]);

        retries.landed();
        assert_eq!(
            retries.after_failure(None, 0.0),
            Duration::from_millis(3750)
        );
        retries.landed();
        assert_eq!(
            retries.after_failure(None, 0.999_999),
            Duration::from_secs(5).mul_f64(0.75 + 0.999_999 / 2.0)
        );
    }

    /// Ten devices behind one address, failing together for ten minutes and
    /// each jittered to its soonest, post at most three beats a minute each:
    /// a quarter of the 120 a minute the api allows that address, and about
    /// what they would if every beat landed.
    #[test]
    fn a_sustained_failure_costs_the_api_about_one_beat_an_interval() {
        let mut retries = Retries::every(HEARTBEAT_INTERVAL);
        let (mut elapsed, mut beats) = (Duration::ZERO, 1);
        while elapsed < Duration::from_secs(600) {
            elapsed += retries.after_failure(None, 0.0);
            beats += 1;
        }
        let per_minute = beats as f64 / 10.0;
        assert!(per_minute <= 3.0, "{per_minute} beats a minute per device");
        assert!(10.0 * per_minute < 120.0);
    }

    /// A `Retry-After` is waited out, in seconds or as a date, up to ten
    /// minutes; the backoff still stands when it asks for less.
    #[test]
    fn a_retry_after_is_honoured_up_to_a_bound() {
        let now = UNIX_EPOCH + Duration::from_secs(1_750_000_000);
        assert_eq!(retry_after(" 42 ", now), Some(Duration::from_secs(42)));
        assert_eq!(
            retry_after(&httpdate::fmt_http_date(now + Duration::from_secs(90)), now),
            Some(Duration::from_secs(90))
        );
        assert_eq!(
            retry_after(&httpdate::fmt_http_date(now - Duration::from_secs(5)), now),
            Some(Duration::ZERO)
        );
        assert_eq!(retry_after("soon", now), None);

        let mut retries = Retries::every(HEARTBEAT_INTERVAL);
        assert_eq!(
            retries.after_failure(Some(Duration::from_secs(45)), 0.5),
            Duration::from_secs(45)
        );
        assert_eq!(
            retries.after_failure(Some(Duration::from_secs(1)), 0.5),
            Duration::from_secs(10),
            "a short Retry-After does not undo the backoff"
        );
        assert_eq!(
            retries.after_failure(Some(Duration::from_secs(86_400)), 0.5),
            LONGEST_RETRY_AFTER
        );
    }

    /// The jitter the schedule is fed spreads over all of `[0, 1)`, so the
    /// waits it makes spread over a quarter either way of the schedule's.
    /// Taken from a UUID, it carried the version's and variant's fixed bits
    /// and never left `[0.5, 0.75)` (#131 review).
    #[test]
    fn the_jitter_spreads_evenly_over_zero_to_one() {
        let draws: Vec<f64> = (0..20_000).map(|_| jitter()).collect();
        assert!(draws.iter().all(|draw| (0.0..1.0).contains(draw)));
        for quarter in 0..4 {
            let low = quarter as f64 / 4.0;
            let share = draws
                .iter()
                .filter(|draw| (low..low + 0.25).contains(*draw))
                .count() as f64
                / draws.len() as f64;
            assert!(
                (0.22..0.28).contains(&share),
                "{share} of the draws in [{low}, {})",
                low + 0.25
            );
        }
        let waits: Vec<Duration> = draws
            .iter()
            .map(|draw| Retries::every(HEARTBEAT_INTERVAL).after_failure(None, *draw))
            .collect();
        let (shortest, longest) = (waits.iter().min().unwrap(), waits.iter().max().unwrap());
        assert!(*shortest < Duration::from_millis(3_900), "{shortest:?}");
        assert!(*longest > Duration::from_millis(6_100), "{longest:?}");
    }

    /// The supervisor holds a beat to the deadline it set itself: overdue
    /// once it is past it by the slack, whether that deadline was the next
    /// interval or the end of a long `Retry-After`; and a loop that has
    /// stopped coming round is still caught.
    #[test]
    fn a_beat_is_overdue_past_its_own_deadline() {
        let beat = recording(&Reachability::unreachable(), |_| Ok(())).0;
        *beat.due.lock().unwrap() = Instant::now();
        let slack = Duration::from_millis(20);

        beat.came_round(Duration::from_millis(10));
        assert!(!beat.overdue_by(slack), "on time");
        std::thread::sleep(Duration::from_millis(60));
        assert!(beat.overdue_by(slack), "stopped coming round");

        beat.came_round(Duration::from_secs(600));
        std::thread::sleep(Duration::from_millis(60));
        assert!(!beat.overdue_by(slack), "waiting out what it was asked to");
    }

    /// A beat that hands each post to `answer` and records it, posting nothing.
    fn recording(
        reachable: &Reachability,
        answer: fn(&HeartbeatRequest) -> Result<(), Dropped>,
    ) -> (Beat, tokio::sync::mpsc::UnboundedReceiver<HeartbeatRequest>) {
        let (posted, beats) = tokio::sync::mpsc::unbounded_channel();
        let beat = Beat {
            identity: identity().0,
            post: Arc::new(move |beat: HeartbeatRequest| -> Posting {
                let answered = answer(&beat);
                let _ = posted.send(beat);
                Box::pin(std::future::ready(answered))
            }),
            reachable: reachable.clone(),
            due: Arc::new(Mutex::new(Instant::now() + HEARTBEAT_INTERVAL)),
        };
        (beat, beats)
    }

    /// How long, on the paused clock, until the next beat is posted.
    async fn next_beat_after(
        beats: &mut tokio::sync::mpsc::UnboundedReceiver<HeartbeatRequest>,
    ) -> Duration {
        let asked = tokio::time::Instant::now();
        beats.recv().await.expect("the beat runs");
        asked.elapsed()
    }

    /// The cold start: the beat is started before the relay is dialled, so its
    /// first tick finds the device unreachable. The relay authenticating it is
    /// what sends the first beat, at once, not an interval later (#321). And
    /// so does every later return of the socket, after which the beats keep
    /// their interval.
    #[tokio::test(start_paused = true)]
    async fn a_beat_goes_out_the_moment_the_relay_authenticates() {
        let reachable = Reachability::unreachable();
        let (beat, mut beats) = recording(&reachable, |_| Ok(()));
        let beating = tokio::spawn(beat.run(HEARTBEAT_INTERVAL));

        tokio::time::sleep(Duration::from_secs(2)).await;
        assert!(beats.try_recv().is_err(), "nothing reaches it yet");
        reachable.reached();
        assert_eq!(next_beat_after(&mut beats).await, Duration::ZERO);
        assert_eq!(next_beat_after(&mut beats).await, HEARTBEAT_INTERVAL);

        for away in [Duration::from_secs(7), Duration::from_secs(45)] {
            tokio::time::sleep(Duration::from_secs(3)).await;
            reachable.lost();
            tokio::time::sleep(away).await;
            while beats.try_recv().is_ok() {}
            reachable.reached();
            assert_eq!(
                next_beat_after(&mut beats).await,
                Duration::ZERO,
                "back after {away:?}"
            );
            assert_eq!(next_beat_after(&mut beats).await, HEARTBEAT_INTERVAL);
        }
        beating.abort();
    }

    /// A socket that drops and is back within the second of a beat sends no
    /// second one: the api has this second's, and refuses its twin as a replay.
    #[tokio::test(start_paused = true)]
    async fn a_socket_back_within_the_second_of_a_beat_is_not_beaten_twice() {
        let reachable = Reachability::unreachable();
        reachable.reached();
        let (beat, mut beats) = recording(&reachable, |_| Ok(()));
        let beating = tokio::spawn(beat.run(HEARTBEAT_INTERVAL));

        assert_eq!(next_beat_after(&mut beats).await, Duration::ZERO);
        tokio::time::sleep(Duration::from_millis(300)).await;
        reachable.lost();
        reachable.reached();
        let waited = next_beat_after(&mut beats).await;
        assert_eq!(waited + Duration::from_millis(300), HEARTBEAT_INTERVAL);
        beating.abort();
    }

    /// A beat refused in passing waits out its retry even when the socket
    /// comes back meanwhile: the api asked for the wait.
    #[tokio::test(start_paused = true)]
    async fn a_socket_coming_back_does_not_cut_a_retry_short() {
        let reachable = Reachability::unreachable();
        reachable.reached();
        let (beat, mut beats) = recording(&reachable, |_| {
            Err(Dropped {
                reason: "busy".to_string(),
                passes: true,
                retry_after: Some(Duration::from_secs(20)),
            })
        });
        let beating = tokio::spawn(beat.run(HEARTBEAT_INTERVAL));

        assert_eq!(next_beat_after(&mut beats).await, Duration::ZERO);
        tokio::time::sleep(Duration::from_secs(2)).await;
        reachable.lost();
        tokio::time::sleep(Duration::from_secs(2)).await;
        reachable.reached();
        let waited = next_beat_after(&mut beats).await;
        assert_eq!(waited + Duration::from_secs(4), Duration::from_secs(20));
        beating.abort();
    }

    /// The common reconnect: a beat fails in the blip that takes the socket
    /// down, its retry comes round while the device is still unreachable and
    /// is skipped, and then the relay authenticates it again. That return is
    /// beaten at once, not left for the next tick up to an interval away
    /// (#321 review): the wait the failure chose is over.
    #[tokio::test(start_paused = true)]
    async fn a_socket_back_after_a_skipped_retry_beats_at_once() {
        static POSTS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let reachable = Reachability::unreachable();
        reachable.reached();
        let (beat, mut beats) = recording(&reachable, |_| {
            if POSTS.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                Err(Dropped::in_passing("connection reset".to_string()))
            } else {
                Ok(())
            }
        });
        let beating = tokio::spawn(beat.run(HEARTBEAT_INTERVAL));

        assert_eq!(next_beat_after(&mut beats).await, Duration::ZERO);
        reachable.lost();
        // The retry is due within 6.25 s (5 s, jittered a quarter either way).
        tokio::time::sleep(Duration::from_secs(8)).await;
        assert!(beats.try_recv().is_err(), "the retry found it unreachable");
        reachable.reached();
        assert_eq!(next_beat_after(&mut beats).await, Duration::ZERO);
        beating.abort();
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

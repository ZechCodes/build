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
//! Best effort and unkillable, in that order: a refused or unreachable api is
//! logged and the loop beats on. Nothing above this waits on it.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;

use crate::relay::DeviceIdentity;
use crate::transport;

/// How often a bridge says it is alive. Three beats fit in the api's 90 s
/// window, so one lost post is not a device going away.
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);

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

/// The task that beats. Owns nothing the rest of the daemon needs, so it is
/// started and forgotten; dropping the handle leaves it running, aborting it
/// stops the beats (and the api sees the device go offline a window later).
pub struct PresenceReporter;

impl PresenceReporter {
    /// Beat to `api_url` as `identity`, once now and every
    /// [`HEARTBEAT_INTERVAL`] after.
    pub fn start(api_url: &str, identity: &DeviceIdentity) -> JoinHandle<()> {
        Self::start_every(api_url, identity, HEARTBEAT_INTERVAL)
    }

    /// [`start`](Self::start) with the interval spelled out — the tests beat in
    /// milliseconds.
    pub fn start_every(
        api_url: &str,
        identity: &DeviceIdentity,
        interval: Duration,
    ) -> JoinHandle<()> {
        let beat = Beat {
            url: format!("{}{HEARTBEAT_PATH}", api_url.trim_end_matches('/')),
            identity: identity.clone(),
            client: reqwest::Client::new(),
        };
        tokio::spawn(beat.run(interval))
    }
}

struct Beat {
    url: String,
    identity: DeviceIdentity,
    client: reqwest::Client,
}

impl Beat {
    async fn run(self, interval: Duration) {
        let mut ticker = tokio::time::interval(interval);
        // A beat that took longer than the interval (a slow api, a suspended
        // laptop) must not become a burst of catch-up beats: the api would
        // refuse them as replays and learn nothing new.
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            // The first tick is immediate, so the api knows within a second of
            // start that this device is here.
            ticker.tick().await;
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

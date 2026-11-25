//! The paired issuer's challenge/claim exchange. Each attempt signs fresh
//! proofs; assigned credentials are private to the connector supervisor.

use std::time::Duration;

use base64::{engine::general_purpose, Engine};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::http::discovery::{
    bounded_json, unix_now, CanonicalOrigin, PairedDeviceProof, ProofError,
};
use crate::http::settings::OriginMode;
use crate::identity::StoredIdentity;

const CHALLENGE_TYPE: &str = "build-http-hosted-tunnel-challenge+jwt";
const CLAIM_TYPE: &str = "build-http-hosted-tunnel-claim+jwt";
const MAX_GENERATION: u64 = (1 << 53) - 1;

#[derive(Clone, Copy, Serialize)]
pub(super) struct Report {
    pub mode: OriginMode,
    pub hosted_capable: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Challenge {
    challenge_id: String,
    nonce: String,
    expires_at: u64,
    // Retain the distinction between a missing field and JSON null below.
    generation: serde_json::Value,
}

impl Challenge {
    fn validate(&self, now: u64) -> Result<Option<u64>, ProofError> {
        if !canonical_uuid(&self.challenge_id)
            || self.expires_at <= now
            || self.expires_at > now.saturating_add(65)
            || !general_purpose::URL_SAFE_NO_PAD
                .decode(&self.nonce)
                .is_ok_and(|nonce| nonce.len() == 32)
        {
            return Err(ProofError::Refused);
        }
        match self.generation {
            serde_json::Value::Null => Ok(None),
            _ => self
                .generation
                .as_u64()
                .filter(|value| valid_generation(*value))
                .map(Some)
                .ok_or(ProofError::Refused),
        }
    }
}

#[derive(Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Reply {
    Off,
    Personal,
    Setup { reason: SetupReason },
    Assigned(Assignment),
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum SetupReason {
    Provisioning,
    Capacity,
    Collision,
    UpdateRequired,
    AssignmentChanged,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Assignment {
    pub hostname: String,
    pub generation: u64,
    pub credentials: Credentials,
}

#[derive(Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase", deny_unknown_fields)]
pub(super) struct Credentials {
    account_tag: String,
    #[serde(rename = "TunnelID")]
    pub tunnel_id: String,
    tunnel_secret: String,
}

impl Drop for Credentials {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.tunnel_secret.zeroize();
    }
}

impl Credentials {
    fn valid(&self) -> bool {
        !self.account_tag.is_empty()
            && self.account_tag.len() <= 128
            && self
                .account_tag
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            && canonical_uuid(&self.tunnel_id)
            && self.tunnel_secret.len() <= 256
            && general_purpose::STANDARD
                .decode(&self.tunnel_secret)
                .map(zeroize::Zeroizing::new)
                .is_ok_and(|secret| secret.len() >= 32)
    }
}

impl Assignment {
    pub fn origin(&self) -> String {
        format!("https://{}", self.hostname)
    }

    fn valid(&self, generation: Option<u64>) -> bool {
        generation == Some(self.generation)
            && valid_generation(self.generation)
            && valid_hostname(&self.hostname)
            && self.credentials.valid()
    }

    pub fn same_as(&self, other: &Self) -> bool {
        self.generation == other.generation
            && self.hostname == other.hostname
            && self.credentials == other.credentials
    }
}

impl Reply {
    fn validate(self, report: Report, generation: Option<u64>) -> Result<Self, ProofError> {
        match &self {
            Self::Personal if report.mode != OriginMode::Personal => Err(ProofError::Refused),
            Self::Assigned(assignment)
                if report.mode != OriginMode::Hosted
                    || !report.hosted_capable
                    || !assignment.valid(generation) =>
            {
                Err(ProofError::Refused)
            }
            _ => Ok(self),
        }
    }
}

fn valid_hostname(host: &str) -> bool {
    let Some((label, zone)) = host.split_once('.') else {
        return false;
    };
    // The issuer authenticates the exact hostname. The bridge only accepts
    // the two prefixes shipped for hosted assignments.
    let label_valid = label.split_once('-').is_some_and(|(prefix, label)| {
        matches!(prefix, "b" | "bd")
            && label.len() == 32
            && label
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    });
    label_valid
        && !zone.is_empty()
        && host.len() <= 253
        && CanonicalOrigin::parse(&format!("https://{host}")).is_ok_and(|origin| {
            origin.as_str().strip_prefix("https://") == Some(host)
                && host
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-')
        })
}

fn canonical_uuid(input: &str) -> bool {
    Uuid::parse_str(input).is_ok_and(|id| id.hyphenated().to_string() == input)
}

fn valid_generation(generation: u64) -> bool {
    (1..=MAX_GENERATION).contains(&generation)
}

fn claim_url(signer: &PairedDeviceProof) -> String {
    format!(
        "{}/api/devices/{}/hosted-tunnel",
        signer.issuer(),
        signer.device_id()
    )
}

fn sign_challenge(
    signer: &PairedDeviceProof,
    report: Report,
    now: u64,
) -> Result<String, ProofError> {
    signer.sign(
        CHALLENGE_TYPE,
        format!("{}/challenge", claim_url(signer)),
        now,
        json!(report),
    )
}

fn sign_claim(
    signer: &PairedDeviceProof,
    report: Report,
    challenge: &Challenge,
    now: u64,
) -> Result<String, ProofError> {
    signer.sign(
        CLAIM_TYPE,
        claim_url(signer),
        now,
        json!({
            "mode": report.mode,
            "hosted_capable": report.hosted_capable,
            "challenge_id": challenge.challenge_id,
            "nonce": challenge.nonce,
            "generation": challenge.generation,
        }),
    )
}

pub(super) struct ClaimClient {
    client: reqwest::Client,
    signer: PairedDeviceProof,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum ClaimFailure {
    Unavailable,
    Refused,
    LegacyIssuer,
}

impl ClaimClient {
    pub fn new(identity: &StoredIdentity, issuer: &str) -> Result<Self, ProofError> {
        let signer = PairedDeviceProof::new(identity, issuer)?;
        let client = reqwest::Client::builder()
            .https_only(true)
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(5))
            .build()
            .map_err(|_| ProofError::Unavailable)?;
        Ok(Self { client, signer })
    }

    pub async fn claim(&self, report: Report) -> Result<Reply, ClaimFailure> {
        self.exchange(report).await
    }

    async fn exchange(&self, report: Report) -> Result<Reply, ClaimFailure> {
        let proof =
            sign_challenge(&self.signer, report, clock()?).map_err(|_| ClaimFailure::Refused)?;
        let url = claim_url(&self.signer);
        let challenge: Challenge = self
            .post(&format!("{url}/challenge"), proof, StatusCode::CREATED)
            .await?;
        let now = clock()?;
        let generation = challenge.validate(now).map_err(|_| ClaimFailure::Refused)?;
        let proof =
            sign_claim(&self.signer, report, &challenge, now).map_err(|_| ClaimFailure::Refused)?;
        let response: Reply = self.post(&url, proof, StatusCode::OK).await?;
        response
            .validate(report, generation)
            .map_err(|_| ClaimFailure::Refused)
    }

    async fn post<T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
        proof: String,
        expected: StatusCode,
    ) -> Result<T, ClaimFailure> {
        let response = self
            .client
            .post(url)
            .json(&json!({"proof": proof}))
            .send()
            .await
            .map_err(|_| ClaimFailure::Unavailable)?;
        match response.status() {
            status if status == expected => bounded_json(response).await.map_err(|error| {
                if error == ProofError::Unavailable {
                    ClaimFailure::Unavailable
                } else {
                    ClaimFailure::Refused
                }
            }),
            StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED => {
                Err(ClaimFailure::LegacyIssuer)
            }
            StatusCode::TOO_MANY_REQUESTS => Err(ClaimFailure::Unavailable),
            status if status.is_server_error() => Err(ClaimFailure::Unavailable),
            _ => Err(ClaimFailure::Refused),
        }
    }
}

fn clock() -> Result<u64, ClaimFailure> {
    unix_now().map_err(|_| ClaimFailure::Unavailable)
}

#[cfg(test)]
#[path = "claim_tests.rs"]
mod tests;

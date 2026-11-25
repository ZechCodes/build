//! Fixed-purpose device assertions and endpoint registration. Public discovery
//! signs captured configuration, never an origin or audience supplied by a peer.

use std::fmt;
use std::time::Duration;

use aws_lc_rs::signature::Ed25519KeyPair;
use base64::{engine::general_purpose::STANDARD_NO_PAD, Engine};
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use serde::{Deserialize, Serialize};

use crate::identity::StoredIdentity;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ProofError {
    #[error("invalid HTTPS origin")]
    InvalidOrigin,
    #[error("identity is not approved by the configured issuer")]
    UnapprovedIdentity,
    #[error("invalid device proof input")]
    InvalidInput,
    #[error("device proof signing failed")]
    Signing,
    #[error("endpoint registration unavailable")]
    Unavailable,
    #[error("endpoint registration refused")]
    Refused,
    #[error("endpoint registration predecessor changed")]
    Conflict,
}

/// An origin, not an arbitrary URL. Parsing does no DNS or network access.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct CanonicalOrigin(String);

impl CanonicalOrigin {
    pub fn parse(input: &str) -> Result<Self, ProofError> {
        let authority = raw_authority(input)?;
        let url = reqwest::Url::parse(input).map_err(|_| ProofError::InvalidOrigin)?;
        if url.scheme() != "https" || url.port_or_known_default() == Some(0) {
            return Err(ProofError::InvalidOrigin);
        }
        validate_host(&url, authority)?;
        Ok(Self(url.origin().ascii_serialization()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CanonicalOrigin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

fn raw_authority(input: &str) -> Result<&str, ProofError> {
    if input.len() > 2048 || input.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(ProofError::InvalidOrigin);
    }
    let (scheme, rest) = input.split_once("://").ok_or(ProofError::InvalidOrigin)?;
    if !scheme.eq_ignore_ascii_case("https") || rest.contains(['?', '#', '@', '\\', '%']) {
        return Err(ProofError::InvalidOrigin);
    }
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    if authority.is_empty() || !path.is_empty() {
        return Err(ProofError::InvalidOrigin);
    }
    Ok(authority)
}

fn validate_host(url: &reqwest::Url, authority: &str) -> Result<(), ProofError> {
    let host = url.host_str().ok_or(ProofError::InvalidOrigin)?;
    if host.ends_with('.') {
        return Err(ProofError::InvalidOrigin);
    }
    if let Ok(ip) = host.parse::<std::net::Ipv4Addr>() {
        let raw_host = authority.split(':').next().unwrap_or("");
        if raw_host != ip.to_string() {
            return Err(ProofError::InvalidOrigin);
        }
    }
    if !host.starts_with('[') && !valid_dns_host(host) {
        return Err(ProofError::InvalidOrigin);
    }
    // Reject empty explicit ports; URL parsers otherwise erase this alias.
    if authority.ends_with(':') {
        return Err(ProofError::InvalidOrigin);
    }
    Ok(())
}

fn valid_dns_host(host: &str) -> bool {
    host.len() <= 253
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label.as_bytes()[0].is_ascii_alphanumeric()
                && label.as_bytes()[label.len() - 1].is_ascii_alphanumeric()
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
}

/// Keeps keys out of Debug/logging and permits only the three reviewed purposes.
pub(super) struct PairedDeviceProof {
    device_id: String,
    issuer: CanonicalOrigin,
    key: EncodingKey,
}

impl PairedDeviceProof {
    pub(super) fn new(identity: &StoredIdentity, issuer: &str) -> Result<Self, ProofError> {
        let issuer = CanonicalOrigin::parse(issuer)?;
        let approved_by = CanonicalOrigin::parse(
            identity
                .approved_by
                .as_deref()
                .unwrap_or(crate::config::DEFAULT_API_URL),
        )?;
        if !identity.approved || issuer != approved_by {
            return Err(ProofError::UnapprovedIdentity);
        }
        let device_id =
            uuid::Uuid::parse_str(&identity.device_id).map_err(|_| ProofError::InvalidInput)?;
        if device_id.hyphenated().to_string() != identity.device_id {
            return Err(ProofError::InvalidInput);
        }
        let seed = STANDARD_NO_PAD
            .decode(identity.identity_private_key_b64.trim_end_matches('='))
            .map_err(|_| ProofError::Signing)?;
        let public = STANDARD_NO_PAD
            .decode(identity.identity_public_key_b64.trim_end_matches('='))
            .map_err(|_| ProofError::Signing)?;
        let pair = Ed25519KeyPair::from_seed_and_public_key(&seed, &public)
            .map_err(|_| ProofError::Signing)?;
        let der = pair.to_pkcs8().map_err(|_| ProofError::Signing)?;
        Ok(Self {
            device_id: identity.device_id.clone(),
            issuer,
            key: EncodingKey::from_ed_der(der.as_ref()),
        })
    }

    pub(super) fn device_id(&self) -> &str {
        &self.device_id
    }

    pub(super) fn issuer(&self) -> &str {
        self.issuer.as_str()
    }

    pub(super) fn sign(
        &self,
        purpose: &str,
        audience: String,
        now: u64,
        mut fields: serde_json::Value,
    ) -> Result<String, ProofError> {
        let expiration = now.checked_add(30).ok_or(ProofError::InvalidInput)?;
        let object = fields.as_object_mut().ok_or(ProofError::InvalidInput)?;
        object.extend(serde_json::json!({"iss":self.device_id,"sub":self.device_id,"aud":audience,"iat":now,"exp":expiration,"jti":uuid::Uuid::new_v4().to_string()}).as_object().unwrap().clone());
        let mut header = Header::new(Algorithm::EdDSA);
        header.typ = Some(purpose.to_owned());
        jsonwebtoken::encode(&header, &fields, &self.key).map_err(|_| ProofError::Signing)
    }
}

pub struct DeviceProofSigner {
    paired: PairedDeviceProof,
    origin: CanonicalOrigin,
    endpoint_epoch: String,
    assignment_generation: Option<u64>,
}

impl DeviceProofSigner {
    pub fn new(
        identity: &StoredIdentity,
        issuer: &str,
        origin: CanonicalOrigin,
        endpoint_epoch: String,
    ) -> Result<Self, ProofError> {
        bounded(&endpoint_epoch, 128)?;
        Ok(Self {
            paired: PairedDeviceProof::new(identity, issuer)?,
            origin,
            endpoint_epoch,
            assignment_generation: None,
        })
    }

    pub(super) fn set_assignment_generation(&mut self, generation: Option<u64>) {
        self.assignment_generation = generation;
    }

    fn registration_fields(&self, mut fields: serde_json::Value) -> serde_json::Value {
        if let Some(generation) = self.assignment_generation {
            fields["assignment_generation"] = generation.into();
        }
        fields
    }

    pub fn device_id(&self) -> &str {
        self.paired.device_id()
    }
    pub fn issuer(&self) -> &str {
        self.paired.issuer()
    }
    pub fn origin(&self) -> &str {
        self.origin.as_str()
    }
    pub fn endpoint_epoch(&self) -> &str {
        &self.endpoint_epoch
    }
    pub fn introspection_url(&self) -> String {
        format!("{}/api/bridge-http/introspect", self.paired.issuer)
    }
    pub fn registration_url(&self) -> String {
        format!(
            "{}/api/devices/{}/http-endpoint",
            self.paired.issuer, self.paired.device_id
        )
    }

    /// Only nonce allocation at the paired issuer; no caller-selected claims.
    pub fn registration_challenge_assertion(&self, now: u64) -> Result<String, ProofError> {
        self.paired.sign(
            "build-http-registration-challenge+jwt",
            format!("{}/challenge", self.registration_url()),
            now,
            serde_json::json!({}),
        )
    }

    pub fn discovery_proof(
        &self,
        challenge_id: &str,
        nonce: &str,
        now: u64,
    ) -> Result<String, ProofError> {
        bounded(challenge_id, 128)?;
        bounded(nonce, 256)?;
        self.paired.sign("build-http-discovery+jwt", format!("{}/api/bridge-discovery/complete", self.paired.issuer), now,
            serde_json::json!({"challenge_id":challenge_id,"nonce":nonce,"origin":self.origin,"issuer":self.paired.issuer,"endpoint_epoch":self.endpoint_epoch}))
    }

    pub fn registration_proof(
        &self,
        challenge_id: &str,
        nonce: &str,
        previous_epoch: Option<&str>,
        enabled: bool,
        now: u64,
    ) -> Result<String, ProofError> {
        bounded(challenge_id, 128)?;
        bounded(nonce, 256)?;
        if let Some(epoch) = previous_epoch {
            bounded(epoch, 128)?;
        }
        self.paired.sign("build-http-registration+jwt",self.registration_url(),now,
            self.registration_fields(serde_json::json!({"challenge_id":challenge_id,"nonce":nonce,"origin":self.origin,"endpoint_epoch":self.endpoint_epoch,"previous_endpoint_epoch":previous_epoch,"enabled":enabled})))
    }

    pub fn registration_proof_candidates(
        &self,
        challenge_id: &str,
        nonce: &str,
        previous_epochs: &[Option<String>],
        enabled: bool,
        now: u64,
    ) -> Result<String, ProofError> {
        bounded(challenge_id, 128)?;
        bounded(nonce, 256)?;
        if previous_epochs.is_empty() || previous_epochs.len() > 8 {
            return Err(ProofError::InvalidInput);
        }
        for (index, epoch) in previous_epochs.iter().enumerate() {
            if let Some(epoch) = epoch {
                bounded(epoch, 128)?;
            }
            if previous_epochs[..index].contains(epoch) {
                return Err(ProofError::InvalidInput);
            }
        }
        self.paired.sign(
            "build-http-registration+jwt",
            self.registration_url(),
            now,
            self.registration_fields(serde_json::json!({
                "challenge_id": challenge_id,
                "nonce": nonce,
                "origin": self.origin,
                "endpoint_epoch": self.endpoint_epoch,
                "previous_endpoint_epochs": previous_epochs,
                "enabled": enabled,
            })),
        )
    }

    pub fn introspection_assertion(
        &self,
        token_hash: &str,
        now: u64,
    ) -> Result<String, ProofError> {
        if token_hash.len() != 64
            || !token_hash
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(ProofError::InvalidInput);
        }
        self.paired.sign(
            "build-http-introspection+jwt",
            self.introspection_url(),
            now,
            serde_json::json!({"token_hash":token_hash}),
        )
    }
}

fn bounded(value: &str, maximum: usize) -> Result<(), ProofError> {
    if value.is_empty() || value.len() > maximum || value.chars().any(char::is_control) {
        return Err(ProofError::InvalidInput);
    }
    Ok(())
}

#[derive(Deserialize)]
struct RegistrationChallenge {
    challenge_id: String,
    nonce: String,
    expires_at: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct RegisteredEndpoint {
    pub origin: String,
    pub endpoint_epoch: String,
    pub enabled: bool,
}

/// No bearer account credential or alternate credential URL enters this client.
pub struct EndpointRegistrar {
    signer: std::sync::Arc<DeviceProofSigner>,
    client: reqwest::Client,
    #[cfg(test)]
    test_endpoint: Option<String>,
}

impl EndpointRegistrar {
    pub fn new(signer: std::sync::Arc<DeviceProofSigner>) -> Result<Self, ProofError> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(2))
            .https_only(true)
            .build()
            .map_err(|_| ProofError::Unavailable)?;
        Ok(Self {
            signer,
            client,
            #[cfg(test)]
            test_endpoint: None,
        })
    }

    #[cfg(test)]
    pub(super) fn with_test_endpoint(
        signer: std::sync::Arc<DeviceProofSigner>,
        base: String,
    ) -> Self {
        let endpoint = format!("{base}/api/devices/{}/http-endpoint", signer.device_id());
        Self {
            signer,
            client: reqwest::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(5))
                .build()
                .unwrap(),
            test_endpoint: Some(endpoint),
        }
    }

    pub async fn register(
        &self,
        previous_epoch: Option<&str>,
        enabled: bool,
    ) -> Result<RegisteredEndpoint, ProofError> {
        self.register_with(previous_epoch, None, enabled).await
    }

    pub async fn register_candidates(
        &self,
        previous_epochs: &[Option<String>],
        enabled: bool,
    ) -> Result<RegisteredEndpoint, ProofError> {
        self.register_with(None, Some(previous_epochs), enabled)
            .await
    }

    async fn register_with(
        &self,
        previous_epoch: Option<&str>,
        previous_epochs: Option<&[Option<String>]>,
        enabled: bool,
    ) -> Result<RegisteredEndpoint, ProofError> {
        let url = self.signer.registration_url();
        #[cfg(test)]
        let url = self.test_endpoint.clone().unwrap_or(url);
        let allocation_proof = self.signer.registration_challenge_assertion(unix_now()?)?;
        let challenge: RegistrationChallenge = bounded_json(
            self.client
                .post(format!("{url}/challenge"))
                .json(&serde_json::json!({"proof":allocation_proof}))
                .send()
                .await
                .map_err(|_| ProofError::Unavailable)?,
        )
        .await?;
        let now = unix_now()?;
        if challenge.expires_at <= now || challenge.expires_at > now.saturating_add(65) {
            return Err(ProofError::Refused);
        }
        let proof = match previous_epochs {
            Some(previous_epochs) => self.signer.registration_proof_candidates(
                &challenge.challenge_id,
                &challenge.nonce,
                previous_epochs,
                enabled,
                now,
            )?,
            None => self.signer.registration_proof(
                &challenge.challenge_id,
                &challenge.nonce,
                previous_epoch,
                enabled,
                now,
            )?,
        };
        let response = self
            .client
            .post(url)
            .json(&serde_json::json!({"proof":proof}))
            .send()
            .await
            .map_err(|_| ProofError::Unavailable)?;
        if response.status() == reqwest::StatusCode::CONFLICT {
            return Err(ProofError::Conflict);
        }
        let registered: RegisteredEndpoint = bounded_json(response).await?;
        if registered.origin != self.signer.origin()
            || registered.endpoint_epoch != self.signer.endpoint_epoch()
            || registered.enabled != enabled
        {
            return Err(ProofError::Refused);
        }
        Ok(registered)
    }
}

pub(super) async fn bounded_json<T: serde::de::DeserializeOwned>(
    mut response: reqwest::Response,
) -> Result<T, ProofError> {
    if !response.status().is_success() {
        return Err(ProofError::Refused);
    }
    let mut bytes = zeroize::Zeroizing::new(Vec::new());
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| ProofError::Unavailable)?
    {
        if bytes.len().saturating_add(chunk.len()) > 16 * 1024 {
            return Err(ProofError::Refused);
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| ProofError::Refused)
}

pub(super) fn unix_now() -> Result<u64, ProofError> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| ProofError::Unavailable)
}

#[cfg(test)]
#[path = "discovery/tests.rs"]
mod tests;

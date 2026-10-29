//! Fixed-purpose device assertions and endpoint registration. Public discovery
//! signs captured configuration, never an origin or audience supplied by a peer.

use std::fmt;
use std::time::Duration;

use aws_lc_rs::signature::Ed25519KeyPair;
use base64::{engine::general_purpose::STANDARD, Engine};
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
pub struct DeviceProofSigner {
    device_id: String,
    issuer: CanonicalOrigin,
    origin: CanonicalOrigin,
    endpoint_epoch: String,
    key: EncodingKey,
}

impl DeviceProofSigner {
    pub fn new(
        identity: &StoredIdentity,
        issuer: &str,
        origin: CanonicalOrigin,
        endpoint_epoch: String,
    ) -> Result<Self, ProofError> {
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
        bounded(&endpoint_epoch, 128)?;
        let seed = STANDARD
            .decode(&identity.identity_private_key_b64)
            .map_err(|_| ProofError::Signing)?;
        let public = STANDARD
            .decode(&identity.identity_public_key_b64)
            .map_err(|_| ProofError::Signing)?;
        let pair = Ed25519KeyPair::from_seed_and_public_key(&seed, &public)
            .map_err(|_| ProofError::Signing)?;
        let der = pair.to_pkcs8().map_err(|_| ProofError::Signing)?;
        Ok(Self {
            device_id: identity.device_id.clone(),
            issuer,
            origin,
            endpoint_epoch,
            key: EncodingKey::from_ed_der(der.as_ref()),
        })
    }

    pub fn device_id(&self) -> &str {
        &self.device_id
    }
    pub fn issuer(&self) -> &str {
        self.issuer.as_str()
    }
    pub fn origin(&self) -> &str {
        self.origin.as_str()
    }
    pub fn endpoint_epoch(&self) -> &str {
        &self.endpoint_epoch
    }
    pub fn introspection_url(&self) -> String {
        format!("{}/api/bridge-http/introspect", self.issuer)
    }
    pub fn registration_url(&self) -> String {
        format!(
            "{}/api/devices/{}/http-endpoint",
            self.issuer, self.device_id
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
        self.sign("build-http-discovery+jwt", format!("{}/api/bridge-discovery/complete", self.issuer), now,
            serde_json::json!({"challenge_id":challenge_id,"nonce":nonce,"origin":self.origin,"issuer":self.issuer,"endpoint_epoch":self.endpoint_epoch}))
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
        self.sign("build-http-registration+jwt",self.registration_url(),now,
            serde_json::json!({"challenge_id":challenge_id,"nonce":nonce,"origin":self.origin,"endpoint_epoch":self.endpoint_epoch,"previous_endpoint_epoch":previous_epoch,"enabled":enabled}))
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
        self.sign(
            "build-http-introspection+jwt",
            self.introspection_url(),
            now,
            serde_json::json!({"token_hash":token_hash}),
        )
    }

    fn sign(
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
}

impl EndpointRegistrar {
    pub fn new(signer: std::sync::Arc<DeviceProofSigner>) -> Result<Self, ProofError> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(2))
            .https_only(true)
            .build()
            .map_err(|_| ProofError::Unavailable)?;
        Ok(Self { signer, client })
    }

    pub async fn register(
        &self,
        previous_epoch: Option<&str>,
        enabled: bool,
    ) -> Result<RegisteredEndpoint, ProofError> {
        let url = self.signer.registration_url();
        let challenge: RegistrationChallenge = bounded_json(
            self.client
                .post(format!("{url}/challenge"))
                .json(&serde_json::json!({}))
                .send()
                .await
                .map_err(|_| ProofError::Unavailable)?,
        )
        .await?;
        let now = unix_now()?;
        if challenge.expires_at <= now || challenge.expires_at > now.saturating_add(65) {
            return Err(ProofError::Refused);
        }
        let proof = self.signer.registration_proof(
            &challenge.challenge_id,
            &challenge.nonce,
            previous_epoch,
            enabled,
            now,
        )?;
        let registered: RegisteredEndpoint = bounded_json(
            self.client
                .post(url)
                .json(&serde_json::json!({"proof":proof}))
                .send()
                .await
                .map_err(|_| ProofError::Unavailable)?,
        )
        .await?;
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
    let mut bytes = Vec::new();
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

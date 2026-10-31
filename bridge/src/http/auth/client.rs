use super::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};

impl VerifierInner {
    pub(super) async fn lookup(
        &self,
        token: &str,
        digest: TokenDigest,
        state: Arc<TokenState>,
    ) -> Verification {
        let permit = tokio::time::timeout(NETWORK_DEADLINE, self.network.acquire())
            .await
            .map_err(|_| AuthError::Overloaded)?
            .map_err(|_| AuthError::Unavailable)?;
        let started = Instant::now();
        let wall = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| AuthError::Unavailable)?;
        let result = tokio::time::timeout(
            NETWORK_DEADLINE,
            self.request(token, digest, state, started, wall),
        )
        .await
        .map_err(|_| AuthError::Unavailable)?;
        drop(permit);
        result
    }

    async fn request(
        &self,
        token: &str,
        digest: TokenDigest,
        state: Arc<TokenState>,
        started: Instant,
        wall: Duration,
    ) -> Verification {
        let hash = hex_digest(&digest);
        let assertion = self.signer.assertion(&hash, wall.as_secs())?;
        let request = IntrospectionRequest {
            token,
            client_assertion: &assertion,
        };
        let mut response = self
            .client
            .post(self.signer.introspection_url())
            .json(&request)
            .send()
            .await
            .map_err(|_| AuthError::Unavailable)?;
        match response.status().as_u16() {
            200 => (),
            401 | 403 => return Err(AuthError::InvalidCredential),
            _ => return Err(AuthError::Unavailable),
        }
        if response
            .content_length()
            .is_some_and(|length| length > RESPONSE_LIMIT as u64)
        {
            return Err(AuthError::Unavailable);
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| AuthError::Unavailable)? {
            if body.len() + chunk.len() > RESPONSE_LIMIT {
                return Err(AuthError::Unavailable);
            }
            body.extend_from_slice(&chunk);
        }
        let response: IntrospectionResponse =
            serde_json::from_slice(&body).map_err(|_| AuthError::Unavailable)?;
        self.validate(response, state, started, wall)
    }

    fn validate(
        &self,
        response: IntrospectionResponse,
        state: Arc<TokenState>,
        started: Instant,
        wall: Duration,
    ) -> Verification {
        if !response.active {
            return Err(AuthError::InvalidCredential);
        }
        let binding = response.binding()?;
        let expected = self.signer.as_ref();
        if binding.issuer != expected.issuer()
            || binding.device_id != expected.device_id()
            || binding.origin != expected.origin()
            || binding.endpoint_epoch != expected.endpoint_epoch()
        {
            return Err(AuthError::InvalidCredential);
        }
        let expiry = response.exp.ok_or(AuthError::Unavailable)?;
        let remaining = Duration::from_secs(expiry)
            .checked_sub(wall)
            .ok_or(AuthError::InvalidCredential)?;
        let deadline = started + remaining.min(self.freshness.window);
        if Instant::now() >= deadline {
            return Err(AuthError::Expired);
        }
        Ok(Arc::new(Evidence {
            binding,
            deadline,
            refresh_at: (started + self.freshness.refresh_after).min(deadline),
            token_state: state,
        }))
    }
}

#[derive(Serialize)]
struct IntrospectionRequest<'a> {
    token: &'a str,
    client_assertion: &'a str,
}

#[derive(Deserialize)]
struct IntrospectionResponse {
    active: bool,
    issuer: Option<String>,
    account_id: Option<String>,
    device_id: Option<String>,
    origin: Option<String>,
    endpoint_epoch: Option<String>,
    authorization_epoch: Option<u64>,
    family_id: Option<String>,
    scope: Option<String>,
    exp: Option<u64>,
    token_id: Option<String>,
}

impl IntrospectionResponse {
    fn binding(&self) -> Result<HttpGrantBinding, AuthError> {
        let mut scopes: Vec<String> = field(&self.scope, 1024)?
            .split_ascii_whitespace()
            .map(str::to_owned)
            .collect();
        scopes.sort();
        scopes.dedup();
        if scopes.is_empty()
            || scopes
                .iter()
                .any(|s| !matches!(s.as_str(), "bridge:read" | "bridge:write"))
        {
            return Err(AuthError::InvalidCredential);
        }
        field(&self.token_id, 256)?;
        Ok(HttpGrantBinding {
            issuer: field(&self.issuer, 512)?,
            account_id: field(&self.account_id, 256)?,
            device_id: field(&self.device_id, 256)?,
            origin: field(&self.origin, 2048)?,
            endpoint_epoch: field(&self.endpoint_epoch, 256)?,
            family_id: field(&self.family_id, 256)?,
            authorization_epoch: self.authorization_epoch.ok_or(AuthError::Unavailable)?,
            scopes,
        })
    }
}

fn field(value: &Option<String>, limit: usize) -> Result<String, AuthError> {
    let value = value.as_ref().ok_or(AuthError::Unavailable)?;
    if value.is_empty() || value.len() > limit || value.chars().any(char::is_control) {
        return Err(AuthError::Unavailable);
    }
    Ok(value.clone())
}

pub(super) fn token_digest(token: &str) -> Result<TokenDigest, AuthError> {
    if token.is_empty() || token.len() > TOKEN_LIMIT || !token.bytes().all(|b| b.is_ascii_graphic())
    {
        return Err(AuthError::InvalidCredential);
    }
    Ok(Sha256::digest(token.as_bytes()).into())
}

pub(super) fn hex_digest(digest: &TokenDigest) -> String {
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(super) fn validate_destination(signer: &dyn AssertionSigner) -> Result<(), AuthError> {
    let url = reqwest::Url::parse(&signer.introspection_url())
        .map_err(|_| AuthError::InvalidConfiguration)?;
    let expected = format!("{}/api/bridge-http/introspect", signer.issuer());
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.as_str() != expected
    {
        return Err(AuthError::InvalidConfiguration);
    }
    Ok(())
}

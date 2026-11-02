//! Pure wire validation for the explicitly negotiated DTLS application codec.
//!
//! These values do not authorize intake. The RTC adapter must bind the actual
//! peer allocation, Opening and channel after verifying a one-time key proof.

use base64::{engine::general_purpose::STANDARD_NO_PAD, Engine};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::transport::{self, Envelope, FrameFields, OuterFields};

pub const CLEAR_PROTOCOL: &str = "dtls-json-v1";
pub const LEGACY_PROTOCOL: &str = "secretbox-v1";
pub const MAX_SEQUENCE: u64 = 9_007_199_254_740_991;
pub const MAX_CONTROL_BYTES: usize = 64 * 1024;

#[derive(Debug, thiserror::Error)]
#[error("invalid RTC binding: {0}")]
pub struct BindingError(&'static str);

type Result<T> = std::result::Result<T, BindingError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PayloadProtocol {
    Secretbox,
    DtlsJson,
}

impl PayloadProtocol {
    /// Only authenticated bootstrap signaling selects a codec. Missing
    /// advertisement is the old client's explicit compatibility case.
    pub fn select(advertised: Option<&Value>) -> Result<Self> {
        let Some(advertised) = advertised else {
            return Ok(Self::Secretbox);
        };
        let list = advertised.as_array().ok_or(BindingError("protocol list"))?;
        for protocol in list {
            match protocol.as_str().ok_or(BindingError("protocol name"))? {
                CLEAR_PROTOCOL => return Ok(Self::DtlsJson),
                LEGACY_PROTOCOL => return Ok(Self::Secretbox),
                _ => {}
            }
        }
        Err(BindingError("no supported payload protocol"))
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Secretbox => LEGACY_PROTOCOL,
            Self::DtlsJson => CLEAR_PROTOCOL,
        }
    }
}

/// Fresh peer tokens, nonces, challenges and binding IDs share one encoding:
/// canonical standard base64 without padding, with exactly 32 random bytes.
pub fn fresh_token() -> String {
    STANDARD_NO_PAD.encode(transport::random_bytes(32))
}

pub fn validate_token(token: &str) -> Result<()> {
    let decoded = STANDARD_NO_PAD
        .decode(token)
        .map_err(|_| BindingError("token encoding"))?;
    if decoded.len() != 32 || STANDARD_NO_PAD.encode(decoded) != token {
        return Err(BindingError("token length or encoding"));
    }
    Ok(())
}

pub fn sdp_hash(sdp: &str) -> String {
    Sha256::digest(sdp.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Application media inherits session-level fingerprints unless it provides
/// its own. Other media do not describe the DataChannel's DTLS certificate.
pub fn fingerprint(sdp: &str) -> Result<String> {
    let mut sections = vec![Vec::new()];
    for line in sdp.lines() {
        if line.starts_with("m=") {
            sections.push(Vec::new());
        }
        sections.last_mut().unwrap().push(line);
    }
    let session = &sections[0];
    let mut found = None;
    for section in sections
        .iter()
        .skip(1)
        .filter(|section| active_application(section))
    {
        let local = section
            .iter()
            .any(|line| line.starts_with("a=fingerprint:"));
        let next = fingerprint_of_lines(if local { section } else { session })?;
        if found.as_ref().is_some_and(|previous| previous != &next) {
            return Err(BindingError("conflicting application fingerprints"));
        }
        found = Some(next);
    }
    found.ok_or(BindingError("missing application media"))
}

fn active_application(section: &[&str]) -> bool {
    let Some(port) = section
        .first()
        .and_then(|line| line.strip_prefix("m=application "))
        .and_then(|rest| rest.split_once(' ').map(|(port, _)| port))
    else {
        return false;
    };
    !port.starts_with('0') && !port.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit())
}

fn fingerprint_of_lines(lines: &[&str]) -> Result<String> {
    let mut found = None;
    for line in lines {
        if let Some(value) = line.strip_prefix("a=fingerprint:") {
            let (algorithm, value) = value.split_once(' ').ok_or(BindingError("fingerprint"))?;
            if !algorithm.eq_ignore_ascii_case("sha-256") {
                return Err(BindingError("unsupported fingerprint algorithm"));
            }
            let normalized = normalize_fingerprint(value)?;
            if found
                .as_ref()
                .is_some_and(|previous| previous != &normalized)
            {
                return Err(BindingError("conflicting fingerprints"));
            }
            found = Some(normalized);
        }
    }
    found.ok_or(BindingError("missing fingerprint"))
}

fn normalize_fingerprint(value: &str) -> Result<String> {
    let bytes: Vec<_> = value.split(':').collect();
    if bytes.len() != 32
        || bytes
            .iter()
            .any(|byte| byte.len() != 2 || !byte.bytes().all(|c| c.is_ascii_hexdigit()))
    {
        return Err(BindingError("malformed fingerprint"));
    }
    Ok(bytes.concat().to_ascii_lowercase())
}

/// Captured from authenticated original SDP bytes, never from channel claims.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Transcript {
    pub peer_nonce: String,
    pub peer_token: String,
    pub offer_hash: String,
    pub answer_hash: String,
    pub offer_fingerprint: String,
    pub answer_fingerprint: String,
}

impl Transcript {
    pub fn new(offer: &str, answer: &str, peer_nonce: &str, peer_token: &str) -> Result<Self> {
        validate_token(peer_nonce)?;
        validate_token(peer_token)?;
        Ok(Self {
            peer_nonce: peer_nonce.into(),
            peer_token: peer_token.into(),
            offer_hash: sdp_hash(offer),
            answer_hash: sdp_hash(answer),
            offer_fingerprint: fingerprint(offer)?,
            answer_fingerprint: fingerprint(answer)?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelRole {
    App,
    Term,
}

impl ChannelRole {
    pub fn id(self) -> u16 {
        match self {
            Self::App => 0,
            Self::Term => 1,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::App => "app",
            Self::Term => "term",
        }
    }
}

/// Required typed proof fields. Equality follows decryption and parsing; JSON
/// property order and a reserialized byte representation are not proof rules.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BindProof {
    pub domain: String,
    pub protocol: String,
    pub session_id: String,
    pub peer_nonce: String,
    pub peer_token: String,
    pub offer_hash: String,
    pub answer_hash: String,
    pub offer_fingerprint: String,
    pub answer_fingerprint: String,
    pub channel_id: u16,
    pub channel_label: String,
    pub challenge: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub binding_id: Option<String>,
}

impl BindProof {
    pub fn new(
        session_id: &str,
        transcript: &Transcript,
        role: ChannelRole,
        challenge: &str,
    ) -> Result<Self> {
        validate_token(challenge)?;
        if session_id.is_empty() {
            return Err(BindingError("empty session"));
        }
        Ok(Self {
            domain: "channel_bind_v1".into(),
            protocol: CLEAR_PROTOCOL.into(),
            session_id: session_id.into(),
            peer_nonce: transcript.peer_nonce.clone(),
            peer_token: transcript.peer_token.clone(),
            offer_hash: transcript.offer_hash.clone(),
            answer_hash: transcript.answer_hash.clone(),
            offer_fingerprint: transcript.offer_fingerprint.clone(),
            answer_fingerprint: transcript.answer_fingerprint.clone(),
            channel_id: role.id(),
            channel_label: role.label().into(),
            challenge: challenge.into(),
            binding_id: None,
        })
    }

    /// One secretbox control proof/ack. Application messages do not call this.
    pub fn seal(
        &self,
        key: &str,
        acknowledgment: bool,
        binding_id: Option<&str>,
    ) -> Result<Envelope> {
        let mut proof = self.clone();
        if acknowledgment {
            proof.domain = "channel_bound_v1".into();
            proof.binding_id = binding_id.map(str::to_string);
        }
        transport::encrypt_frame(
            key,
            &OuterFields {
                session_id: self.session_id.clone(),
                route_to: "rtc:binding".into(),
            },
            &FrameFields {
                frame_type: "data".into(),
                sender: if acknowledgment { "device" } else { "client" }.into(),
                payload: serde_json::to_value(proof).map_err(|_| BindingError("proof JSON"))?,
                message_id: None,
                created_at: None,
            },
            None,
        )
        .map_err(|_| BindingError("control encryption"))
    }

    /// This verifies fields, not authority or single use. The adapter consumes
    /// its captured pending challenge exactly once before creating a lease.
    pub fn verify(
        &self,
        key: &str,
        envelope: &Envelope,
        acknowledgment: bool,
    ) -> Result<Option<String>> {
        if serde_json::to_vec(envelope)
            .map_err(|_| BindingError("control JSON"))?
            .len()
            > MAX_CONTROL_BYTES
        {
            return Err(BindingError("oversized control"));
        }
        let frame = transport::decrypt_envelope(key, envelope)
            .map_err(|_| BindingError("control authentication"))?;
        let expected_sender = if acknowledgment { "device" } else { "client" };
        if frame.session_id != self.session_id
            || frame.sender != expected_sender
            || frame.frame_type != "data"
        {
            return Err(BindingError("control frame identity"));
        }
        let mut received: Self =
            serde_json::from_value(frame.payload).map_err(|_| BindingError("control fields"))?;
        let binding_id = received.binding_id.take();
        if acknowledgment {
            validate_token(
                binding_id
                    .as_deref()
                    .ok_or(BindingError("missing binding ID"))?,
            )?;
            if received.domain != "channel_bound_v1" {
                return Err(BindingError("acknowledgment domain"));
            }
            received.domain = "channel_bind_v1".into();
        } else if binding_id.is_some() {
            return Err(BindingError("unexpected binding ID"));
        }
        if &received != self {
            return Err(BindingError("proof transcript mismatch"));
        }
        Ok(binding_id)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClearFrame {
    pub protocol: String,
    pub binding_id: String,
    pub seq: u64,
    pub frame_type: String,
    pub payload: Value,
}

/// Local ordered-channel fences, scoped to one already authorized binding.
/// This codec has no session lookup and cannot construct caller authority.
pub struct ClearCodec {
    binding_id: String,
    incoming: u64,
    outgoing: u64,
    live: bool,
}

impl ClearCodec {
    pub fn new(binding_id: &str) -> Result<Self> {
        validate_token(binding_id)?;
        Ok(Self {
            binding_id: binding_id.into(),
            incoming: 1,
            outgoing: 1,
            live: true,
        })
    }

    /// Call only inside the serialized writer, after its cancellation fence.
    pub fn encode(&mut self, frame_type: &str, payload: Value) -> Result<Value> {
        if !self.live || self.outgoing > MAX_SEQUENCE || !valid_frame_type(frame_type) {
            self.live = false;
            return Err(BindingError("closed or invalid outgoing frame"));
        }
        let frame = ClearFrame {
            protocol: CLEAR_PROTOCOL.into(),
            binding_id: self.binding_id.clone(),
            seq: self.outgoing,
            frame_type: frame_type.into(),
            payload,
        };
        self.outgoing += 1;
        self.live = frame_type != "close" && frame.seq < MAX_SEQUENCE;
        serde_json::to_value(frame).map_err(|_| BindingError("clear frame JSON"))
    }

    pub fn decode(&mut self, value: Value) -> Result<ClearFrame> {
        let frame: ClearFrame = match serde_json::from_value(value) {
            Ok(frame) => frame,
            Err(_) => {
                self.live = false;
                return Err(BindingError("clear frame fields"));
            }
        };
        if !self.accepts(&frame) {
            self.live = false;
            return Err(BindingError("clear frame binding or sequence"));
        }
        self.incoming += 1;
        self.live = frame.frame_type != "close" && frame.seq < MAX_SEQUENCE;
        Ok(frame)
    }

    fn accepts(&self, frame: &ClearFrame) -> bool {
        self.live
            && frame.protocol == CLEAR_PROTOCOL
            && frame.binding_id == self.binding_id
            && frame.seq == self.incoming
            && frame.seq <= MAX_SEQUENCE
            && valid_frame_type(&frame.frame_type)
    }
}

fn valid_frame_type(frame_type: &str) -> bool {
    matches!(frame_type, "data" | "close")
}

#[cfg(test)]
#[path = "binding/tests.rs"]
mod tests;

//! Terminal v1 validation, before shared invocation decoding.

use base64::{engine::general_purpose::STANDARD, Engine};
use serde::Deserialize;
use serde_json::{Map, Value};
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

use crate::{
    api::ApiError,
    execution::{Invocation, RequestMetadata},
};

const MAX_ID: usize = 128;
const MAX_DEPTH: usize = 16;
const MAX_CURSOR: u64 = 9_007_199_254_740_991;
const MAX_INPUT: usize = 64 * 1024;
const MAX_INPUT_ENCODED: usize = MAX_INPUT.div_ceil(3) * 4;
const SELECTORS: &[&str] = &[
    "workspace_id",
    "run_id",
    "project_id",
    "worktree_id",
    "source_id",
    "source_path",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Method {
    Create,
    Attach,
    AgentAttach,
    Input,
    Resize,
    Ack,
    Close,
    Ping,
}

impl Method {
    pub(super) fn parse(method: &str) -> Result<Self, ApiError> {
        match method {
            "term.create" => Ok(Self::Create),
            "term.attach" => Ok(Self::Attach),
            "agent.attach" => Ok(Self::AgentAttach),
            "term.input" => Ok(Self::Input),
            "term.resize" => Ok(Self::Resize),
            "term.ack" => Ok(Self::Ack),
            "term.close" => Ok(Self::Close),
            "ping" => Ok(Self::Ping),
            _ => Err(ApiError::unknown_method(method)),
        }
    }

    pub(super) fn is_attach(self) -> bool {
        matches!(self, Self::Attach | Self::AgentAttach)
    }

    pub(super) fn requires_attachment(self) -> bool {
        matches!(self, Self::Input | Self::Resize | Self::Ack)
    }

    fn fields(self) -> &'static [&'static str] {
        match self {
            Self::Create => &["kind", "cols", "rows"],
            Self::Attach => &["term_id", "cols", "rows"],
            Self::AgentAttach => &["id", "agent_id", "conversation_id", "cols", "rows"],
            Self::Input => &["term_id", "data"],
            Self::Resize => &["term_id", "cols", "rows"],
            Self::Ack => &["term_id", "cursor"],
            Self::Close => &["term_id"],
            Self::Ping => &[],
        }
    }

    fn accepts(self, field: &str) -> bool {
        self.fields().contains(&field)
            || (matches!(
                self,
                Self::Create | Self::Attach | Self::AgentAttach | Self::Close
            ) && SELECTORS.contains(&field))
    }

    fn grid_defaults(self) -> Option<(u64, u64)> {
        match self {
            Self::AgentAttach => Some((120, 40)),
            Self::Create | Self::Attach | Self::Resize => Some((80, 24)),
            _ => None,
        }
    }

    fn validate(self, params: &Map<String, Value>) -> Result<(), ApiError> {
        if params.keys().any(|name| !self.accepts(name)) {
            return Err(ApiError::invalid_params("Unexpected terminal parameter"));
        }
        if let Some(defaults) = self.grid_defaults() {
            validate_grid(params, defaults)?;
        }
        match self {
            Self::Input => validate_input(params.get("data").and_then(Value::as_str)),
            Self::Ack => validate_cursor(params.get("cursor").and_then(Value::as_u64)),
            _ => Ok(()),
        }
    }
}

fn validate_grid(params: &Map<String, Value>, (cols, rows): (u64, u64)) -> Result<(), ApiError> {
    let cols = params.get("cols").map_or(Some(cols), Value::as_u64);
    let rows = params.get("rows").map_or(Some(rows), Value::as_u64);
    if cols
        .zip(rows)
        .is_some_and(|(cols, rows)| crate::screen::wss_grid_supported(cols, rows))
    {
        Ok(())
    } else {
        Err(ApiError::invalid_params("Invalid terminal dimensions"))
    }
}

fn validate_input(data: Option<&str>) -> Result<(), ApiError> {
    let decoded = data
        .filter(|data| data.len() <= MAX_INPUT_ENCODED)
        .and_then(|data| STANDARD.decode(data).ok());
    if decoded.is_some_and(|bytes| bytes.len() <= MAX_INPUT) {
        Ok(())
    } else {
        Err(ApiError::invalid_params("Invalid terminal input"))
    }
}

fn validate_cursor(cursor: Option<u64>) -> Result<(), ApiError> {
    if cursor.is_some_and(|cursor| cursor <= MAX_CURSOR) {
        Ok(())
    } else {
        Err(ApiError::invalid_params("Invalid terminal cursor"))
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
pub(super) enum Incoming {
    Call(Request),
    Control(Control),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Request {
    pub(super) id: String,
    method: String,
    params: Map<String, Value>,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Control {
    Detach { attach_id: String },
}

impl Incoming {
    pub(super) fn parse(bytes: &[u8]) -> Result<Self, CloseCode> {
        let text = std::str::from_utf8(bytes).map_err(|_| CloseCode::Invalid)?;
        if !text
            .trim_start_matches([' ', '\t', '\r', '\n'])
            .starts_with('{')
        {
            return Err(CloseCode::Protocol);
        }
        let message = serde_json::from_str(text).map_err(|_| CloseCode::Protocol)?;
        let (id, nested) = match &message {
            Self::Call(request) => (
                request.id.as_str(),
                request.params.values().any(|value| exceeds_depth(value, 3)),
            ),
            Self::Control(Control::Detach { attach_id }) => (attach_id.as_str(), false),
        };
        if nested || !valid_id(id) {
            return Err(CloseCode::Protocol);
        }
        Ok(message)
    }
}

impl Request {
    pub(super) fn decode(self) -> Result<(Method, Invocation), ApiError> {
        let method = Method::parse(&self.method)?;
        method.validate(&self.params)?;
        let invocation = Invocation::decode(&self.method, &Value::Object(self.params))?
            .with_metadata(RequestMetadata {
                request_id: Some(self.id),
                ..RequestMetadata::default()
            })?;
        Ok((method, invocation))
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ID
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}

fn exceeds_depth(value: &Value, depth: usize) -> bool {
    match value {
        Value::Object(children) => {
            depth > MAX_DEPTH || children.values().any(|v| exceeds_depth(v, depth + 1))
        }
        Value::Array(children) => {
            depth > MAX_DEPTH || children.iter().any(|v| exceeds_depth(v, depth + 1))
        }
        _ => false,
    }
}

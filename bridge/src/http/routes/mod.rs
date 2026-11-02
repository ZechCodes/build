//! Explicit REST resource placement, with no HTTP framework or application logic.
//!
//! The runtime authenticates and binds the opening before calling [`operation`]
//! to reserve body bytes. After bounded parsing it calls [`resolve`], then the
//! shared `execution::Invocation::decode` and executor. A resolved route is not
//! authenticated authority and must never be dispatched without that context.
use percent_encoding::percent_decode_str;
use serde::Deserialize;
use serde_json::{Map, Value};
use std::sync::OnceLock;

/// Placement and limits only; domain validation belongs to the existing registry.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceOperation {
    pub method: String,
    pub operation_id: String,
    pub verb: String,
    pub path: String,
    pub path_params: Vec<String>,
    pub request_max_bytes: usize,
    pub response_max_bytes: usize,
    #[serde(rename = "requires_events")]
    pub requires_events: bool,
}

#[derive(Debug, Deserialize)]
pub struct ResourceManifest {
    pub version: u32,
    pub operations: Vec<ResourceOperation>,
}

/// Internal mapping result, never deserialized from an untrusted request.
#[derive(Debug)]
pub struct ResolvedRoute {
    pub method: &'static str,
    pub params: Value,
    pub operation: &'static ResourceOperation,
}

#[derive(Debug, PartialEq, Eq)]
pub enum RouteError {
    NotFound,
    MethodNotAllowed { allow: Vec<&'static str> },
    InvalidParams { message: String },
}

impl RouteError {
    fn invalid(message: impl Into<String>) -> Self {
        Self::InvalidParams {
            message: message.into(),
        }
    }
}

/// The same generated artifact used by the browser, desktop and OpenAPI tests.
pub fn manifest() -> &'static ResourceManifest {
    static MANIFEST: OnceLock<ResourceManifest> = OnceLock::new();
    MANIFEST.get_or_init(|| {
        serde_json::from_str(include_str!(
            "../../../../fixtures/api/http/resource-manifest.json"
        ))
        .expect("generated HTTP resource manifest must pass contract checks")
    })
}

/// Match metadata before buffering. This uses exactly the same matcher as resolve.
pub fn operation(
    verb: &str,
    path: &str,
    query: Option<&str>,
) -> Result<&'static ResourceOperation, RouteError> {
    match_resource(verb, path, query).map(|(operation, _)| operation)
}

/// Restore path selectors to the existing method params without dropping fields.
/// Unknown JSON fields reach the shared typed validator; a body cannot change a
/// path selector. Named queries preserve scalar/plural/nullable JSON semantics.
pub fn resolve(
    verb: &str,
    path: &str,
    query: Option<&str>,
    body: Value,
) -> Result<ResolvedRoute, RouteError> {
    let (operation, selectors) = match_resource(verb, path, query)?;
    let mut params = match body {
        Value::Null if operation.verb == "GET" => Map::new(),
        Value::Object(params) => params,
        _ => return Err(RouteError::invalid("resource body must be a JSON object")),
    };
    if operation.verb == "GET" && !params.is_empty() {
        return Err(RouteError::invalid(
            "GET resource selectors belong in the path",
        ));
    }
    for (name, value) in selectors {
        if params.get(&name).is_some_and(|body| body != &value) {
            return Err(RouteError::invalid(format!(
                "path/body disagreement for {name}"
            )));
        }
        params.insert(name, value);
    }
    Ok(ResolvedRoute {
        method: &operation.method,
        params: Value::Object(params),
        operation,
    })
}

type ResourceMatch = (&'static ResourceOperation, Map<String, Value>);

fn match_resource(
    verb: &str,
    path: &str,
    query: Option<&str>,
) -> Result<ResourceMatch, RouteError> {
    if query.is_some_and(|query| !query.is_empty()) {
        return Err(RouteError::invalid(
            "query strings are unsupported; use a named JSON query resource",
        ));
    }
    if !path.starts_with('/') || path.contains(['?', '#']) {
        return Err(RouteError::NotFound);
    }
    let mut allow = Vec::new();
    for operation in &manifest().operations {
        let Some(selectors) = match_path(&operation.path, path)? else {
            continue;
        };
        if operation.verb == verb {
            return Ok((operation, selectors));
        }
        allow.push(operation.verb.as_str());
    }
    if allow.is_empty() {
        return Err(RouteError::NotFound);
    }
    allow.sort_unstable();
    allow.dedup();
    Err(RouteError::MethodNotAllowed { allow })
}

fn match_path(template: &str, path: &str) -> Result<Option<Map<String, Value>>, RouteError> {
    let expected: Vec<_> = template.split('/').collect();
    let actual: Vec<_> = path.split('/').collect();
    if expected.len() != actual.len() {
        return Ok(None);
    }
    // Check static segments before parsing selectors of an unrelated route.
    if expected
        .iter()
        .zip(&actual)
        .any(|(expected, actual)| !expected.starts_with('{') && expected != actual)
    {
        return Ok(None);
    }
    let mut selectors = Map::new();
    for (expected, actual) in expected.iter().zip(actual) {
        if let Some(name) = expected
            .strip_prefix('{')
            .and_then(|name| name.strip_suffix('}'))
        {
            selectors.insert(name.into(), Value::String(decode_segment(actual)?));
        }
    }
    Ok(Some(selectors))
}

fn decode_segment(segment: &str) -> Result<String, RouteError> {
    let bytes = segment.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == b'%'
            && !bytes
                .get(index + 1..index + 3)
                .is_some_and(|hex| hex.iter().all(u8::is_ascii_hexdigit))
        {
            return Err(RouteError::invalid("invalid path percent encoding"));
        }
    }
    let decoded = percent_decode_str(segment)
        .decode_utf8()
        .map_err(|_| RouteError::invalid("path selector must be UTF-8"))?;
    if decoded.is_empty()
        || matches!(decoded.as_ref(), "." | "..")
        || decoded
            .chars()
            .any(|c| c.is_control() || matches!(c, '/' | '\\' | '?' | '#'))
    {
        return Err(RouteError::invalid("invalid opaque path selector"));
    }
    Ok(decoded.into_owned())
}

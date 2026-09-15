//! What each live session said it was.
//!
//! `session.hello` may carry a `client` object — `{name, version, api_range}`,
//! every field optional — and the bridge keeps it for as long as the session
//! is open. `bridge.stats` counts the live sessions by the range they asked
//! for, so retiring an API major is a decision made from numbers rather than
//! from a guess about who is still out there.
//!
//! The registry is a leaf: its one mutex guards a small map and nothing that
//! takes it holds anything else. `bridge.stats` reads it without the app
//! mutex, which is the property that makes the stats verb worth having.

use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

use serde_json::{json, Value};

/// The range a session is counted under when it declared none, or declared
/// something that was not a string.
pub const UNKNOWN_RANGE: &str = "unknown";

/// What one session declared about itself. Absent fields stay absent; a
/// field of the wrong type is treated as absent, never as an error.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeclaredClient {
    pub name: Option<String>,
    pub version: Option<String>,
    pub api_range: Option<String>,
}

impl DeclaredClient {
    /// Read the optional `client` object out of a `session.hello`'s params.
    pub fn from_hello_params(params: &Value) -> DeclaredClient {
        let client = params.get("client");
        let field = |key: &str| {
            client
                .and_then(|client| client.get(key))
                .and_then(Value::as_str)
                .map(str::to_string)
        };
        DeclaredClient {
            name: field("name"),
            version: field("version"),
            api_range: field("api_range"),
        }
    }
}

/// The live sessions' declarations, keyed by session id.
#[derive(Default)]
pub struct ClientRegistry {
    sessions: Mutex<HashMap<String, DeclaredClient>>,
}

impl ClientRegistry {
    pub fn new() -> ClientRegistry {
        ClientRegistry::default()
    }

    /// A session greeted. Greeting again replaces what it said before.
    pub fn record(&self, session_id: &str, client: DeclaredClient) {
        self.sessions
            .lock()
            .unwrap()
            .insert(session_id.to_string(), client);
    }

    /// A session ended; whatever it declared no longer counts.
    pub fn forget(&self, session_id: &str) {
        self.sessions.lock().unwrap().remove(session_id);
    }

    /// Live sessions by declared `api_range`, as the `clients` object in
    /// `bridge.stats`. Sorted so the reply is stable to read.
    pub fn counts(&self) -> Value {
        let mut counts: BTreeMap<String, u64> = BTreeMap::new();
        for client in self.sessions.lock().unwrap().values() {
            let range = client.api_range.as_deref().unwrap_or(UNKNOWN_RANGE);
            *counts.entry(range.to_string()).or_insert(0) += 1;
        }
        json!(counts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_client_is_read_from_the_hello_params() {
        let declared = DeclaredClient::from_hello_params(&json!({
            "client": { "name": "spa", "version": "abc", "api_range": ">=1.0.0 <2.0.0" }
        }));
        assert_eq!(
            declared,
            DeclaredClient {
                name: Some("spa".into()),
                version: Some("abc".into()),
                api_range: Some(">=1.0.0 <2.0.0".into()),
            }
        );
    }

    #[test]
    fn a_missing_or_malformed_client_is_an_empty_declaration() {
        for params in [
            json!({}),
            json!({ "client": null }),
            json!({ "client": "spa" }),
            json!({ "client": { "name": 1, "version": [], "api_range": {} } }),
        ] {
            assert_eq!(
                DeclaredClient::from_hello_params(&params),
                DeclaredClient::default(),
                "{params}"
            );
        }
    }

    #[test]
    fn counts_group_live_sessions_by_range_and_forget_closed_ones() {
        let registry = ClientRegistry::new();
        let v1 = DeclaredClient {
            api_range: Some(">=1.0.0 <2.0.0".into()),
            ..DeclaredClient::default()
        };
        registry.record("a", v1.clone());
        registry.record("b", v1.clone());
        registry.record("c", DeclaredClient::default());
        registry.record("a", v1);
        assert_eq!(
            registry.counts(),
            json!({ ">=1.0.0 <2.0.0": 2, UNKNOWN_RANGE: 1 })
        );
        registry.forget("a");
        registry.forget("never-greeted");
        assert_eq!(
            registry.counts(),
            json!({ ">=1.0.0 <2.0.0": 1, UNKNOWN_RANGE: 1 })
        );
    }

    #[test]
    fn an_empty_registry_counts_nothing() {
        assert_eq!(ClientRegistry::new().counts(), json!({}));
    }
}

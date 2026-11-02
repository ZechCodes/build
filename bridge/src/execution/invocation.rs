//! Validated application requests at the transport codec boundary.

use crate::api::ApiError;
use serde_json::Value;

/// A recognized application method and its parameters at the shared boundary.
///
/// Registered methods normalize through the existing typed v1 registry. Legacy
/// methods have no declared parameter types yet: their unchanged domain parsers
/// still validate them during execution. Preserve those objects verbatim so
/// malformed optional defaults, aliases and presence-sensitive scope checks keep
/// their existing behavior. This wrapper does not grant caller authority.
/// Private fields prevent adapters from inventing an unrecognized method.
/// Wire metadata remains outside semantic intent and caller authority.
#[derive(Debug, Clone, Default)]
pub struct RequestMetadata {
    pub request_id: Option<String>,
    pub priority: crate::timing::Priority,
    pub write: Option<crate::api::writes::WriteMetadata>,
}

#[derive(Debug, Clone)]
pub struct Invocation {
    method: String,
    params: Value,
    metadata: RequestMetadata,
}

impl Invocation {
    pub fn decode(method: &str, params: &Value) -> Result<Self, ApiError> {
        // Signaling stays with WebRTC; no generic RPC method tunnel is part of
        // the application registry exposed to transport adapters.
        if method.starts_with("rtc.") || method.starts_with("rpc.") {
            return Err(ApiError::unknown_method(method));
        }
        let params = match crate::api::v1::normalize_read_params(method, params) {
            Ok(params) => params,
            Err(ApiError::UnknownMethod { .. }) => legacy_params(method, params)?,
            Err(error) => return Err(error),
        };
        Ok(Self {
            method: method.into(),
            params,
            metadata: RequestMetadata::default(),
        })
    }

    pub fn with_metadata(mut self, metadata: RequestMetadata) -> Result<Self, ApiError> {
        let mut payload = serde_json::json!({"method":self.method});
        if let Some(write) = &metadata.write {
            payload["write"] = serde_json::to_value(write)
                .map_err(|error| ApiError::invalid_params(error.to_string()))?;
        }
        crate::api::v1::validate_write_metadata(&payload)?;
        self.metadata = metadata;
        Ok(self)
    }

    pub(crate) fn into_frame(self, session_id: &str, order: u64) -> crate::transport::Frame {
        let id = self
            .metadata
            .request_id
            .map(Value::String)
            .unwrap_or_else(|| order.into());
        let mut payload = serde_json::json!({"id":id,"method":self.method,"params":self.params,"priority":self.metadata.priority});
        if let Some(write) = self.metadata.write {
            payload["write"] = serde_json::to_value(write).expect("write metadata serializes");
        }
        crate::transport::Frame {
            session_id: session_id.into(),
            message_id: order.to_string(),
            frame_type: "data".into(),
            sender: "client".into(),
            created_at: String::new(),
            payload,
        }
    }

    pub fn method(&self) -> &str {
        &self.method
    }

    pub fn params(&self) -> &Value {
        &self.params
    }
}

fn legacy_params(method: &str, params: &Value) -> Result<Value, ApiError> {
    // QA availability is still checked by AppState.
    let application_method =
        crate::api::LEGACY_METHODS.contains(&method) || crate::api::QA_METHODS.contains(&method);
    if !application_method {
        return Err(ApiError::unknown_method(method));
    }
    if !params.is_object() {
        return Err(ApiError::invalid_params("params must be an object"));
    }
    Ok(params.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn registered_fixture_parameters_use_the_existing_typed_registry() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/api/v1");
        for (method, _) in crate::api::v1::methods() {
            let path = root.join(format!("{method}.json"));
            let fixture: Value =
                serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            let decoded = Invocation::decode(method, &fixture["params"])
                .unwrap_or_else(|error| panic!("{method}: {error:?}"));
            assert_eq!(decoded.method(), *method);
            assert_eq!(
                decoded.params(),
                &crate::api::v1::normalize_read_params(method, &fixture["params"]).unwrap()
            );
        }
    }

    #[test]
    fn registered_methods_reject_unknown_parameters_and_bad_types() {
        for params in [json!({"surprise": true}), json!({"project_id": 4})] {
            assert_eq!(
                Invocation::decode("tasks.list", &params)
                    .unwrap_err()
                    .code(),
                "invalid_params"
            );
        }
    }

    #[test]
    fn unknown_methods_and_signaling_cannot_enter_application_execution() {
        for method in [
            "missing.method",
            "rtc.offer",
            "rtc.ice",
            "rtc.close",
            "rpc.call",
        ] {
            let error = Invocation::decode(method, &json!({})).unwrap_err();
            assert_eq!(error.code(), "unknown_method");
            assert!(error.message().contains(method));
        }
    }

    #[test]
    fn legacy_nonterminal_calls_preserve_parameters() {
        for (method, params) in [
            ("ping", json!({})),
            ("bridge.stats", json!({})),
            (
                "session.hello",
                json!({"changes": "subscriptions", "client": {
                    "name": "spa", "version": "1", "api_range": ">=3.0.0"
                }}),
            ),
            (
                "agent.attach",
                json!({"id": "run-1", "agent_id": "agent-1", "cols": 90}),
            ),
            (
                "agent.start",
                json!({"run_id": "run-1", "agent_id": "agent-1",
                "conversation_id": "conversation-1", "thread_id": "thread-1",
                "provider": "codex", "model": "model", "effort": "high"}),
            ),
            (
                "agent.interrupt",
                json!({"entity_id": "run-1", "agent_id": "agent-1",
                "conversation_id": "conversation-1", "thread_id": "thread-1"}),
            ),
            ("stream.start", json!({"count": 3, "interval_ms": 1})),
            (
                "stream.events",
                json!({"stream_id": "stream-1", "since": 2, "limit": 5}),
            ),
            ("stream.state", json!({"stream_id": "stream-1"})),
            ("user.present", json!({})),
        ] {
            let decoded = Invocation::decode(method, &params)
                .unwrap_or_else(|error| panic!("{method}: {error:?}"));
            assert_eq!(decoded.params(), &params, "{method}");
        }
    }

    #[test]
    fn terminal_calls_keep_scopes_flow_control_and_server_order_inputs() {
        for (method, params) in [
            (
                "term.create",
                json!({"project_id": "project-1", "source_id": "source-1",
                "source_path": "/source", "kind": "shell", "cols": 90, "rows": 30}),
            ),
            (
                "term.list",
                json!({"workspace_id": "workspace-1", "source_id": "source-1"}),
            ),
            (
                "term.attach",
                json!({"term_id": "term-1", "workspace_id": "workspace-1",
                "cols": 90, "rows": 30}),
            ),
            (
                "term.close",
                json!({"term_id": "term-1", "project_id": "project-1",
                "source_id": "source-1", "source_path": "/source"}),
            ),
            ("term.input", json!({"term_id": "term-1", "data": "YQ=="})),
            (
                "term.resize",
                json!({"term_id": "term-1", "cols": 90, "rows": 30}),
            ),
            ("term.ack", json!({"term_id": "term-1", "cursor": 42})),
        ] {
            let decoded = Invocation::decode(method, &params)
                .unwrap_or_else(|error| panic!("{method}: {error:?}"));
            assert_eq!(decoded.params(), &params, "{method}");
        }
    }

    #[test]
    fn legacy_parameters_preserve_existing_defaults_and_domain_validation() {
        for (method, params) in [
            ("bridge.stats", json!({"method": "fs.write"})),
            ("session.hello", json!({"client": {"name": 4}})),
            (
                "session.hello",
                json!({"client": "old-client", "changes": false}),
            ),
            ("agent.start", json!({"id": "run-1", "method": "fs.write"})),
            ("agent.start", json!({"id": null, "run_id": "run-1"})),
            ("agent.interrupt", json!({"entity_id": "run-1"})),
            ("term.input", json!({"term_id": "term-1"})),
            (
                "term.create",
                json!({"project_id": "project-1", "source_id": null}),
            ),
            (
                "term.resize",
                json!({"term_id": "term-1", "cols": "default", "rows": 65537}),
            ),
            ("term.ack", json!({"term_id": "term-1", "cursor": -1})),
            ("stream.start", json!({"count": "many"})),
            ("stream.state", json!({"stream_id": "stream-1", "extra": 1})),
        ] {
            assert_eq!(
                Invocation::decode(method, &params).unwrap().params(),
                &params,
                "{method}: {params}"
            );
        }
    }

    #[test]
    fn legacy_arguments_must_be_parameter_objects() {
        for params in [Value::Null, json!([]), json!(4), json!("text")] {
            assert_eq!(
                Invocation::decode("session.hello", &params)
                    .unwrap_err()
                    .code(),
                "invalid_params"
            );
        }
    }

    #[test]
    fn parameters_cannot_select_an_alternate_method() {
        let decoded = Invocation::decode("bridge.stats", &json!({"method": "fs.write"})).unwrap();
        assert_eq!(decoded.method(), "bridge.stats");
        assert_eq!(
            Invocation::decode("not.a.method", &json!({"method": "board.list"}))
                .unwrap_err()
                .code(),
            "unknown_method"
        );
    }

    #[test]
    fn absent_optional_fields_are_not_materialized() {
        assert_eq!(
            Invocation::decode("session.hello", &json!({}))
                .unwrap()
                .params(),
            &json!({})
        );
        assert_eq!(
            Invocation::decode("agent.start", &json!({"id": "run-1"}))
                .unwrap()
                .params(),
            &json!({"id": "run-1"})
        );
    }
}

#[cfg(test)]
mod metadata_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn request_id_and_priority_are_wire_metadata_not_normalized_intent() {
        let invocation = Invocation::decode("board.list", &json!({}))
            .unwrap()
            .with_metadata(RequestMetadata {
                request_id: Some("request-42".into()),
                priority: crate::timing::Priority::Background,
                write: None,
            })
            .unwrap();
        assert_eq!(invocation.params(), &json!({}));
        let frame = invocation.into_frame("opening", 10);
        assert_eq!(frame.payload["id"], "request-42");
        assert_eq!(frame.payload["priority"], "background");
        assert!(frame.payload.get("write").is_none());
    }

    #[test]
    fn legacy_six_keep_unknown_field_compatibility_but_reject_non_objects() {
        for method in [
            "agent.start",
            "agent.interrupt",
            "bridge.stats",
            "ping",
            "session.hello",
            "term.list",
        ] {
            let params = json!({"unknown_future_field": 1});
            assert_eq!(
                Invocation::decode(method, &params).unwrap().params(),
                &params
            );
            for bad in [Value::Null, json!([]), json!("params")] {
                assert_eq!(
                    Invocation::decode(method, &bad).unwrap_err().code(),
                    "invalid_params"
                );
            }
        }
    }

    #[test]
    fn operation_metadata_cannot_enable_durable_mode_on_a_legacy_method() {
        let write: crate::api::writes::WriteMetadata = serde_json::from_value(
            json!({"operation_id":"9a2978a4-d2bf-4ab3-9941-dbd9237d0c27","after":[]}),
        )
        .unwrap();
        let error = Invocation::decode("board.list", &json!({}))
            .unwrap()
            .with_metadata(RequestMetadata {
                write: Some(write),
                ..Default::default()
            })
            .unwrap_err();
        assert_eq!(error.code(), "unavailable");
    }
}

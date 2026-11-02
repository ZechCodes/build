//! Characterize the six exported legacy resources at the existing production
//! dispatcher, rather than inventing typed parameter contracts for them.
//!
//! These tests do not exercise the future execution::Invocation object-shape
//! gate. The current legacy handlers inspect fields themselves, preserve unknown
//! field compatibility, and sometimes ignore malformed optional fields. Agent
//! control tests stop at required/domain refusals so no harness is launched.

use build_bridge::{
    app::AppState,
    carrier::{FrameHandler, SessionSender},
    harness::HarnessContext,
    transport::Frame,
};
use serde_json::{json, Value};

struct LegacyCase {
    _directory: tempfile::TempDir,
    handler: FrameHandler,
    project_id: String,
}

impl LegacyCase {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let context = HarnessContext::resolved(
            directory.path().join("mcp.sock"),
            directory.path().join("state"),
        )
        .unwrap();
        let mut app = AppState::new_unrooted_configured(
            directory.path().join("worktrees"),
            "main",
            true,
            context,
        );
        let project_id = app.add_project(directory.path().to_path_buf(), "main".into());
        Self {
            _directory: directory,
            handler: app.into_handler(),
            project_id,
        }
    }

    fn call(&self, method: &str, params: Value) -> Value {
        self.handler.call(
            SessionSender::detached("legacy-resource-test"),
            Frame {
                session_id: "legacy-resource-test".into(),
                message_id: "legacy-call".into(),
                frame_type: "data".into(),
                sender: "client".into(),
                created_at: "test".into(),
                payload: json!({"id":"legacy-call", "method":method, "params":params}),
            },
        )
    }

    fn result(&self, method: &str, params: Value) -> Value {
        let reply = self.call(method, params);
        assert_eq!(reply["id"], "legacy-call", "{reply}");
        assert_eq!(reply["ok"], true, "{method}: {reply}");
        reply["result"].clone()
    }

    fn refusal(&self, method: &str, params: Value, message: &str) {
        let reply = self.call(method, params);
        assert_eq!(reply["ok"], false, "{method}: {reply}");
        assert!(
            reply["error"].as_str().unwrap().contains(message),
            "{reply}"
        );
    }
}

#[tokio::test]
async fn ping_ignores_unknown_params_and_retains_liveness_reply() {
    let case = LegacyCase::new();
    let ordinary = case.result("ping", json!({}));
    assert_eq!(ordinary["pong"], true);
    assert!(ordinary["api_version"].is_string());
    assert_eq!(
        case.result("ping", json!({"unrecognized_http_param": {"nested":true}})),
        ordinary
    );
    // The current handler does not impose an object gate on this legacy verb.
    assert_eq!(case.result("ping", json!([false])), ordinary);
}

#[tokio::test]
async fn stats_ignores_unknown_params_and_reports_registered_clients() {
    let case = LegacyCase::new();
    for params in [
        json!({}),
        json!({"unrecognized_http_param":true}),
        json!(null),
    ] {
        let stats = case.result("bridge.stats", params);
        assert!(stats["clients"].is_object(), "{stats}");
    }
}

#[tokio::test]
async fn hello_preserves_unknown_fields_and_malformed_optional_client_compatibility() {
    let case = LegacyCase::new();
    let ordinary = case.result("session.hello", json!({}));
    assert_eq!(ordinary["push_events"], true);
    assert!(ordinary["api_version"].is_string());
    case.result(
        "session.hello",
        json!({
            "client": {"name":"fixture", "version":"1", "api_range":"legacy-test"},
            "unrecognized_http_param": {"kept_compatible":true}
        }),
    );
    assert_eq!(
        case.result("bridge.stats", json!({}))["clients"]["legacy-test"],
        1
    );
    for params in [
        json!({"client":{"name":12,"version":[],"api_range":false}}),
        json!({"client":false,"unrecognized_http_param":true}),
    ] {
        let greeting = case.result("session.hello", params);
        assert_eq!(greeting["api_version"], ordinary["api_version"]);
        assert_eq!(
            case.result("bridge.stats", json!({}))["clients"]["unknown"],
            1
        );
    }
}

#[tokio::test]
async fn terminal_roster_resolves_scope_and_keeps_legacy_optional_field_defaults() {
    let case = LegacyCase::new();
    let empty = json!({"terminals":[]});
    assert_eq!(
        case.result("term.list", json!({"project_id":case.project_id})),
        empty
    );
    assert_eq!(
        case.result(
            "term.list",
            json!({
                "project_id":case.project_id,
                "run_id":17,
                "worktree_id":false,
                "unrecognized_http_param":true
            })
        ),
        empty
    );
    case.refusal("term.list", json!({}), "missing scope");
    case.refusal("term.list", json!({"project_id":12}), "missing scope");
    case.refusal(
        "term.list",
        json!({"project_id":"missing-project"}),
        "unknown project_id",
    );
}

#[tokio::test]
async fn agent_start_validates_required_id_and_existing_entity_before_any_spawn() {
    let case = LegacyCase::new();
    for params in [json!({}), json!({"id":17}), json!({"id":""})] {
        case.refusal("agent.start", params, "missing id");
    }
    for key in ["id", "run_id", "plan_id"] {
        let mut params = json!({"unrecognized_http_param":true});
        params[key] = json!("missing-entity");
        case.refusal("agent.start", params, "unknown id");
    }
}

#[tokio::test]
async fn agent_interrupt_validates_required_address_and_existing_entity() {
    let case = LegacyCase::new();
    case.refusal("agent.interrupt", json!({}), "entity_id");
    case.refusal("agent.interrupt", json!({"entity_id":false}), "entity_id");
    case.refusal(
        "agent.interrupt",
        json!({"entity_id":"missing-entity"}),
        "agent_id",
    );
    case.refusal(
        "agent.interrupt",
        json!({
            "entity_id":"missing-entity", "agent_id":"missing-agent", "conversation_id":false
        }),
        "conversation_id",
    );
    for extra in [
        json!({}),
        json!({"unrecognized_http_param":{"nested":true}}),
    ] {
        let mut params = json!({
            "entity_id":"missing-entity", "agent_id":"missing-agent", "conversation_id":"missing-conversation"
        });
        params
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        case.refusal("agent.interrupt", params, "unknown id");
    }
}

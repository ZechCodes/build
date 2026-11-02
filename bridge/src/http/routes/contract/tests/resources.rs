use build_http_resource_contract::{manifest, resolve, RouteError};
use serde_json::json;

#[test]
fn task_query_preserves_nullable_plural_and_unknown_fields() {
    let body = json!({"state":null,"statuses":["ready","done"],"limit":25,"future_param":true});
    let route = resolve(
        "POST",
        "/v1/projects/project-a/task-queries",
        None,
        body.clone(),
    )
    .unwrap();
    assert_eq!(route.method, "tasks.list");
    let mut expected = body;
    expected["project_id"] = json!("project-a");
    assert_eq!(route.params, expected);
}

#[test]
fn path_identity_is_authoritative_and_cannot_be_overridden() {
    assert!(matches!(
        resolve(
            "PATCH",
            "/v1/tasks/task-a",
            None,
            json!({"task_id":"task-b","title":"Changed"})
        ),
        Err(RouteError::InvalidParams { .. })
    ));
    let route = resolve(
        "PATCH",
        "/v1/tasks/task-a",
        None,
        json!({"task_id":"task-a","title":"Changed"}),
    )
    .unwrap();
    assert_eq!(route.params["task_id"], "task-a");
}

#[test]
fn routes_do_not_accept_method_tunnels_or_terminal_output() {
    for path in [
        "/rpc",
        "/v1/rpc",
        "/v1/term/input",
        "/v1/rtc/offer",
        "/v1/agents/attach",
    ] {
        assert!(matches!(
            resolve(
                "POST",
                path,
                None,
                json!({"method":"tasks.create","params":{}})
            ),
            Err(RouteError::NotFound)
        ));
    }
    assert_eq!(
        resolve(
            "POST",
            "/v1/terminal-roster-queries",
            None,
            json!({"project_id":"p"})
        )
        .unwrap()
        .method,
        "term.list"
    );
}

#[test]
fn missing_resources_and_wrong_verbs_are_distinct() {
    assert!(matches!(
        resolve("GET", "/v1/not-a-resource", None, json!({})),
        Err(RouteError::NotFound)
    ));
    let Err(RouteError::MethodNotAllowed { allow }) =
        resolve("PUT", "/v1/projects", None, json!({}))
    else {
        panic!("expected 405")
    };
    assert_eq!(allow, vec!["GET", "POST"]);
}

#[test]
fn selectors_are_json_and_never_silently_dropped() {
    assert!(matches!(
        resolve("GET", "/v1/projects", Some("project_id=other"), json!({})),
        Err(RouteError::InvalidParams { .. })
    ));
    assert!(matches!(
        resolve("POST", "/v1/projects", None, json!([])),
        Err(RouteError::InvalidParams { .. })
    ));
    assert!(matches!(
        resolve("GET", "/v1/projects", None, json!({"unexpected":1})),
        Err(RouteError::InvalidParams { .. })
    ));
}

#[test]
fn path_decoding_is_once_only_and_rejects_ambiguous_segments() {
    let route = resolve("PATCH", "/v1/tasks/task%20%C3%A9", None, json!({})).unwrap();
    assert_eq!(route.params["task_id"], "task é");
    for id in [
        "%", "%GG", "%FF", "%2F", "%5C", "%00", "%2e%2e", "..", ".", "",
    ] {
        assert!(
            resolve("PATCH", &format!("/v1/tasks/{id}"), None, json!({})).is_err(),
            "{id}"
        );
    }
    assert!(resolve("PATCH", "/v1/tasks/a/", None, json!({})).is_err());
    assert!(resolve("PATCH", "https://other/v1/tasks/a", None, json!({})).is_err());
}

#[test]
fn every_operation_roundtrips_its_declared_path_and_remaining_json() {
    for operation in &manifest().operations {
        let mut path = operation.path.clone();
        let mut expected = json!({});
        for name in &operation.path_params {
            path = path.replace(&format!("{{{name}}}"), "opaque-id");
            expected[name] = json!("opaque-id");
        }
        let body = if operation.verb == "GET" {
            json!({})
        } else {
            json!({"probe":null,"selectors":[1,2]})
        };
        for (key, value) in body.as_object().unwrap() {
            expected[key] = value.clone();
        }
        let route = resolve(&operation.verb, &path, None, body).unwrap();
        assert_eq!(route.method, operation.method);
        assert_eq!(route.params, expected, "{}", operation.method);
        assert!(route.operation.response_max_bytes > 0);
    }
}

#[test]
fn metadata_lookup_precedes_body_read_and_has_exact_scoped_limits() {
    use build_http_resource_contract::operation;
    for (verb, path, expected) in [
        ("POST", "/v1/projects/p/task-attachments", 7_056_044),
        ("POST", "/v1/entities/e/attachments", 7_056_044),
        ("PUT", "/v1/file-uploads/u/chunks", 5_657_944),
        ("PUT", "/v1/files/content", 1_463_640),
        ("POST", "/v1/projects/p/tasks", 1_048_576),
    ] {
        let metadata = operation(verb, path, None).unwrap();
        assert_eq!(metadata.request_max_bytes, expected);
        assert_eq!(metadata.response_max_bytes, 8_388_608);
        let decoded = resolve(verb, path, None, json!({})).unwrap();
        assert!(std::ptr::eq(metadata, decoded.operation));
    }
    assert!(
        !operation("POST", "/v1/entities/e/attachments", None)
            .unwrap()
            .requires_events
    );
    assert!(
        operation("POST", "/v1/file-uploads", None)
            .unwrap()
            .requires_events
    );
    assert!(
        operation("POST", "/v1/change-subscriptions", None)
            .unwrap()
            .requires_events
    );
}

#[test]
fn every_fixture_passes_through_its_actual_resource_decoder_without_loss() {
    use std::path::Path;
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../../..");
    for op in &manifest().operations {
        let path = root.join(format!("fixtures/api/v1/{}.json", op.method));
        if !path.exists() {
            assert!(["agent.start", "agent.interrupt", "bridge.stats", "ping"]
                .contains(&op.method.as_str()));
            continue;
        }
        let fixture: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let params = fixture["params"].clone();
        let mut body = params.clone();
        let mut path = op.path.clone();
        for key in &op.path_params {
            let id = params[key]
                .as_str()
                .unwrap_or_else(|| panic!("{} lacks path selector {key}", op.method));
            let encoded =
                percent_encoding::utf8_percent_encode(id, percent_encoding::NON_ALPHANUMERIC)
                    .to_string();
            path = path.replace(&format!("{{{key}}}"), &encoded);
            body.as_object_mut().unwrap().remove(key);
        }
        let route = resolve(&op.verb, &path, None, body).unwrap();
        assert_eq!(route.params, params, "{}", op.method);
    }
}

#[test]
fn six_legacy_resources_have_explicit_placement_and_preserve_declared_input_objects() {
    // These are route transport tests, not a substitute for the old handlers'
    // permissive/domain validation or #908's future shared Invocation seam.
    for (method, verb, path, params) in [
        (
            "agent.start",
            "POST",
            "/v1/agent-starts",
            json!({"id":"run-7","agent_id":"a","future":null}),
        ),
        (
            "agent.interrupt",
            "POST",
            "/v1/agent-interruptions",
            json!({"entity_id":"run-7","agent_id":"a","conversation_id":"c","future":true}),
        ),
        ("bridge.stats", "GET", "/v1/bridge/diagnostics", json!({})),
        ("ping", "GET", "/v1/health", json!({})),
        (
            "session.hello",
            "POST",
            "/v1/session-greetings",
            json!({"client":{"name":7},"future":[1,2]}),
        ),
        (
            "term.list",
            "POST",
            "/v1/terminal-roster-queries",
            json!({"project_id":"proj-1","future":"retained"}),
        ),
    ] {
        let route = resolve(verb, path, None, params.clone()).unwrap();
        assert_eq!(route.method, method);
        assert_eq!(route.params, params, "{method}");
        assert!(
            matches!(
                resolve(verb, path, None, json!([])),
                Err(RouteError::InvalidParams { .. })
            ),
            "{method}"
        );
    }
}

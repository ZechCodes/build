use super::*;
use crate::{app::AppState, http::policy::HttpPolicy};
use axum::http::header::{ACCESS_CONTROL_ALLOW_METHODS, ACCESS_CONTROL_ALLOW_ORIGIN};
use serde_json::Value;
use std::{collections::BTreeSet, time::Duration};

const APP_ORIGIN: &str = "https://app.example";
const REQUEST_HEADERS: &str =
    "authorization,content-type,x-build-priority,x-build-request-id,x-build-session,x-build-subscription";

#[tokio::test]
async fn cors_preflights_match_the_http_catalog() {
    let catalog: BTreeSet<_> = include_str!("../../routes/resources.tsv")
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| line.split('\t').nth(1).unwrap())
        .collect();
    let manifest = crate::http::routes::manifest();
    assert_eq!(
        manifest
            .operations
            .iter()
            .map(|op| op.verb.as_str())
            .collect::<BTreeSet<_>>(),
        catalog
    );
    let app = HttpPolicy::new("https://bridge.example", &[APP_ORIGIN.to_owned()])
        .unwrap()
        .apply(Router::new());
    for method in &catalog {
        let response = preflight(&app, APP_ORIGIN, method).await;
        assert_eq!(response.status(), StatusCode::OK, "preflight for {method}");
        assert_eq!(response.headers()[ACCESS_CONTROL_ALLOW_ORIGIN], APP_ORIGIN);
        assert_eq!(
            allowed_methods(response.headers()),
            catalog,
            "preflight for {method}"
        );
    }
    for (origin, method) in [(APP_ORIGIN, "TRACE"), ("https://app.example.evil", "PUT")] {
        let response = preflight(&app, origin, method).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert!(!response.headers().contains_key(ACCESS_CONTROL_ALLOW_ORIGIN));
    }
}

async fn preflight(app: &Router, origin: &str, method: &str) -> Response {
    let request = Request::builder()
        .method("OPTIONS")
        .uri("/v1/entities/run-fixture/read-cursor")
        .header("host", "bridge.example")
        .header("origin", origin)
        .header("access-control-request-method", method)
        .header("access-control-request-headers", REQUEST_HEADERS)
        .body(Body::empty())
        .unwrap();
    app.clone().oneshot(request).await.unwrap()
}

fn allowed_methods(headers: &HeaderMap) -> BTreeSet<&str> {
    headers[ACCESS_CONTROL_ALLOW_METHODS]
        .to_str()
        .unwrap()
        .split(',')
        .map(str::trim)
        .collect()
}

#[tokio::test]
async fn https_view_clears_project_unread_on_a_fresh_opening() {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let (app, owner, agent) = project_with_unread(&root);
    let store = app.attached_store_for_test().unwrap();
    let execution = ExecutionService::new(AppState::handler(Arc::new(Mutex::new(app))));
    let runtime = runtime_with(
        execution,
        Arc::new(WitnessVerifier {
            calls: Arc::default(),
            authority: Some(authority()),
        }),
    );
    let (client, base, stop, server) = serve_fixture(&root, runtime).await;

    let session = open_fixture(&client, &base).await;
    let entity = format!("{base}/v1/entities/{owner}");
    let roster = client
        .post(format!("{entity}/agent-queries"))
        .json(&json!({"include_project_workers": true}));
    let before = resource(roster.try_clone().unwrap(), &session).await;
    assert_eq!(before["agents"][0]["unread_count"], 1, "{before}");
    let page = resource(
        client
            .post(format!("{entity}/message-queries"))
            .json(&json!({"agent_id": agent})),
        &session,
    )
    .await;
    let item = &page["items"][0]["data"];
    assert_eq!(item["body"], "Read this message", "{page}");
    let sequence = item["sequence"].as_u64().unwrap();

    let cursor_url = format!("{entity}/read-cursor");
    let response = client
        .request(reqwest::Method::OPTIONS, &cursor_url)
        .header("origin", APP_ORIGIN)
        .header("access-control-request-method", "PUT")
        .header("access-control-request-headers", REQUEST_HEADERS)
        .send()
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "the browser must be allowed to send the read cursor"
    );
    assert_eq!(response.headers()[ACCESS_CONTROL_ALLOW_ORIGIN], APP_ORIGIN);
    assert!(allowed_methods(response.headers()).contains("PUT"));

    resource(
        client.put(&cursor_url).json(&json!({
            "agent_id": agent, "thread_id": page["thread_id"],
            "read_from_sequence": sequence, "read_through_sequence": sequence,
        })),
        &session,
    )
    .await;
    let immediately = resource(roster.try_clone().unwrap(), &session).await;
    assert_eq!(immediately["agents"][0]["unread_count"], 0, "{immediately}");

    let fresh = open_fixture(&client, &base).await;
    assert_ne!(session, fresh);
    let after = resource(roster, &fresh).await;
    assert_eq!(after["agents"][0]["unread_count"], 0, "{after}");
    assert_eq!(after["agents"][0]["read_through_sequence"], sequence);
    assert_eq!(store.load_attention()[&owner].cursor_for(&agent), sequence);

    stop.send(true).unwrap();
    server.await.unwrap().unwrap();
}

fn project_with_unread(root: &std::path::Path) -> (AppState, String, String) {
    let repo = crate::git_fixture::init_repo_named(root, "repo");
    let context =
        crate::harness::HarnessContext::resolved(root.join("mcp.sock"), root.to_owned()).unwrap();
    let mut app = AppState::new_unrooted_configured(root.join("worktrees"), "main", true, context)
        .with_task_store(root.join("store"))
        .unwrap();
    let project = app.add_project(repo, "main".into());
    let ensured = app
        .project_ensure_conversation(&json!({"project_id": project}))
        .unwrap();
    let owner = ensured["entity_id"].as_str().unwrap().to_owned();
    let roster = app
        .agent_list(&json!({"entity_id": owner, "include_project_workers": true}))
        .unwrap();
    let agent = roster["pinned_agent_id"].as_str().unwrap().to_owned();
    app.post_agent_message_for_test(&owner, &agent, "Read this message");
    (app, owner, agent)
}

async fn serve_fixture(
    root: &std::path::Path,
    runtime: HttpRuntime,
) -> (
    reqwest::Client,
    String,
    tokio::sync::watch::Sender<bool>,
    tokio::task::JoinHandle<std::io::Result<()>>,
) {
    let rcgen::CertifiedKey { cert, key_pair } =
        rcgen::generate_simple_self_signed(["localhost".to_owned()]).unwrap();
    let cert_path = root.join("cert.pem");
    let key_path = root.join("key.pem");
    std::fs::write(&cert_path, cert.pem()).unwrap();
    std::fs::write(&key_path, key_pair.serialize_pem()).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let tls = crate::http::tls::load(&cert_path, &key_path).unwrap();
    let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = socket.local_addr().unwrap();
    let base = format!("https://localhost:{}", address.port());
    let policy = HttpPolicy::new(&base, &[APP_ORIGIN.to_owned()]).unwrap();
    let (stop, stopped) = tokio::sync::watch::channel(false);
    let server = tokio::spawn(serve_mounted(socket, tls, policy, runtime, stopped));
    let client = reqwest::Client::builder()
        .no_proxy()
        .resolve("localhost", address)
        .timeout(Duration::from_secs(5))
        .add_root_certificate(reqwest::Certificate::from_pem(cert.pem().as_bytes()).unwrap())
        .build()
        .unwrap();
    (client, base, stop, server)
}

async fn open_fixture(client: &reqwest::Client, base: &str) -> String {
    let response = client
        .post(format!("{base}/v1/sessions"))
        .header("origin", APP_ORIGIN)
        .bearer_auth("account-bound-token")
        .json(&json!({"api_range": ">=3 <4"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    response.json::<Value>().await.unwrap()["session_id"]
        .as_str()
        .unwrap()
        .to_owned()
}

async fn resource(request: reqwest::RequestBuilder, session: &str) -> Value {
    let response = request
        .header("origin", APP_ORIGIN)
        .header("x-build-session", session)
        .bearer_auth("account-bound-token")
        .send()
        .await
        .unwrap();
    let status = response.status();
    assert_eq!(response.headers()[ACCESS_CONTROL_ALLOW_ORIGIN], APP_ORIGIN);
    let body = response.json::<Value>().await.unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["ok"], true, "{body}");
    body["result"].clone()
}

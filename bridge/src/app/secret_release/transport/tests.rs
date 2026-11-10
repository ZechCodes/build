use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct Fixture(AtomicUsize);
impl Backend for Fixture {
    fn release(&self, request: &Request) -> crate::secrets::Result<crate::secrets::SecretValue> {
        self.0.fetch_add(1, Ordering::SeqCst);
        if request.capability != "synthetic-capability" {
            return Err(crate::secrets::SecretError::Unavailable);
        }
        crate::secrets::SecretValue::new(b"synthetic-value\nwith-newline".to_vec())
    }
}
fn request() -> Vec<u8> {
    b"{\"agent_id\":\"synthetic-agent\",\"capability\":\"synthetic-capability\",\"secret_id\":\"sec_00000000-0000-4000-8000-000000000001\",\"request_id\":\"00000000-0000-4000-8000-000000000002\"}\n".to_vec()
}
async fn exchange(bytes: Vec<u8>) -> (Vec<u8>, usize) {
    let backend = Arc::new(Fixture(AtomicUsize::new(0)));
    let (server, mut client) = tokio::net::UnixStream::pair().unwrap();
    let plane = Arc::new(Plane::new(backend.clone()));
    let task = tokio::spawn(handle(server, plane));
    client.write_all(&bytes).await.unwrap();
    client.shutdown().await.unwrap();
    let mut response = Vec::new();
    client.read_to_end(&mut response).await.unwrap();
    task.await.unwrap();
    (response, backend.0.load(Ordering::SeqCst))
}
#[tokio::test]
async fn exact_raw_bytes_follow_bounded_length_header() {
    let (response, calls) = exchange(request()).await;
    let split = response.iter().position(|byte| *byte == b'\n').unwrap();
    let header: serde_json::Value = serde_json::from_slice(&response[..split]).unwrap();
    assert_eq!(header, serde_json::json!({"ok":true,"length":28}));
    assert_eq!(&response[split + 1..], b"synthetic-value\nwith-newline");
    assert_eq!(calls, 1);
}
#[tokio::test]
async fn spoofed_selectors_malformed_uuid_and_oversize_never_reach_backend() {
    for bytes in [
        [
            request()[..request().len() - 2].to_vec(),
            b",\"project_id\":\"spoofed\"}\n".to_vec(),
        ]
        .concat(),
        b"{\"request_id\":\"bogus\"}\n".to_vec(),
        vec![b'x'; MAX_FRAME_BYTES + 1],
        b"{bad-json}\n".to_vec(),
    ] {
        let (response, calls) = exchange(bytes).await;
        assert_eq!(calls, 0);
        assert!(!response.windows(9).any(|part| part == b"synthetic"));
        assert_eq!(response, b"{\"ok\":false,\"error\":\"unauthorized\"}\n");
    }
}
#[tokio::test]
async fn cwd_workspace_and_owner_selectors_cannot_grant_authority() {
    for (key, value) in [
        ("cwd", "/synthetic-other-project"),
        ("workspace_id", "synthetic-workspace"),
        ("owner_id", "synthetic-owner"),
    ] {
        let mut frame: serde_json::Value = serde_json::from_slice(&request()).unwrap();
        frame[key] = serde_json::Value::String(value.into());
        let (response, calls) = exchange(format!("{frame}\n").into_bytes()).await;
        assert_eq!(calls, 0);
        assert_eq!(response, b"{\"ok\":false,\"error\":\"unauthorized\"}\n");
    }
}

#[tokio::test]
async fn one_connection_cannot_pipeline_two_releases() {
    let (response, calls) = exchange([request(), request()].concat()).await;
    assert_eq!(calls, 1);
    assert_eq!(response.iter().filter(|byte| **byte == b'{').count(), 1);
}
#[test]
fn rate_limits_bound_total_and_per_capability_without_unbounded_state() {
    let mut limiter = RateLimits::default();
    let now = std::time::Instant::now();
    for _ in 0..PER_CAPABILITY_PER_SECOND {
        assert!(limiter.admit("synthetic-capability", now));
    }
    assert!(!limiter.admit("synthetic-capability", now));
    for id in 0..GLOBAL_PER_SECOND {
        limiter.admit(&format!("synthetic-{id}"), now);
    }
    assert!(limiter.buckets.len() <= GLOBAL_PER_SECOND as usize);
    assert!(!limiter.admit("another", now));
    assert!(limiter.admit("synthetic-capability", now + Duration::from_secs(1)));
}
#[tokio::test]
async fn idle_streams_cannot_exceed_connection_bound() {
    let plane = Arc::new(Plane::new(Arc::new(Fixture(AtomicUsize::new(0)))));
    let mut tasks = Vec::new();
    let mut clients = Vec::new();
    for _ in 0..MAX_CONNECTIONS {
        let (server, client) = tokio::net::UnixStream::pair().unwrap();
        clients.push(client);
        tasks.push(tokio::spawn(handle(server, plane.clone())));
    }
    tokio::time::timeout(Duration::from_secs(2), async {
        while plane.connections.available_permits() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let (server, mut extra) = tokio::net::UnixStream::pair().unwrap();
    handle(server, plane.clone()).await;
    let mut byte = [0];
    assert_eq!(extra.read(&mut byte).await.unwrap(), 0);
    for task in tasks {
        task.abort();
        let _ = task.await;
    }
    assert_eq!(plane.connections.available_permits(), MAX_CONNECTIONS);
    drop(clients);
}

struct AuditedBackend {
    service: crate::secrets::SecretService,
    caller: crate::secrets::SecretCaller,
    released: tokio::sync::Notify,
}
impl Backend for AuditedBackend {
    fn release(&self, request: &Request) -> crate::secrets::Result<crate::secrets::SecretValue> {
        let result = self.service.release(
            &self.caller,
            &request.secret_id,
            &request.request_id,
            || true,
        );
        self.released.notify_one();
        result
    }
}
fn audited_fixture() -> (
    tempfile::TempDir,
    crate::store::Store,
    Arc<AuditedBackend>,
    String,
) {
    use crate::secrets::*;
    use crate::store::Store;
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().join("tasks")).unwrap();
    let agent = crate::agent::Agent::new(
        "synthetic-agent",
        "synthetic-project",
        crate::models::ModelChoice::default(),
        1,
        "2026-11-04T00:00:00Z",
    );
    store
        .writer_for_test()
        .execute(
            "INSERT INTO agents(id,owner_id,ordinal,record) VALUES (?1,?2,1,?3)",
            rusqlite::params![
                agent.id,
                agent.owner_id,
                serde_json::to_string(&agent).unwrap()
            ],
        )
        .unwrap();
    store.writer_for_test().execute_batch("INSERT INTO device_project_ids(path,project_id) VALUES ('/synthetic-project','synthetic-project'); INSERT INTO device_owner_projects(owner_id,project_path) VALUES ('synthetic-project','/synthetic-project')").unwrap();
    let caller = SecretCaller {
        device_id: "synthetic-device".into(),
        channel: "dev".into(),
        project_id: "synthetic-project".into(),
        owner_id: agent.owner_id.clone(),
        canonical_owner_id: agent.owner_id.clone(),
        agent_id: agent.id.clone(),
        conversation_id: agent.id.clone(),
        thread_id: agent.thread.id.clone(),
        generation: agent.thread.id.clone(),
        generation_revision: agent.thread.generation_revision,
        session_incarnation: "synthetic-session".into(),
    };
    crate::store::secrets::seed_current_project_for_test(&store, "synthetic-project");
    let service = SecretService::open(dir.path(), "synthetic-device", store.writer()).unwrap();
    let record = service
        .stage(
            SecretContext {
                id: format!("sec_{}", uuid::Uuid::new_v4()),
                device_id: caller.device_id.clone(),
                creator: "synthetic-user".into(),
                name: "SYNTHETIC_TOKEN".into(),
                ttl_seconds: None,
                grant: GrantScope::Project {
                    project_id: caller.project_id.clone(),
                },
                stage: StageContext {
                    owner_id: caller.owner_id.clone(),
                    conversation_id: caller.conversation_id.clone(),
                    generation: caller.generation.clone(),
                    draft_id: None,
                },
            },
            SecretValue::new(vec![b'x'; MAX_VALUE_BYTES]).unwrap(),
            uuid::Uuid::new_v4().to_string(),
            "a".repeat(64),
        )
        .unwrap();
    service
        .attach(std::slice::from_ref(&record.context.id), "synthetic-send")
        .unwrap();
    (
        dir,
        store,
        Arc::new(AuditedBackend {
            service,
            caller,
            released: tokio::sync::Notify::new(),
        }),
        record.context.id,
    )
}
fn constrain_send_buffer(stream: &tokio::net::UnixStream) {
    use std::os::fd::AsRawFd;
    let size: libc::c_int = 1024;
    // SAFETY: the fd remains live and the pointer/length describe a c_int.
    let result = unsafe {
        libc::setsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_SNDBUF,
            (&size as *const libc::c_int).cast(),
            std::mem::size_of_val(&size) as libc::socklen_t,
        )
    };
    assert_eq!(result, 0);
}
#[tokio::test]
async fn stalled_client_cannot_keep_audit_or_membership_writer_lock() {
    let (_dir, store, backend, id) = audited_fixture();
    let (server, mut client) = tokio::net::UnixStream::pair().unwrap();
    constrain_send_buffer(&server);
    let plane = Arc::new(Plane::new(backend.clone()));
    let mut task = tokio::spawn(handle(server, plane));
    let frame = serde_json::json!({"agent_id":backend.caller.agent_id,"capability":"synthetic-capability","secret_id":id,"request_id":uuid::Uuid::new_v4().to_string()});
    client
        .write_all(format!("{frame}\n").as_bytes())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), backend.released.notified())
        .await
        .unwrap();
    // The client reads nothing; a 16-KiB reply cannot fit in the small buffer.
    assert!(tokio::time::timeout(Duration::from_millis(50), &mut task)
        .await
        .is_err());
    let writer = store.writer();
    let saved_id = id.clone();
    let write = tokio::task::spawn_blocking(move || {
        assert_eq!(
            writer
                .secret_audit_summary(&saved_id)
                .unwrap()
                .unwrap()
                .release_count,
            1
        );
        writer.secrets_audit_maintain(0).unwrap();
    });
    tokio::time::timeout(Duration::from_secs(2), write)
        .await
        .expect("writer must be free while client stalls")
        .unwrap();
    let mut response = Vec::new();
    client.read_to_end(&mut response).await.unwrap();
    task.await.unwrap();
    assert!(response.starts_with(b"{\"ok\":true,\"length\":16384}\n"));
    assert_eq!(response.iter().filter(|b| **b == b'x').count(), 16384);
}

#[tokio::test]
async fn failed_audit_sends_only_fixed_error_without_value_or_request() {
    let (_dir, store, backend, id) = audited_fixture();
    store.writer_for_test().execute_batch("CREATE TRIGGER synthetic_fail_audit BEFORE INSERT ON secret_audit BEGIN SELECT RAISE(ABORT,'synthetic failure'); END").unwrap();
    let (server, mut client) = tokio::net::UnixStream::pair().unwrap();
    let task = tokio::spawn(handle(server, Arc::new(Plane::new(backend))));
    let frame = serde_json::json!({"agent_id":"synthetic-agent","capability":"synthetic-capability","secret_id":id,"request_id":uuid::Uuid::new_v4().to_string()});
    client
        .write_all(format!("{frame}\n").as_bytes())
        .await
        .unwrap();
    let mut response = Vec::new();
    client.read_to_end(&mut response).await.unwrap();
    task.await.unwrap();
    assert_eq!(
        response,
        b"{\"ok\":false,\"error\":\"secret_unavailable\"}\n"
    );
}

#[tokio::test]
async fn idle_pre_auth_clients_release_all_slots_within_one_second() {
    let plane = Arc::new(Plane::new(Arc::new(Fixture(AtomicUsize::new(0)))));
    let mut clients = Vec::new();
    let mut tasks = Vec::new();
    for _ in 0..MAX_CONNECTIONS {
        let (server, client) = UnixStream::pair().unwrap();
        clients.push(client);
        tasks.push(tokio::spawn(handle(server, plane.clone())));
    }
    tokio::time::timeout(Duration::from_secs(1), async {
        for task in tasks {
            task.await.unwrap();
        }
    })
    .await
    .expect("idle pre-auth streams must not monopolize release capacity");
    assert_eq!(plane.connections.available_permits(), MAX_CONNECTIONS);
    drop(clients);
}

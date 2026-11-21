use super::*;

fn real_app() -> (tempfile::TempDir, FrameHandler, String) {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let repo = crate::git_fixture::init_repo_named(&root, "repo");
    let harness =
        crate::harness::HarnessContext::resolved(root.join("mcp.sock"), root.clone()).unwrap();
    let mut app = crate::app::AppState::new_unrooted_configured(
        root.join("worktrees"),
        "main",
        true,
        harness,
    );
    let project = app.add_project(repo, "main".into());
    let handler = crate::app::AppState::handler(Arc::new(Mutex::new(app)));
    (dir, handler, project)
}

#[tokio::test]
async fn mounted_tls_effects_require_an_attachment_on_this_socket() {
    let (_scratch, handler, project) = real_app();
    let server = Server::start(handler).await;
    let opening = server.opening().await;
    let (application, pushes) = application_binding(&server, &opening);
    let mut first = server.socket(&server.ticket(&opening).await).await;
    let created = call(
        &mut first,
        "create",
        "term.create",
        json!({"project_id":project}),
    )
    .await;
    assert_eq!(created["ok"], true, "{created:?}");
    let term = case_text(&created["result"]["term_id"]);
    let mut second = server.socket(&server.ticket(&opening).await).await;
    assert_closed(&mut first, None).await;
    for (method, params) in [
        ("term.input", json!({"term_id":term,"data":"YQ=="})),
        ("term.resize", json!({"term_id":term,"cols":100,"rows":30})),
        ("term.ack", json!({"term_id":term,"cursor":0})),
    ] {
        let reply = call(
            &mut second,
            method.replace('.', "-").as_str(),
            method,
            params,
        )
        .await;
        assert_eq!(reply["error_code"], "invalid_params", "{method}: {reply:?}");
        assert_eq!(reply["error"], "Terminal is not attached");
    }
    exercise_shell(&mut second, term, &project).await;
    assert_eq!(pushes.load(Ordering::Relaxed), 0);
    application
        .events()
        .push(json!({"type":"application.changed"}))
        .unwrap();
    assert_eq!(pushes.load(Ordering::Relaxed), 1);
    server.stop().await;
}

async fn exercise_shell(socket: &mut Client, term: &str, project: &str) {
    let attached = call(
        socket,
        "attach",
        "term.attach",
        json!({"term_id":term,"project_id":project}),
    )
    .await;
    assert_eq!(attached["ok"], true, "{attached:?}");
    assert_eq!(attached["result"]["cols"], 80);
    assert_eq!(attached["result"]["rows"], 24);
    let resized = call(
        socket,
        "resize",
        "term.resize",
        json!({"term_id":term,"cols":100,"rows":30}),
    )
    .await;
    assert_eq!(resized["ok"], true, "{resized:?}");
    send(
        socket,
        "input",
        "term.input",
        json!({"term_id":term,
        "data":STANDARD.encode(b"printf '%s%s\\n' exact-binding -output\n")}),
    )
    .await;
    let cursor = wait_for_shell_output(socket, term).await;
    let acked = call(
        socket,
        "ack",
        "term.ack",
        json!({"term_id":term,"cursor":cursor}),
    )
    .await;
    assert_eq!(acked["ok"], true, "{acked:?}");
    close_shell(socket, term, project).await;
}

async fn close_shell(socket: &mut Client, term: &str, project: &str) {
    send(
        socket,
        "close",
        "term.close",
        json!({"term_id":term,"project_id":project}),
    )
    .await;
    let mut closed = false;
    let mut replied = false;
    while !closed || !replied {
        let value = object(socket).await;
        if value["type"] == "term.closed" {
            assert_eq!(value["term_id"], term);
            assert_eq!(value["reason"], "closed");
            closed = true;
        }
        if value["id"] == "close" {
            assert_eq!(value["ok"], true, "{value:?}");
            replied = true;
        }
    }
}

async fn wait_for_shell_output(socket: &mut Client, term: &str) -> u64 {
    timeout(TIMEOUT, async {
        let mut bytes = Vec::new();
        let mut replied = false;
        let mut cursor = 0;
        loop {
            let value = object(socket).await;
            if value["id"] == "input" {
                assert_eq!(value["ok"], true, "{value:?}");
                replied = true;
            }
            if value["type"] == "term.output" {
                assert_eq!(value["term_id"], term);
                bytes.extend(STANDARD.decode(case_text(&value["data"])).unwrap());
                cursor = value["cursor"].as_u64().unwrap();
            }
            if replied && String::from_utf8_lossy(&bytes).contains("exact-binding-output") {
                return cursor;
            }
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn mounted_tls_agent_rekey_preserves_detach_handle_and_sibling() {
    let waiting = crate::screen::ScreenHandle::new("agent:waiting", 120, 40);
    let born = crate::screen::ScreenHandle::new("agent:born", 120, 40);
    let shell = crate::screen::ScreenHandle::new("shell", 80, 24);
    let handler = greeting(FrameHandler::new(FrameClock::new(), {
        let waiting = waiting.clone();
        let shell = shell.clone();
        move |sender, frame, _| match frame.payload["method"].as_str().unwrap() {
            "agent.attach" => {
                waiting.attach(&sender.for_terminal_screen(), None);
                success(
                    &frame,
                    json!({"term_id":"agent:waiting","snapshot":"","live":false,"provider":null}),
                )
            }
            "term.attach" => {
                shell.attach(&sender.for_terminal_screen(), None);
                success(
                    &frame,
                    json!({"term_id":"shell","snapshot":"","live":true,"provider":null}),
                )
            }
            "term.input" => success(&frame, json!({})),
            other => panic!("unexpected {other}"),
        }
    }));
    let server = Server::start(handler).await;
    let opening = server.opening().await;
    let mut socket = server.socket(&server.ticket(&opening).await).await;
    let reply = call(&mut socket, "agent", "agent.attach", json!({})).await;
    assert_eq!(reply["result"]["term_id"], "agent:waiting");
    assert_eq!(
        call(
            &mut socket,
            "shell",
            "term.attach",
            json!({"term_id":"shell"})
        )
        .await["ok"],
        true
    );
    assert!(born.carry_clients_from(&waiting));
    born.restart();
    let reset = object(&mut socket).await;
    assert_eq!(reset["type"], "term.reset");
    assert_eq!(reset["term_id"], "agent:born");
    let input = |id: &str| json!({"term_id":id,"data":"YQ=="});
    assert_eq!(
        call(&mut socket, "stale", "term.input", input("agent:waiting")).await["error_code"],
        "invalid_params"
    );
    assert_eq!(
        call(&mut socket, "rekey", "term.input", input("agent:born")).await["ok"],
        true
    );
    detach(&mut socket, "agent").await;
    detach(&mut socket, "agent").await;
    assert_eq!(
        call(&mut socket, "detached", "term.input", input("agent:born")).await["error_code"],
        "invalid_params"
    );
    assert!(born.feed(b"detached"));
    born.flush();
    assert!(shell.feed(b"sibling"));
    shell.flush();
    let output = object(&mut socket).await;
    assert_eq!(output["term_id"], "shell");
    detach(&mut socket, "shell").await;
    assert_closed(&mut socket, Some(1000)).await;
    server.stop().await;
}

async fn detach(socket: &mut Client, attach: &str) {
    socket
        .send(Message::Binary(
            json!({"type":"detach","attach_id":attach})
                .to_string()
                .into_bytes(),
        ))
        .await
        .unwrap();
}

#[tokio::test]
async fn mounted_tls_replacement_fences_queued_effects_and_keeps_application() {
    let began = Arc::new(Notify::new());
    let (release, blocked) = std::sync::mpsc::channel();
    let effects = Arc::new(Mutex::new(Vec::new()));
    let handler = blocking_input_handler(began.clone(), blocked, effects.clone());
    let server = Server::start(handler).await;
    let opening = server.opening().await;
    let (application, pushes) = application_binding(&server, &opening);
    let mut first = server.socket(&server.ticket(&opening).await).await;
    assert_attached(&mut first).await;
    let input = json!({"term_id":"shell","data":"YQ=="});
    send(&mut first, "running", "term.input", input.clone()).await;
    timeout(TIMEOUT, began.notified()).await.unwrap();
    send(&mut first, "queued", "term.input", input.clone()).await;
    assert_eq!(
        call(&mut first, "barrier", "ping", json!({})).await["ok"],
        true
    );

    let mut second = server.socket(&server.ticket(&opening).await).await;
    assert_closed(&mut first, None).await;
    release.send(()).unwrap();
    assert_attached(&mut second).await;
    assert_eq!(
        call(&mut second, "successor", "term.input", input).await["ok"],
        true
    );
    assert_eq!(&*effects.lock().unwrap(), &["running", "successor"]);
    assert!(application.is_live());
    application
        .events()
        .push(json!({"type":"application.changed"}))
        .unwrap();
    assert_eq!(pushes.load(Ordering::Relaxed), 1);
    server.stop().await;
}

fn blocking_input_handler(
    began: Arc<Notify>,
    blocked: std::sync::mpsc::Receiver<()>,
    effects: Arc<Mutex<Vec<String>>>,
) -> FrameHandler {
    let blocked = Mutex::new(blocked);
    greeting(FrameHandler::new(
        FrameClock::new(),
        move |_, frame, _| match frame.payload["method"].as_str().unwrap() {
            "term.attach" => success(&frame, json!({"term_id":"shell","snapshot":""})),
            "term.input" => {
                let id = case_text(&frame.payload["id"]);
                effects.lock().unwrap().push(id.into());
                if id == "running" {
                    began.notify_one();
                    blocked.lock().unwrap().recv_timeout(TIMEOUT).unwrap();
                }
                success(&frame, json!({}))
            }
            other => panic!("unexpected {other}"),
        },
    ))
}

async fn assert_attached(socket: &mut Client) {
    let reply = call(socket, "attach", "term.attach", json!({"term_id":"shell"})).await;
    assert_eq!(reply["result"]["term_id"], "shell", "{reply:?}");
}

#[tokio::test]
async fn mounted_tls_outstanding_calls_are_bounded_and_duplicate_ids_close() {
    let began = Arc::new(Notify::new());
    let (release, blocked) = std::sync::mpsc::channel();
    let effects = Arc::new(Mutex::new(Vec::new()));
    let server = Server::start(blocking_input_handler(began.clone(), blocked, effects)).await;
    let opening = server.opening().await;
    let mut socket = server.socket(&server.ticket(&opening).await).await;
    assert_attached(&mut socket).await;
    let input = json!({"term_id":"shell","data":"YQ=="});
    send(&mut socket, "running", "term.input", input.clone()).await;
    timeout(TIMEOUT, began.notified()).await.unwrap();
    let cap = fixture()["limits"]["outstanding_calls"]["socket"]
        .as_u64()
        .unwrap();
    for offset in 1..cap {
        send(
            &mut socket,
            &format!("queued-{offset}"),
            "term.input",
            input.clone(),
        )
        .await;
    }
    let busy = call(&mut socket, "ping-2", "ping", json!({})).await;
    let mut expected: Value =
        serde_json::from_str(case_text(&fixture()["messages"]["busy"]["utf8"])).unwrap();
    expected["bridge_epoch"] = busy["bridge_epoch"].clone();
    assert_eq!(busy, expected);
    send(&mut socket, "running", "term.input", input).await;
    assert_closed(&mut socket, Some(1002)).await;
    release.send(()).unwrap();
    server.stop().await;
}

#[tokio::test]
async fn mounted_tls_detach_cancels_a_pending_attach() {
    let waiting = crate::screen::ScreenHandle::new("agent:waiting", 120, 40);
    let began = Arc::new(Notify::new());
    let finished = Arc::new(Notify::new());
    let (release, blocked) = std::sync::mpsc::channel();
    let handler = pending_agent_attach(waiting.clone(), began.clone(), finished.clone(), blocked);
    let server = Server::start(handler).await;
    let opening = server.opening().await;
    let mut socket = server.socket(&server.ticket(&opening).await).await;
    assert_attached(&mut socket).await;

    send(&mut socket, "agent", "agent.attach", json!({})).await;
    timeout(TIMEOUT, began.notified()).await.unwrap();
    detach(&mut socket, "agent").await;
    assert_eq!(
        call(&mut socket, "barrier", "ping", json!({})).await["ok"],
        true
    );
    release.send(()).unwrap();
    timeout(TIMEOUT, finished.notified()).await.unwrap();
    assert_eq!(waiting.attached(), 0);
    send(&mut socket, "after", "ping", json!({})).await;
    assert_eq!(object(&mut socket).await["id"], "after");
    detach(&mut socket, "attach").await;
    assert_closed(&mut socket, Some(1000)).await;
    server.stop().await;
}

fn pending_agent_attach(
    waiting: crate::screen::ScreenHandle,
    began: Arc<Notify>,
    finished: Arc<Notify>,
    blocked: std::sync::mpsc::Receiver<()>,
) -> FrameHandler {
    let blocked = Mutex::new(blocked);
    greeting(FrameHandler::new(
        FrameClock::new(),
        move |sender, frame, _| match frame.payload["method"].as_str().unwrap() {
            "term.attach" => success(&frame, json!({"term_id":"shell","snapshot":""})),
            "agent.attach" => {
                began.notify_one();
                blocked.lock().unwrap().recv_timeout(TIMEOUT).unwrap();
                waiting.attach(&sender.for_terminal_screen(), None);
                finished.notify_one();
                success(&frame, json!({"term_id":"agent:waiting","snapshot":""}))
            }
            other => panic!("unexpected {other}"),
        },
    ))
}

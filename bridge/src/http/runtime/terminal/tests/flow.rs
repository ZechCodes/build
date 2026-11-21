use super::*;
use crate::screen::ScreenHandle;

#[tokio::test]
async fn mounted_tls_delayed_ack_pauses_only_its_pane() {
    let hidden = ScreenHandle::new("hidden", 80, 24);
    let sibling = ScreenHandle::new("sibling", 80, 24);
    let server = Server::start(screen_handler(vec![
        ("hidden", hidden.clone()),
        ("sibling", sibling),
    ]))
    .await;
    let opening = server.opening().await;
    let (application, app_pushes) = application_binding(&server, &opening);
    let mut socket = server.socket(&server.ticket(&opening).await).await;
    attach(&mut socket, "attach-hidden", "hidden", 80, 24).await;
    attach(&mut socket, "attach-sibling", "sibling", 80, 24).await;
    let (rtc, rtc_pushes) = rtc_client();
    hidden.attach(&rtc, None);

    hidden.feed(b"first");
    hidden.flush();
    let first = object(&mut socket).await;
    assert_eq!(first["type"], "term.output");
    assert_eq!(first["cursor"], 5);
    let flood = vec![b'q'; 1024 * 1024 + 1];
    hidden.feed(&flood);
    // Between feed and flush, the old RTC policy credits the live model
    // cursor even though the last delivered cursor is only five.
    hidden.ack(rtc.session_id(), u64::MAX);
    let refused = call(
        &mut socket,
        "over-ack",
        "term.ack",
        json!({"term_id":"hidden","cursor":5 + flood.len()}),
    )
    .await;
    assert_eq!(refused["error_code"], "invalid_params");
    hidden.flush();
    {
        let rtc_events = rtc_pushes.lock().unwrap();
        assert_eq!(rtc_events[0]["cursor"], 5);
        assert_eq!(rtc_events[1]["type"], "term.reset");
        assert_eq!(rtc_events[1]["cursor"], 5 + flood.len());
    }

    // A paused producer has no write pending. Advancing only the Tokio clock
    // here crosses the real physical-stall timeout without delaying the suite.
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(11)).await;
    tokio::time::resume();

    application
        .events()
        .push(json!({"type":"app.changed"}))
        .unwrap();
    assert_eq!(app_pushes.load(Ordering::Relaxed), 1);
    assert_echo(&mut socket, "sibling", b"x").await;
    send(
        &mut socket,
        "applied",
        "term.ack",
        json!({"term_id":"hidden","cursor":5}),
    )
    .await;
    let reset = take_reset(&mut socket, "hidden", Some("applied")).await;
    assert_eq!(reset["cursor"], 5 + flood.len());
    assert_echo(&mut socket, "hidden", b"y").await;
    assert_echo(&mut socket, "sibling", b"z").await;
    hidden.feed(&flood);
    hidden.flush();
    hidden.close("closed");
    let closed = object(&mut socket).await;
    assert_eq!(closed["type"], "term.closed");
    assert_eq!(closed["term_id"], "hidden");
    assert_echo(&mut socket, "sibling", b"w").await;
    server.stop().await;
}

#[tokio::test]
async fn mounted_tls_wss_credit_applies_before_the_first_ack() {
    let screen = ScreenHandle::new("credit", 80, 24);
    let (rtc, rtc_pushes) = rtc_client();
    screen.attach(&rtc, None);
    let server = Server::start(screen_handler(vec![("credit", screen.clone())])).await;
    let opening = server.opening().await;
    let mut socket = server.socket(&server.ticket(&opening).await).await;
    attach(&mut socket, "credit", "credit", 80, 24).await;

    let chunk = vec![b'C'; 64 * 1024];
    for index in 1..=16 {
        screen.feed(&chunk);
        screen.flush();
        let output = object(&mut socket).await;
        assert_eq!(output["type"], "term.output");
        assert_eq!(output["cursor"], index * chunk.len());
    }

    screen.feed(&chunk);
    screen.flush();
    assert_eq!(
        rtc_pushes.lock().unwrap().len(),
        17,
        "legacy RTC never ACKed"
    );
    send(&mut socket, "ping", "ping", json!({})).await;
    assert_eq!(object(&mut socket).await["id"], "ping");

    send(
        &mut socket,
        "resume",
        "term.ack",
        json!({"term_id":"credit","cursor":16 * chunk.len()}),
    )
    .await;
    let reset = take_reset(&mut socket, "credit", Some("resume")).await;
    assert_eq!(reset["cursor"], 17 * chunk.len());
    server.stop().await;
}

#[tokio::test]
async fn mounted_tls_maximal_snapshot_recovers_on_the_same_socket() {
    let recipe = &fixture()["recovery"];
    let cols = recipe["cols"].as_u64().unwrap() as u16;
    let rows = recipe["rows"].as_u64().unwrap() as u16;
    let screen = ScreenHandle::new("painted", cols, rows);
    let mut paint = Paint::new(recipe, cols, rows);
    screen.feed(&paint.bytes);
    screen.flush();

    let server = Server::start(screen_handler(vec![("painted", screen.clone())])).await;
    let opening = server.opening().await;
    let mut socket = server.socket(&server.ticket(&opening).await).await;
    let attached = attach(&mut socket, "attach", "painted", cols, rows).await;
    let mut emulator = paint.verify(
        &attached,
        &attached["result"]["snapshot"],
        &attached["result"],
    );
    assert_eq!(paint.cursor, paint.bytes.len() as u64);
    assert_eq!(
        call(
            &mut socket,
            "baseline",
            "term.ack",
            json!({"term_id":"painted","cursor":paint.cursor})
        )
        .await["ok"],
        true
    );

    // Overwrite the grid with a second flood, including a changed witness
    // cell. Applying the reset must reconstruct this newer screen and cursor.
    paint.bytes.extend_from_slice(b"\x1b[2;5H?\x1b[6;8H");
    screen.feed(&paint.bytes);
    screen.flush();
    paint.expected.process(&paint.bytes);
    paint.cursor += paint.bytes.len() as u64;
    let reset = take_reset(&mut socket, "painted", None).await;
    emulator.process(&STANDARD.decode(case_text(&reset["data"])).unwrap());
    paint.verify_grid(emulator.screen());
    paint.verify_size(&reset);
    assert_eq!(reset["cursor"], paint.cursor);

    let next = case_text(&recipe["next_output"]).as_bytes();
    screen.feed(next);
    screen.flush();
    let output = object(&mut socket).await;
    assert_eq!(output["type"], "term.output");
    assert_eq!(output["cursor"], paint.cursor + next.len() as u64);
    assert_eq!(STANDARD.decode(case_text(&output["data"])).unwrap(), next);
    emulator.process(next);
    let (row, col) = paint.expected.screen().cursor_position();
    assert_eq!(
        emulator.screen().cell(row, col).unwrap().contents(),
        case_text(&recipe["next_output"])
    );
    let input = STANDARD
        .decode(case_text(&recipe["next_input_base64"]))
        .unwrap();
    assert_echo(&mut socket, "painted", &input).await;
    assert_eq!(server.ticket_requests.load(Ordering::Relaxed), 1);
    assert_eq!(server.upgrades.load(Ordering::Relaxed), 1);
    server.stop().await;
}

#[tokio::test]
async fn mounted_tls_rtc_resized_grid_refuses_only_the_wss_pane() {
    let screen = ScreenHandle::new("painted", 80, 24);
    let shell = ScreenHandle::new("sibling", 80, 24);
    let server = Server::start(screen_handler(vec![
        ("painted", screen.clone()),
        ("sibling", shell),
    ]))
    .await;
    let opening = server.opening().await;
    let mut socket = server.socket(&server.ticket(&opening).await).await;
    attach(&mut socket, "sibling", "sibling", 80, 24).await;
    attach(&mut socket, "live", "painted", 80, 24).await;
    let ack = call(
        &mut socket,
        "baseline",
        "term.ack",
        json!({"term_id":"painted","cursor":0}),
    )
    .await;
    assert_eq!(ack["ok"], true);

    let (rtc, pushes) = rtc_client();
    let unsupported = &fixture()["recovery"]["unsupported"]["retained_grid"];
    let size = |axis: &str| unsupported[axis].as_u64().unwrap() as u16;
    let (cols, rows) = (size("cols"), size("rows"));
    let rtc_attach = screen.attach(&rtc, Some((cols, rows)));
    assert_eq!((rtc_attach.cols, rtc_attach.rows), (cols, rows));
    screen.feed(&vec![b'R'; 128 * 1024 + 1]);
    screen.flush();

    let failed = object(&mut socket).await;
    assert_eq!(failed["type"], "attachment_error");
    assert_eq!(failed["attach_id"], "live");
    assert_eq!(failed["reason"], "snapshot_too_large");
    assert_eq!(pushes.lock().unwrap()[0]["type"], "term.reset");
    assert_eq!(screen.attached(), 1, "RTC registration survives");
    assert_eq!(
        call(
            &mut socket,
            "retired",
            "term.input",
            json!({"term_id":"painted","data":"eA=="})
        )
        .await["error_code"],
        "invalid_params"
    );

    // The handler captures the retained screen without applying the requested
    // viewport. The small request dimensions cannot bypass the actual bound.
    let refused = call(
        &mut socket,
        "retained",
        "term.attach",
        json!({"term_id":"painted","cols":80,"rows":24}),
    )
    .await;
    let expected: Value = serde_json::from_str(case_text(
        &fixture()["messages"]["oversized_attach"]["utf8"],
    ))
    .unwrap();
    for key in ["error", "error_code", "retryable"] {
        assert_eq!(refused[key], expected[key]);
    }
    assert_eq!(refused["id"], "retained");
    assert_eq!(screen.attached(), 1);
    assert_echo(&mut socket, "sibling", b"s").await;
    assert_eq!(server.ticket_requests.load(Ordering::Relaxed), 1);
    assert_eq!(server.upgrades.load(Ordering::Relaxed), 1);
    server.stop().await;
}

fn rtc_client() -> (crate::execution::CallContext, Arc<Mutex<Vec<Value>>>) {
    let pushes = Arc::new(Mutex::new(Vec::new()));
    let events = pushes.clone();
    let sender = crate::execution::CallContext::legacy(
        "rtc",
        crate::execution::session::Opening::new(),
        Arc::new(move |event| {
            events.lock().unwrap().push(event);
            true
        }),
    );
    (sender, pushes)
}

struct Paint {
    bytes: Vec<u8>,
    cursor: u64,
    expected: vt100::Parser,
    cols: u16,
    rows: u16,
}

impl Paint {
    fn new(recipe: &Value, cols: u16, rows: u16) -> Self {
        let mut bytes = case_text(&recipe["prefix"]).as_bytes().to_vec();
        for row in 0..rows {
            for col in 0..cols {
                let cell = &recipe["cells"][usize::from((row + col) % 2)];
                bytes.extend_from_slice(case_text(&cell["sgr"]).as_bytes());
                bytes.extend_from_slice(case_text(&cell["text"]).as_bytes());
            }
        }
        bytes.extend_from_slice(case_text(&recipe["suffix"]).as_bytes());
        let mut expected = vt100::Parser::new(rows, cols, 0);
        expected.process(&bytes);
        Self {
            cursor: bytes.len() as u64,
            bytes,
            expected,
            cols,
            rows,
        }
    }

    fn verify(&self, envelope: &Value, snapshot: &Value, fields: &Value) -> vt100::Parser {
        self.verify_size(envelope);
        assert_eq!(fields["cursor"], self.cursor);
        let mut emulator = vt100::Parser::new(self.rows, self.cols, 0);
        emulator.process(&STANDARD.decode(case_text(snapshot)).unwrap());
        self.verify_grid(emulator.screen());
        emulator
    }

    fn verify_size(&self, envelope: &Value) {
        let len = serde_json::to_vec(envelope).unwrap().len();
        assert!(len > 1024 * 1024 && len <= 12 * 1024 * 1024, "{len}");
    }

    fn verify_grid(&self, actual: &vt100::Screen) {
        assert_eq!(
            actual.cursor_position(),
            self.expected.screen().cursor_position()
        );
        for row in 0..self.rows {
            for col in 0..self.cols {
                assert_eq!(
                    actual.cell(row, col),
                    self.expected.screen().cell(row, col),
                    "cell {row},{col}"
                );
            }
        }
    }
}

fn screen_handler(screens: Vec<(&'static str, ScreenHandle)>) -> FrameHandler {
    greeting(FrameHandler::new(
        FrameClock::new(),
        move |sender, frame, _| {
            let params = &frame.payload["params"];
            let term = case_text(&params["term_id"]);
            let (_, screen) = screens.iter().find(|(id, _)| *id == term).unwrap();
            let result = match case_text(&frame.payload["method"]) {
                "term.attach" => {
                    let reading = match screen.attach_checked(&sender.for_terminal_screen(), None) {
                        Ok(reading) => reading,
                        Err(error) => {
                            return api::reply(
                                frame.payload["id"].clone(),
                                Err(ApiError::from(error.to_string())),
                            )
                        }
                    };
                    json!({"term_id":term,"snapshot":reading.snapshot,"cursor":reading.cursor,
                    "cols":reading.cols,"rows":reading.rows})
                }
                "term.ack" => {
                    screen.ack(
                        sender.terminal_client_id(),
                        params["cursor"].as_u64().unwrap(),
                    );
                    json!({"ok":true})
                }
                "term.input" => {
                    let input = STANDARD.decode(case_text(&params["data"])).unwrap();
                    screen.feed(&input);
                    screen.flush();
                    json!({"ok":true})
                }
                other => panic!("unexpected {other}"),
            };
            success(&frame, result)
        },
    ))
}

async fn attach(socket: &mut Client, id: &str, term: &str, cols: u16, rows: u16) -> Value {
    let reply = call(
        socket,
        id,
        "term.attach",
        json!({"term_id":term,"cols":cols,"rows":rows}),
    )
    .await;
    assert_eq!(reply["ok"], true, "{reply:?}");
    reply
}

async fn assert_echo(socket: &mut Client, term: &str, bytes: &[u8]) {
    send(
        socket,
        "echo",
        "term.input",
        json!({"term_id":term,"data":STANDARD.encode(bytes)}),
    )
    .await;
    let mut output = false;
    let mut reply = false;
    while !(output && reply) {
        let event = object(socket).await;
        if event["id"] == "echo" {
            assert_eq!(event["ok"], true, "{event:?}");
            reply = true;
        } else {
            assert_eq!(event["type"], "term.output");
            assert_eq!(event["term_id"], term);
            assert_eq!(STANDARD.decode(case_text(&event["data"])).unwrap(), bytes);
            output = true;
        }
    }
}

async fn take_reset(socket: &mut Client, term: &str, ack_id: Option<&str>) -> Value {
    let mut reset = None;
    let mut acked = ack_id.is_none();
    while reset.is_none() || !acked {
        let event = object(socket).await;
        if event["type"] == "term.reset" {
            assert_eq!(event["term_id"], term);
            assert!(reset.replace(event).is_none(), "one reset per recovery");
        } else {
            assert_eq!(event["id"].as_str(), ack_id);
            assert_eq!(event["ok"], true, "{event:?}");
            acked = true;
        }
    }
    reset.unwrap()
}

#[tokio::test]
async fn mounted_tls_detach_cancels_a_paused_panes_deferred_reset() {
    let hidden = ScreenHandle::new("hidden", 80, 24);
    let sibling = ScreenHandle::new("sibling", 80, 24);
    let server = Server::start(screen_handler(vec![
        ("hidden", hidden.clone()),
        ("sibling", sibling.clone()),
    ]))
    .await;
    let opening = server.opening().await;
    let mut socket = server.socket(&server.ticket(&opening).await).await;
    attach(&mut socket, "attach-hidden", "hidden", 80, 24).await;
    attach(&mut socket, "attach-sibling", "sibling", 80, 24).await;

    let pool = server.state.execution.sessions().terminal_snapshot_pool();
    let held: Vec<_> = (0..pool.available_permits())
        .map(|_| pool.clone().try_acquire_owned().unwrap())
        .collect();
    assert_eq!(held.len(), 2);
    let opening_slot = server.state.openings.lock().unwrap()[case_text(&opening["session_id"])]
        .session
        .opening()
        .terminal_snapshot_slot();
    hidden.feed(&vec![b'x'; 128 * 1024 + 1]);
    hidden.flush();
    let acknowledged = call(
        &mut socket,
        "ack-hidden",
        "term.ack",
        json!({"term_id":"hidden","cursor":0}),
    )
    .await;
    assert_eq!(acknowledged["ok"], true);
    let task = hidden
        .pending_wss_reset()
        .expect("paused pane has a deferred reset");
    timeout(TIMEOUT, async {
        while opening_slot.available_permits() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("reset is waiting for the bridge slot");
    assert!(!task.is_finished());

    super::dispatch::detach(&mut socket, "attach-hidden").await;
    assert_eq!(
        call(&mut socket, "after-detach", "ping", json!({})).await["ok"],
        true
    );
    hidden.flush();
    timeout(TIMEOUT, async {
        while !task.is_finished() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("detached reset task exits while the bridge slots are still held");
    assert_eq!(opening_slot.available_permits(), 1);
    assert_eq!(hidden.attached(), 0);

    drop(held);
    assert_eq!(pool.available_permits(), 2);
    assert_echo(&mut socket, "sibling", b"s").await;
    assert_eq!(sibling.attached(), 1);
    assert_eq!(server.ticket_requests.load(Ordering::Relaxed), 1);
    assert_eq!(server.upgrades.load(Ordering::Relaxed), 1);
    server.stop().await;
}

use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine;
use build_bridge::api::{reply, ApiError};
use build_bridge::execution::Invocation;
use build_bridge::screen::ScreenHandle;
use serde_json::{json, Value};
use std::path::Path;

fn fixture() -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../fixtures/api/http/terminal-websocket-v1.json");
    serde_json::from_slice(&std::fs::read(path).expect("shared WSS v1 vectors")).unwrap()
}

fn string(value: &Value) -> &str {
    value.as_str().unwrap()
}

fn number(value: &Value) -> u64 {
    value.as_u64().unwrap()
}

fn wire(case: &Value) -> Value {
    serde_json::from_str(string(&case["utf8"])).unwrap()
}

#[test]
fn binary_rpc_and_terminal_events_use_the_existing_envelopes() {
    let contract = fixture();
    let messages = &contract["messages"];
    let epoch = &contract["opening"]["base"]["greeting"]["bridge_epoch"];
    let request = wire(&messages["input"]);
    assert_eq!(
        request,
        json!({"id":"input-1","method":"term.input","params":{
        "term_id":"term-1","data":"AP9hCg=="}})
    );
    assert_eq!(
        STANDARD.decode(string(&request["params"]["data"])).unwrap(),
        [0, 255, b'a', b'\n']
    );

    let allowed = contract["wire"]["methods"].as_array().unwrap();
    for case in messages.as_object().unwrap().values() {
        let payload = wire(case);
        assert_eq!(case["opcode"], 2);
        // The shared Rust encoder serializes a serde_json::Value after stamping the epoch.
        assert_eq!(
            serde_json::to_vec(&payload).unwrap(),
            string(&case["utf8"]).as_bytes()
        );
        if case["from"] == "bridge" {
            assert_eq!(&payload["bridge_epoch"], epoch);
        }
        if let Some(method) = payload["method"].as_str() {
            assert!(allowed.contains(&json!(method)));
            Invocation::decode(method, &payload["params"]).unwrap();
        }
    }

    let expected = reply(
        json!("attach-1"),
        Ok(json!({
            "term_id":"term-1","live":true,"provider":null,"snapshot":STANDARD.encode(b"\x1b[2J\x1b[HHi"),
            "cursor":9,"cols":80,"rows":24
        })),
    );
    assert_wire(&messages["attach_ok"], epoch, expected);
    let expected = reply(
        json!("input-2"),
        Err(ApiError::invalid_params("Invalid terminal input")),
    );
    assert_wire(&messages["invalid_input"], epoch, expected);
    let expected = reply(json!("list-1"), Err(ApiError::unknown_method("term.list")));
    assert_wire(&messages["rejected_method"], epoch, expected);
    assert_wire(
        &messages["output"],
        epoch,
        json!({
            "cursor": 12, "data": STANDARD.encode(b"\r\nX"), "term_id": "term-1", "type": "term.output"
        }),
    );
    assert_wire(
        &messages["reset"],
        epoch,
        json!({
            "cursor": 12, "data": STANDARD.encode(b"\x1b[2J\x1b[HHi\r\nX"), "term_id": "term-1", "type": "term.reset"
        }),
    );
    assert_wire(
        &messages["closed"],
        epoch,
        json!({
            "term_id": "term-1", "type": "term.closed", "reason": "closed"
        }),
    );
    assert_eq!(wire(&messages["oversized_attach"])["retryable"], false);
    assert_eq!(wire(&messages["attachment_error"])["attach_id"], "attach-1");
    assert_eq!(
        wire(&messages["attachment_error"])["reason"],
        "snapshot_too_large"
    );
    assert_eq!(
        wire(&messages["detach"]),
        json!({"type":"detach","attach_id":"attach-1"})
    );
}

fn assert_wire(case: &Value, epoch: &Value, mut expected: Value) {
    expected["bridge_epoch"] = epoch.clone();
    assert_eq!(
        serde_json::to_vec(&expected).unwrap(),
        string(&case["utf8"]).as_bytes()
    );
}

fn ticket_token(token: &str) -> bool {
    token.len() == 43
        && URL_SAFE_NO_PAD
            .decode(token)
            .is_ok_and(|raw| raw.len() == 32 && URL_SAFE_NO_PAD.encode(raw) == token)
}

#[test]
fn upgrade_offers_have_one_protocol_and_one_canonical_ticket() {
    let contract = fixture();
    let handshake = &contract["handshake"];
    let protocol = string(&handshake["protocol"]);
    assert_eq!(protocol, "build.term.v1");
    assert_eq!(handshake["selected"], protocol);
    assert_eq!(handshake["path"], "/v1/terminal");
    let request_headers = &handshake["request"]["headers"];
    let response_headers = &handshake["response"]["headers"];
    assert_eq!(handshake["response"]["status"], 101);
    let key = string(&request_headers["sec-websocket-key"]);
    assert_eq!(STANDARD.decode(key).unwrap().len(), 16);
    assert_eq!(
        response_headers["sec-websocket-accept"],
        tokio_tungstenite::tungstenite::handshake::derive_accept_key(key.as_bytes())
    );
    assert_eq!(response_headers["sec-websocket-protocol"], protocol);
    assert!(!response_headers
        .as_object()
        .unwrap()
        .contains_key("sec-websocket-extensions"));
    for case in handshake["offers"].as_array().unwrap() {
        let tokens: Vec<_> = case["tokens"]
            .as_array()
            .unwrap()
            .iter()
            .map(string)
            .collect();
        let tickets = tokens
            .iter()
            .filter_map(|token| token.strip_prefix("ticket."));
        let accepted = tokens.len() == 2
            && tokens.contains(&protocol)
            && tickets.filter(|token| ticket_token(token)).count() == 1;
        assert_eq!(case["accepted"], accepted, "{}", case["name"]);
    }
    let issue = &contract["ticket"];
    assert_eq!(issue["path"], "/v1/terminal-tickets");
    assert_eq!(issue["response"]["status"], 201);
    assert_eq!(issue["response"]["headers"]["cache-control"], "no-store");
    assert!(ticket_token(string(&issue["response"]["body"]["ticket"])));
    assert_eq!(
        serde_json::to_vec(&issue["request"]["body"]).unwrap(),
        string(&issue["request"]["utf8"]).as_bytes()
    );
    let parse = |value: &Value| {
        time::OffsetDateTime::parse(
            string(value),
            &time::format_description::well_known::Rfc3339,
        )
        .unwrap()
    };
    let max_age = number(&contract["limits"]["tickets"]["max_age_ms"]);
    for case in issue["lifetime_cases"].as_array().unwrap() {
        let ticket_deadline = parse(&case["issued_at"])
            + time::Duration::milliseconds(i64::try_from(max_age).unwrap());
        assert_eq!(
            parse(&case["expires_at"]),
            ticket_deadline.min(parse(&case["proof_deadline"]))
        );
    }
}

fn http_token(value: &Value) -> bool {
    value.as_str().is_some_and(|text| {
        !text.is_empty()
            && text
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^`_|~".contains(&byte))
    })
}

fn browser_selection(case: &Value, protocol: &Value) -> &'static str {
    let Some(descriptor) = case.get("descriptor") else {
        return "webrtc";
    };
    let offered = &descriptor["protocol"];
    match descriptor["version"].as_u64() {
        Some(1) if offered == protocol && case["capability"] == true => "wss",
        Some(version) if version > 1 && offered != protocol && http_token(offered) => "webrtc",
        _ => "protocol_error",
    }
}

#[test]
fn opening_vectors_share_the_legacy_contract() {
    let contract = fixture();
    let opening = &contract["opening"];
    let manifest: Value = serde_json::from_str(include_str!(
        "../../fixtures/api/http/resource-manifest.json"
    ))
    .unwrap();
    let legacy = &opening["base"]["transport"];
    assert_eq!(legacy["terminal"], "webrtc");
    assert_eq!(legacy["durable_events"], false);
    assert_eq!(legacy["resource_manifest_version"], manifest["version"]);
    let ids = manifest["operations"].as_array().unwrap();
    assert!(ids
        .iter()
        .any(|route| route["method"] == "term.list"
            && route["path"] == "/v1/terminal-roster-queries"));
    assert!(ids
        .iter()
        .all(|route| route["path"] != contract["handshake"]["path"]
            && route["path"] != contract["ticket"]["path"]));
    assert_eq!(opening["capability"], "term.websocket");
    assert_eq!(
        opening["descriptor"],
        json!({"version":1,"protocol":"build.term.v1"})
    );
    for case in opening["cases"].as_array().unwrap() {
        let selected = browser_selection(case, &opening["descriptor"]["protocol"]);
        assert_eq!(case["browser"], selected, "{}", case["name"]);
        assert_eq!(
            case["desktop"],
            if selected == "wss" && case["native_terminal_ipc"] != "complete" {
                "webrtc"
            } else {
                selected
            }
        );
    }
}

fn depth(value: &Value) -> u64 {
    match value {
        Value::Object(fields) => 1 + fields.values().map(depth).max().unwrap_or(0),
        Value::Array(values) => 1 + values.iter().map(depth).max().unwrap_or(0),
        _ => 0,
    }
}

fn named<'a>(cases: &'a Value, name: &str) -> &'a Value {
    cases
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["name"] == name)
        .unwrap()
}

#[test]
fn invalid_binary_vectors_cover_parser_and_admission_boundaries() {
    let contract = fixture();
    let cases = &contract["invalid_messages"];
    let bad_utf8 = named(cases, "invalid_utf8");
    let hex = string(&bad_utf8["hex"]);
    let bytes: Vec<_> = (0..hex.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&hex[index..index + 2], 16).unwrap())
        .collect();
    assert!(std::str::from_utf8(&bytes).is_err());
    assert_eq!(bad_utf8["close_code"], 1007);
    assert!(serde_json::from_str::<Value>(string(&named(cases, "invalid_json")["utf8"])).is_err());
    assert!(serde_json::from_str::<Value>(string(&named(cases, "utf8_bom")["utf8"])).is_err());
    let text_frame = named(cases, "text_frame");
    assert_eq!(wire(text_frame), wire(&contract["messages"]["input"]));
    assert_eq!(
        (
            number(&text_frame["opcode"]),
            number(&text_frame["close_code"])
        ),
        (1, 1003)
    );
    let nested = wire(named(cases, "excess_nesting"));
    assert_eq!(
        depth(&nested),
        number(&contract["limits"]["max_json_container_depth"]) + 1
    );

    let duplicate = named(cases, "duplicate_outstanding_id");
    assert!(duplicate["outstanding_ids"]
        .as_array()
        .unwrap()
        .contains(&wire(duplicate)["id"]));
    let input = wire(named(cases, "invalid_input_base64"));
    assert!(STANDARD.decode(string(&input["params"]["data"])).is_err());
    let roster = wire(named(cases, "roster_over_socket"));
    assert!(!contract["wire"]["methods"]
        .as_array()
        .unwrap()
        .contains(&roster["method"]));
}

#[test]
fn only_attaches_and_resets_get_the_large_message_budget() {
    let contract = fixture();
    let limits = &contract["limits"];
    let eligible = &limits["snapshot_eligible"];
    for case in limits["message_cases"].as_array().unwrap() {
        let budget = if let Some(fragments) = case["fragments"].as_array() {
            assert_eq!(
                fragments.iter().map(number).sum::<u64>(),
                number(&case["bytes"])
            );
            "client"
        } else if case["push"] == eligible["push"]
            || (case["ok"] == true
                && eligible["successful_reply_to"]
                    .as_array()
                    .unwrap()
                    .contains(&case["reply_to"]))
        {
            "snapshot"
        } else {
            "ordinary"
        };
        assert_eq!(case["budget"], budget);
        assert_eq!(
            case["accepted"],
            number(&case["bytes"]) <= number(&limits["message_bytes"][budget])
        );
    }
    for case in limits["decoded_input_cases"].as_array().unwrap() {
        let base64 = STANDARD.encode(vec![0; usize::try_from(number(&case["bytes"])).unwrap()]);
        let decoded = STANDARD.decode(base64).unwrap();
        assert_eq!(
            case["accepted"],
            decoded.len() as u64 <= number(&limits["decoded_input_bytes"])
        );
    }
}

fn supported_grid(grid: &Value, cols: u64, rows: u64) -> bool {
    (1..=number(&grid["max_cols"])).contains(&cols)
        && (1..=number(&grid["max_rows"])).contains(&rows)
        && cols * rows <= number(&grid["max_cells"])
}

#[test]
fn dimensions_and_reservations_match_the_plan() {
    let contract = fixture();
    let limits = &contract["limits"];
    let grid = &limits["grid"];
    for case in grid["cases"].as_array().unwrap() {
        let default = &grid["defaults"][string(&case["method"])];
        let axis = |name: &str| {
            case["params"]
                .get(name)
                .map_or(default[name].as_u64(), Value::as_u64)
        };
        let valid = axis("cols")
            .zip(axis("rows"))
            .is_some_and(|(cols, rows)| supported_grid(grid, cols, rows));
        assert_eq!(case["accepted"], valid, "{}", case["name"]);
    }
    let bound = &limits["snapshot_bound"];
    assert!(include_str!("../Cargo.lock").contains(&format!(
        "name = \"vt100\"\nversion = \"{}\"",
        string(&bound["vt100_version"])
    )));
    let per_cell = number(&bound["utf8_bytes_per_cell"])
        + number(&bound["sgr_bytes_per_cell"])
        + number(&bound["positioning_erase_bytes_per_cell"]);
    assert_eq!(bound["raw_bytes_per_cell"], per_cell);
    assert_eq!(per_cell, 128);
    let raw = per_cell * number(&grid["max_cells"])
        + number(&bound["raw_bytes_per_row"]) * number(&grid["max_rows"])
        + number(&bound["raw_bytes_per_screen"]);
    assert_eq!(bound["raw_bytes"], raw);
    let encoded = 4 * raw.div_ceil(3) + number(&bound["json_metadata_bytes"]);
    assert_eq!(bound["encoded_bytes"], encoded);
    assert!(encoded < number(&limits["message_bytes"]["snapshot"]));

    let sockets = &limits["sockets"];
    assert_eq!(
        (
            number(&sockets["held_listener"]),
            number(&sockets["held_family"]),
            number(&sockets["current_opening"])
        ),
        (16, 8, 1)
    );
    for case in limits["socket_cases"].as_array().unwrap() {
        let admitted = number(&case["held_family"]) < number(&sockets["held_family"])
            && number(&case["held_listener"]) < number(&sockets["held_listener"]);
        assert_eq!(case["admit"], admitted, "{}", case["name"]);
    }
    let memory = &limits["bridge_reservations_bytes"];
    let ordinary = (number(&memory["inbound_per_socket"])
        + number(&memory["ordinary_outbound_per_socket"])
        + number(&memory["ordinary_codec_per_socket"]))
        * number(&sockets["held_listener"]);
    assert_eq!(memory["ordinary_total"], ordinary);
    assert_eq!(
        memory["total"],
        ordinary + number(&memory["snapshot_slot"]) * number(&limits["snapshot_slots"]["bridge"])
    );
    assert_eq!(memory["total"], 224 * 1024 * 1024);
}

#[test]
fn a_supported_viewport_can_target_an_unsupported_retained_screen() {
    let contract = fixture();
    let unsupported = &contract["recovery"]["unsupported"];
    let grid = &contract["limits"]["grid"];
    let requested = &unsupported["requested_grid"];
    assert!(supported_grid(
        grid,
        number(&requested["cols"]),
        number(&requested["rows"])
    ));
    let retained = &unsupported["retained_grid"];
    let cols = u16::try_from(number(&retained["cols"])).unwrap();
    let rows = u16::try_from(number(&retained["rows"])).unwrap();
    let screen = ScreenHandle::new("term-1", cols, rows);
    let (actual_cols, actual_rows) = screen.size();
    assert!(!supported_grid(
        grid,
        u64::from(actual_cols),
        u64::from(actual_rows)
    ));
    assert_eq!(
        wire(&contract["messages"][string(&unsupported["attach_reply"])])["error_code"],
        "snapshot_too_large"
    );
    assert_eq!(
        wire(&contract["messages"][string(&unsupported["live_attachment_control"])])["type"],
        "attachment_error"
    );
    assert_eq!(
        unsupported["automatic_reattaches_including_owner_reconnect"],
        0
    );
}

fn paint(recovery: &Value, cols: u16, rows: u16) -> Vec<u8> {
    let mut bytes = string(&recovery["prefix"]).as_bytes().to_vec();
    for row in 0..rows {
        for col in 0..cols {
            let cell = &recovery["cells"][usize::from((row + col) % 2)];
            bytes.extend_from_slice(string(&cell["sgr"]).as_bytes());
            bytes.extend_from_slice(string(&cell["text"]).as_bytes());
        }
    }
    bytes.extend_from_slice(string(&recovery["suffix"]).as_bytes());
    bytes
}

fn check_recovery_styles(screen: &vt100::Screen, recipe: &Value) {
    let rgb = |value: &Value| {
        let component = |index: usize| u8::try_from(number(&value[index])).unwrap();
        vt100::Color::Rgb(component(0), component(1), component(2))
    };
    for col in 0..2 {
        let expected = &recipe[usize::from(col)];
        let cell = screen.cell(0, col).unwrap();
        assert_eq!(cell.fgcolor(), rgb(&expected["fg_rgb"]));
        assert_eq!(cell.bgcolor(), rgb(&expected["bg_rgb"]));
        for (style, enabled) in [
            ("bold", cell.bold()),
            ("italic", cell.italic()),
            ("underline", cell.underline()),
            ("inverse", cell.inverse()),
        ] {
            assert_eq!(
                enabled,
                expected["styles"]
                    .as_array()
                    .unwrap()
                    .contains(&json!(style)),
                "{style}"
            );
        }
    }
}

#[test]
fn maximal_sgr_recipe_round_trips_through_the_real_screen_formatter() {
    let contract = fixture();
    let recovery = &contract["recovery"];
    let cols = u16::try_from(number(&recovery["cols"])).unwrap();
    let rows = u16::try_from(number(&recovery["rows"])).unwrap();
    let pty = paint(recovery, cols, rows);
    let screen = ScreenHandle::new("term-1", cols, rows);
    assert!(screen.feed(&pty));
    let snapshot = screen.snapshot();
    assert_eq!(snapshot.cursor, pty.len() as u64);
    assert_eq!((snapshot.cols, snapshot.rows), (cols, rows));
    let raw = STANDARD.decode(&snapshot.snapshot).unwrap();
    let mut actual = vt100::Parser::new(rows, cols, 0);
    actual.process(&raw);
    check_recovery_styles(actual.screen(), &recovery["cells"]);
    let mut expected = vt100::Parser::new(rows, cols, 0);
    expected.process(&pty);
    for row in 0..rows {
        for col in 0..cols {
            assert_eq!(
                actual.screen().cell(row, col),
                expected.screen().cell(row, col),
                "cell at {row},{col}"
            );
        }
    }
    let cursor = &recovery["cursor_zero_based"];
    let position = actual.screen().cursor_position();
    assert_eq!(json!([position.0, position.1]), *cursor);
    for witness in recovery["witness_cells"].as_array().unwrap() {
        let row = u16::try_from(number(&witness["row"])).unwrap();
        let col = u16::try_from(number(&witness["col"])).unwrap();
        assert_eq!(
            actual.screen().cell(row, col).unwrap().contents(),
            string(&witness["text"])
        );
    }

    let mut attach = wire(&contract["messages"]["attach_ok"]);
    attach["result"]["cols"] = json!(cols);
    attach["result"]["rows"] = json!(rows);
    attach["result"]["cursor"] = json!(snapshot.cursor);
    attach["result"]["snapshot"] = json!(snapshot.snapshot);
    let mut reset = wire(&contract["messages"]["reset"]);
    reset["cursor"] = json!(snapshot.cursor);
    reset["data"] = attach["result"]["snapshot"].clone();
    for envelope in [attach, reset] {
        let len = serde_json::to_vec(&envelope).unwrap().len() as u64;
        assert!(len > number(&contract["limits"]["message_bytes"]["ordinary"]));
        assert!(len <= number(&contract["limits"]["message_bytes"]["snapshot"]));
    }
    let tail = string(&recovery["next_output"]);
    assert!(screen.feed(tail.as_bytes()));
    assert_eq!(
        screen.snapshot().cursor,
        snapshot.cursor + tail.len() as u64
    );
    actual.process(tail.as_bytes());
    assert_eq!(
        actual
            .screen()
            .cell(position.0, position.1)
            .unwrap()
            .contents(),
        tail
    );
}

use super::*;

const STREAM: &str = "mixed-output";

fn fixture(events: Vec<LogEvent>) -> (tempfile::TempDir, Arc<Mutex<AppState>>) {
    let root = tempfile::tempdir().unwrap();
    let mut app = AppState::new_unrooted(
        root.path().join("worktrees"),
        "main",
        true,
        "/tmp/stream-snapshots.sock",
    );
    app.streams.insert(
        STREAM.into(),
        StreamState {
            count: 0,
            events,
            complete: false,
            ..Default::default()
        },
    );
    app.initialize_snapshot_projection();
    (root, Arc::new(Mutex::new(app)))
}

fn mixed_events() -> Vec<LogEvent> {
    [
        ("output", json!({"text":""})),
        ("output", json!({"text":null})),
        ("other", json!({"text":"ignored"})),
        ("output", json!({"text":"α\nβ"})),
        ("output", json!({})),
        ("output", json!({"text":""})),
        ("done", json!({"count":0})),
    ]
    .into_iter()
    .enumerate()
    .map(|(index, (kind, data))| LogEvent {
        seq: index as u64 + 1,
        kind: kind.into(),
        data,
    })
    .collect()
}

#[test]
fn warming_seeded_streams_publishes_exact_output_and_each_event() {
    let events = mixed_events();
    let expected = concat_output(&events);
    assert_eq!(expected, "\nα\nβ\n");
    let (_root, state) = fixture(events.clone());
    let app = state.lock().unwrap();
    let view = app.read_view.load();
    let summary = view.get_owned("stream.state", STREAM).unwrap();
    assert_eq!(summary["output_len"], expected.len());
    assert_eq!(summary["checksum"], sha256_hex(expected.as_bytes()));
    assert_eq!(summary["head"], events.len());
    for event in events {
        let actual = view
            .get_owned("stream.events", &format!("{STREAM}/{}", event.seq))
            .unwrap();
        assert_eq!(
            actual,
            json!({"seq":event.seq,"kind":event.kind,"data":event.data})
        );
    }
}

#[test]
fn each_append_publishes_a_coherent_summary_and_preserves_retained_roots() {
    let (_root, state) = fixture(Vec::new());
    let view = state.lock().unwrap().read_view.clone();
    let before = view.load();
    assert_eq!(before.get_owned("stream.state", STREAM).unwrap()["head"], 0);
    let mut appended = Vec::new();
    for event in mixed_events() {
        assert!(append_event(
            &state,
            STREAM,
            &event.kind,
            event.data.clone()
        ));
        appended.push(event.clone());
        let current = view.load();
        let summary = current.get_owned("stream.state", STREAM).unwrap();
        let expected = concat_output(&appended);
        assert_eq!(summary["head"], event.seq);
        assert_eq!(summary["complete"], event.kind == "done");
        assert_eq!(summary["output_len"], expected.len());
        assert_eq!(summary["checksum"], sha256_hex(expected.as_bytes()));
        assert!(current
            .get_owned("stream.events", &format!("{STREAM}/{}", event.seq))
            .is_some());
    }
    assert_eq!(before.get_owned("stream.state", STREAM).unwrap()["head"], 0);
    assert!(before
        .get_owned("stream.events", &format!("{STREAM}/1"))
        .is_none());
}

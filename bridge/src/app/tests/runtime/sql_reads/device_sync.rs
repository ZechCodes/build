use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn device_sync_wire_uses_leased_snapshot_and_replays_changes_off_app_lock() {
    device_roundtrip(Duration::from_secs(5), 4).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "100 ms acceptance timing runs serially in the release timing gate"]
async fn device_sync_wire_roundtrip_within_100ms() {
    device_roundtrip(Duration::from_millis(100), 6000).await;
}

async fn device_roundtrip(deadline: Duration, fixture_rows: usize) {
    let dir = tempfile::tempdir().unwrap();
    let (_repo, app, _project_id) = tracked(dir.path());
    let store = app.store.clone().unwrap();
    let writer = store.writer();
    let now = crate::store::sync::now_ms();
    let generation = writer
        .mark_sync_dirty("wire_fixture", "wire-fixture", now)
        .unwrap();
    writer
        .reconcile_sync_dirty(
            "wire_fixture",
            "wire-fixture",
            generation,
            Some(json!({"name":"before"})),
            now,
        )
        .unwrap();
    writer
        .sync_transaction(|_, batch| {
            for index in 0..fixture_rows {
                batch.upsert(
                    crate::store::sync::Entity::new("wire_fixture", &format!("large-{index:05}")),
                    json!({"name":"bounded directory"}),
                    &[crate::store::sync::Stream::device()],
                );
            }
            Ok(())
        })
        .unwrap();
    let state = app.shared();
    let samples = state.lock().unwrap().frame_clock.capture_phases();
    let transport = crate::transport::generate_transport_keypair();
    let intake = FrameIntake::new(AppState::handler(Arc::clone(&state)), transport.clone());
    let mut wire = ChannelWire::open();
    let key = crate::transport::generate_session_key();
    wire.open_session(
        &intake,
        "device-sync",
        &session_init("device-sync", &transport.public_key_b64, &key),
    )
    .unwrap();
    wire.outbound.try_recv().unwrap();
    let (held, holding) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel();
    let holder = std::thread::spawn(move || {
        let _app = state.lock().unwrap();
        held.send(()).unwrap();
        released.recv_timeout(Duration::from_secs(10)).unwrap();
    });
    holding.recv_timeout(Duration::from_secs(5)).unwrap();
    let snapshot_started = std::time::Instant::now();
    wire.accept(&intake, client_request(&key, "device-sync", "data", json!({"id":1,"method":"sync.snapshot","params":{"scope":{"kind":"device"},"limit":100}}))).await.unwrap();
    let answer = tokio::time::timeout(deadline, final_answer(&mut wire, &key)).await;
    let snapshot_elapsed = snapshot_started.elapsed();
    release.send(()).unwrap();
    holder.join().unwrap();
    let answer =
        answer.expect("device lease admission and first page must bypass app lock before release");
    eprintln!(
        "device wire snapshot rows={fixture_rows} elapsed_us={}",
        snapshot_elapsed.as_micros()
    );
    let phases = samples.snapshot();
    eprintln!("device snapshot phases={phases}");
    assert_eq!(
        phases["sync.snapshot"]["device.lease_writer_hold"]["count"],
        1
    );
    assert_eq!(
        phases["sync.snapshot"]["frame.held"]["sum_ms"], 0.0,
        "device recovery must not hold the app mutex"
    );
    assert_eq!(answer["ok"], true, "{answer}");
    assert_eq!(answer["result"]["consistency"], "leased", "{answer}");
    assert!(answer["result"]["lease"]["id"].is_string(), "{answer}");
    assert!(
        answer["result"]["records"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["key"] == "wire-fixture" && row["row"]["name"] == "before"),
        "{answer}"
    );
    let cursor = answer["result"]["cursor"].clone();
    let generation = writer
        .mark_sync_dirty("wire_fixture", "wire-fixture", now + 1)
        .unwrap();
    writer
        .reconcile_sync_dirty(
            "wire_fixture",
            "wire-fixture",
            generation,
            Some(json!({"name":"after"})),
            now + 1,
        )
        .unwrap();
    for (id, method, params) in [
        (
            2,
            "sync.head",
            json!({"scope":{"kind":"device"},"cursor":cursor}),
        ),
        (
            3,
            "sync.changes",
            json!({"scope":{"kind":"device"},"cursor":cursor}),
        ),
    ] {
        let started = std::time::Instant::now();
        wire.accept(
            &intake,
            client_request(
                &key,
                "device-sync",
                "data",
                json!({"id":id,"method":method,"params":params}),
            ),
        )
        .await
        .unwrap();
        let answer = tokio::time::timeout(deadline, final_answer(&mut wire, &key))
            .await
            .unwrap();
        eprintln!(
            "device wire {method} elapsed_us={}",
            started.elapsed().as_micros()
        );
        assert_eq!(answer["ok"], true, "{answer}");
        if method == "sync.changes" {
            assert!(
                answer["result"]["changes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|row| row["key"] == "wire-fixture" && row["row"]["name"] == "after"),
                "{answer}"
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn device_sync_wire_rejects_unknown_scope_as_negative_control() {
    let dir = tempfile::tempdir().unwrap();
    let (_repo, app, _project_id) = tracked(dir.path());
    let transport = crate::transport::generate_transport_keypair();
    let intake = FrameIntake::new(AppState::handler(app.shared()), transport.clone());
    let mut wire = ChannelWire::open();
    let key = crate::transport::generate_session_key();
    wire.open_session(
        &intake,
        "device-negative",
        &session_init("device-negative", &transport.public_key_b64, &key),
    )
    .unwrap();
    wire.outbound.try_recv().unwrap();
    wire.accept(
        &intake,
        client_request(
            &key,
            "device-negative",
            "data",
            json!({"id":1,"method":"sync.snapshot","params":{"scope":{"kind":"unknown_device"}}}),
        ),
    )
    .await
    .unwrap();
    let answer = final_answer(&mut wire, &key).await;
    assert_eq!(answer["ok"], false, "{answer}");
    assert_eq!(answer["error_code"], "invalid_params", "{answer}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn device_sync_wire_binds_snapshot_pages_to_the_exact_opening() {
    let dir = tempfile::tempdir().unwrap();
    let (_repo, app, _project_id) = tracked(dir.path());
    let writer = app.store.as_ref().unwrap().writer();
    for key in ["opening-a", "opening-b"] {
        let now = crate::store::sync::now_ms();
        let generation = writer.mark_sync_dirty("wire_fixture", key, now).unwrap();
        writer
            .reconcile_sync_dirty(
                "wire_fixture",
                key,
                generation,
                Some(json!({"name":key})),
                now,
            )
            .unwrap();
    }
    let transport = crate::transport::generate_transport_keypair();
    let intake = FrameIntake::new(AppState::handler(app.shared()), transport.clone());
    let key = crate::transport::generate_session_key();
    let mut one = ChannelWire::open();
    one.open_session(
        &intake,
        "device-page-one",
        &session_init("device-page-one", &transport.public_key_b64, &key),
    )
    .unwrap();
    one.outbound.try_recv().unwrap();
    one.accept(
        &intake,
        client_request(
            &key,
            "device-page-one",
            "data",
            json!({"id":1,"method":"sync.snapshot","params":{"scope":{"kind":"device"},"limit":1}}),
        ),
    )
    .await
    .unwrap();
    let answer = final_answer(&mut one, &key).await;
    assert_eq!(answer["ok"], true, "{answer}");
    let page = answer["result"]["next_page"]
        .as_str()
        .expect("two records require another page")
        .to_owned();
    let mut two = ChannelWire::open();
    two.open_session(
        &intake,
        "device-page-two",
        &session_init("device-page-two", &transport.public_key_b64, &key),
    )
    .unwrap();
    two.outbound.try_recv().unwrap();
    two.accept(&intake,client_request(&key,"device-page-two","data",json!({"id":2,"method":"sync.snapshot","params":{"scope":{"kind":"device"},"limit":1,"page":page}}))).await.unwrap();
    let foreign = final_answer(&mut two, &key).await;
    assert_eq!(foreign["ok"], false, "{foreign}");
    assert_eq!(foreign["error_code"], "invalid_params", "{foreign}");
    one.accept(&intake,client_request(&key,"device-page-one","data",json!({"id":3,"method":"sync.snapshot","params":{"scope":{"kind":"device"},"limit":1,"page":page}}))).await.unwrap();
    let valid = final_answer(&mut one, &key).await;
    assert_eq!(valid["ok"], true, "{valid}");
    one.accept(&intake,client_request(&key,"device-page-one","data",json!({"id":4,"method":"sync.snapshot","params":{"scope":{"kind":"device"},"limit":1,"page":format!("{page}A")}}))).await.unwrap();
    let tampered = final_answer(&mut one, &key).await;
    assert_eq!(tampered["ok"], false, "{tampered}");
    assert_eq!(tampered["error_code"], "invalid_params", "{tampered}");
}

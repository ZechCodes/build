use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn thread_revision_sql_lane_answers_with_zero_app_lock_time() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let mut record = fake_run_record("revision-held");
    let revision = record.agents[0].thread.add_revision(
        crate::thread::ArtifactKind::Diff,
        "held lock revision witness",
        "2026-10-21T12:00:00Z",
    );
    app.store.as_ref().unwrap().save_run(&record).unwrap();
    app.runs.insert(
        record.id.clone(),
        crate::orchestrator::ActiveRun::reattach(&record, ".build/plan.md".into()),
    );
    let state = app.shared();
    let samples = state.lock().unwrap().frame_clock.capture_phases();
    let transport = crate::transport::generate_transport_keypair();
    let intake = FrameIntake::new(AppState::handler(Arc::clone(&state)), transport.clone());
    let mut wire = ChannelWire::open();
    let key = crate::transport::generate_session_key();
    wire.open_session(
        &intake,
        "revision",
        &session_init("revision", &transport.public_key_b64, &key),
    )
    .unwrap();
    assert!(matches!(
        wire.outbound.try_recv(),
        Ok(OutboundEnvelope::SessionAccept { .. })
    ));

    let (held, holding) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel();
    let writer = Arc::clone(&state);
    let holder = std::thread::spawn(move || {
        let _app = writer.lock().unwrap();
        held.send(()).unwrap();
        released.recv_timeout(Duration::from_secs(10)).unwrap();
    });
    holding.recv_timeout(Duration::from_secs(5)).unwrap();
    let started = std::time::Instant::now();
    wire.accept(
        &intake,
        client_request(
            &key,
            "revision",
            "data",
            json!({
                "id":1,"method":"thread.revision","params":{
                    "entity_id":record.id,"revision_id":revision.id,
                    "agent_id":record.agents[0].id,"thread_id":record.agents[0].thread.id
                }
            }),
        ),
    )
    .await
    .unwrap();
    let answer =
        tokio::time::timeout(Duration::from_millis(100), final_answer(&mut wire, &key)).await;
    let elapsed = started.elapsed();
    release.send(()).unwrap();
    holder.join().unwrap();
    let answer = answer.expect("revision must reply while the application mutex remains held");
    assert!(
        elapsed <= Duration::from_millis(100),
        "reply took {elapsed:?}"
    );
    assert_eq!(answer["ok"], true, "{answer}");
    assert_eq!(answer["result"]["contents"], "held lock revision witness");
    tokio::time::timeout(Duration::from_secs(1), async {
        while samples.active_frames() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let captured = samples.snapshot();
    let frame = &captured["thread.revision"];
    assert_eq!(frame["frame.total"]["count"], 1, "{captured}");
    for phase in ["frame.lock_wait", "frame.held"] {
        assert_eq!(frame[phase]["count"], 1, "{captured}");
        assert_eq!(frame[phase]["max_ms"], 0.0, "{captured}");
    }
}

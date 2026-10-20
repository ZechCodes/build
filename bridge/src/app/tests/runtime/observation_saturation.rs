//! Client-observed deadlines through the real handler with actual SQL saturation.
use super::*;
use crate::carrier::testing::{client_request, session_init, ChannelWire};
use crate::carrier::{FrameIntake, OutboundEnvelope};
use std::time::Instant;

const SESSION: &str = "real-observation-saturation";
const READERS: usize = 8;

async fn send(
    wire: &ChannelWire,
    intake: &FrameIntake,
    key: &str,
    id: usize,
    method: &str,
    params: Value,
) {
    wire.accept(
        intake,
        client_request(
            key,
            SESSION,
            "data",
            json!({"id":id,"method":method,"params":params}),
        ),
    )
    .await
    .unwrap();
}

async fn answer(wire: &mut ChannelWire, key: &str) -> Value {
    loop {
        let envelope = wire
            .outbound
            .recv()
            .await
            .expect("the observation session stays open");
        let value = SessionSender::decrypt_push(key, &envelope);
        if value.get("accepted").is_none() {
            return value;
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_file_and_git_reads_finish_within_100ms_while_eight_mixed_reads_wait_for_sql() {
    let (directory, repository) = init_repo();
    let visible = "saturation-visible.txt";
    std::fs::write(repository.join(visible), "visible during SQL saturation\n").unwrap();
    let mut app = qa_state(&repository, directory.path());
    let project = app.project_at(0).id.clone();
    let tasks: Vec<_> = (0..READERS)
        .map(|index| {
            crate::app::tests::tracker::filed(
                &mut app,
                &project,
                &format!("saturated mixed read {index}"),
            )["id"]
                .clone()
        })
        .collect();
    let pool = app.store.as_ref().unwrap().read_pool();
    let clock = app.frame_clock.clone();
    let state = app.shared();
    let handler = AppState::handler(state.clone());
    handler.start_read_workers().unwrap();
    let transport = crate::transport::generate_transport_keypair();
    let intake = FrameIntake::new(handler, transport.clone());
    let mut wire = ChannelWire::open();
    let key = crate::transport::generate_session_key();
    wire.open_session(
        &intake,
        SESSION,
        &session_init(SESSION, &transport.public_key_b64, &key),
    )
    .unwrap();
    assert!(matches!(
        wire.outbound.try_recv(),
        Ok(OutboundEnvelope::SessionAccept { .. })
    ));

    let mut held = Vec::new();
    for _ in 0..READERS {
        held.push(pool.acquire().await);
    }
    for (id, task_id) in tasks.iter().enumerate() {
        send(
            &wire,
            &intake,
            &key,
            id,
            "tasks.review.get",
            json!({"task_id":task_id}),
        )
        .await;
    }
    assert_eq!(clock.stats()["queue_depth"], READERS);
    let mut observations = Vec::new();
    for (id, method) in [(100, "fs.list"), (101, "git.status")] {
        let started = Instant::now();
        let result = tokio::time::timeout(Duration::from_secs(2), async {
            send(
                &wire,
                &intake,
                &key,
                id,
                method,
                json!({"project_id":project}),
            )
            .await;
            answer(&mut wire, &key).await
        })
        .await;
        let elapsed = started.elapsed();
        eprintln!("SATURATION real AppState handler: {method} send-to-decrypted-reply={:.3}ms held_connections={} queued_mixed={READERS}",
            elapsed.as_secs_f64() * 1000.0, held.len());
        observations.push((id, method, result, elapsed));
    }
    // Neither elapsed interval includes releasing SQL. Cleanup precedes all
    // reply/deadline assertions, so failure never strands the mixed requests.
    drop(held);
    for (id, method, result, elapsed) in observations {
        let reply = result.expect("real pure reads finish while SQL connections remain held");
        assert_eq!(reply["id"], id, "{reply}");
        assert_eq!(reply["ok"], true, "{reply}");
        let entries = if method == "fs.list" {
            &reply["result"]["entries"]
        } else {
            &reply["result"]["files"]
        };
        let field = if method == "fs.list" { "name" } else { "path" };
        assert!(
            entries
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry[field] == visible),
            "{reply}"
        );
        if method == "git.status" {
            assert_eq!(reply["result"]["branch"], "main");
        }
        assert!(
            elapsed <= Duration::from_millis(100),
            "real {method} under SQL saturation took {elapsed:?}, exceeding 100ms"
        );
    }
    let mut completed = std::collections::BTreeSet::new();
    for _ in 0..READERS {
        let reply = tokio::time::timeout(Duration::from_secs(5), answer(&mut wire, &key))
            .await
            .unwrap();
        assert_eq!(reply["ok"], true, "{reply}");
        assert_eq!(reply["result"]["review"], Value::Null, "{reply}");
        completed.insert(reply["id"].as_u64().unwrap());
    }
    assert_eq!(completed, (0..READERS as u64).collect());
}

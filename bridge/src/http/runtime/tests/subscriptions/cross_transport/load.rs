use super::cancellation::Gate;
use super::fixture::*;
use super::*;
use crate::http::{limits::TOTAL_BUFFER_BYTES, listener::OutputAuthority, sse::MAX_QUEUED_BYTES};

/// Pass: 4 negotiated-channel sessions and all 32 HTTP streams coexist; each
/// batch's four REST calls finish while all four RTC handlers remain blocked.
/// Each transport completes 32 calls; each SSE stream receives exactly three
/// pushes in order. Both stream limits refuse overflow and all allowances return.
#[tokio::test]
async fn cross_transport_full_stream_pool_preserves_progress_and_bounds() {
    let mut gates = (0..8).map(|_| Gate::new()).collect::<Vec<_>>();
    let completed: Arc<[AtomicUsize; 8]> = Arc::new(std::array::from_fn(|_| AtomicUsize::new(0)));
    let mut f = Fixture::new(load_handler(&gates, completed.clone()));
    assert_eq!(STREAM_BUFFER_BYTES, 32 * 1024 * 1024);
    assert_eq!(MAX_FAMILY_STREAMS, 8);
    for _ in 0..MAX_FAMILY_STREAMS {
        f.http.push(f.http_client("family-0").await);
    }
    f.refused_stream("family-0").await;
    f.fill_stream_pool().await;
    f.refused_stream("family-0").await;
    f.refused_stream("another-family").await;

    for (batch, gate) in gates.iter_mut().enumerate() {
        for (index, client) in f.rtc.iter().enumerate() {
            client
                .send(&f.intake, batch * RTC_SESSIONS + index)
                .await
                .unwrap();
        }
        let mut running = Vec::new();
        for _ in 0..RTC_SESSIONS {
            running.push(gate.started().await);
        }
        // Only release RTC work after every REST response in this batch.
        for index in batch * 4..batch * 4 + 4 {
            let request = f.greeting(&f.http[index]);
            let output = request
                .extensions()
                .get::<OutputAuthority>()
                .unwrap()
                .clone();
            let response = tokio::time::timeout(WATCHDOG, f.app.clone().oneshot(request))
                .await
                .expect("REST progresses beside WebRTC and the full SSE pool")
                .unwrap();
            let (status, value) = json_response(response).await;
            assert_eq!(status, StatusCode::OK, "{value}");
            assert_eq!(
                completed[batch].load(Ordering::SeqCst),
                0,
                "REST must return 200 while all four RTC handlers remain pending"
            );
            assert!(running.iter().all(|context| context.read_is_live()));
            flush(output).await;
            assert_ordered_pushes(&mut f.http[index]).await;
            let queued = {
                let openings = f.runtime.state.openings.lock().unwrap();
                openings[&f.http[index].session]
                    .streams
                    .binding(Some(&f.http[index].subscription))
                    .unwrap()
                    .queued()
            };
            assert_eq!(
                queued,
                (0, 0),
                "exactly three pushes: no trailing duplicate remains queued"
            );
        }
        gate.release();
        for (index, client) in f.rtc.iter_mut().enumerate() {
            client.reply(batch * RTC_SESSIONS + index).await;
        }
        assert_eq!(completed[batch].load(Ordering::SeqCst), RTC_SESSIONS);
        assert_eq!(f.runtime.state.buffers.available(), TOTAL_BUFFER_BYTES);
        assert_eq!(f.runtime.state.buffers.streams_available(), 0);
        let openings = f.runtime.state.openings.lock().unwrap();
        for client in &f.http {
            let (items, bytes) = openings[&client.session]
                .streams
                .binding(Some(&client.subscription))
                .unwrap()
                .queued();
            assert!(items <= crate::http::sse::MAX_QUEUED_EVENTS);
            assert!(bytes <= MAX_QUEUED_BYTES);
        }
    }
    f.http.clear();
    assert_eq!(
        f.runtime.state.buffers.streams_available(),
        STREAM_BUFFER_BYTES
    );
    println!("cross transport: {RTC_SESSIONS} RTC sessions held pending for every REST batch + {POOL_STREAMS} streams; 32 RTC + 32 REST calls; 3 ordered SSE pushes per stream without duplicates; RPC pool {TOTAL_BUFFER_BYTES} bytes, SSE pool {STREAM_BUFFER_BYTES} bytes, queue {MAX_QUEUED_BYTES} bytes");
}

fn load_handler(gates: &[Gate], completed: Arc<[AtomicUsize; 8]>) -> FrameHandler {
    let handlers = gates
        .iter()
        .map(|gate| gate.handler.clone())
        .collect::<Vec<_>>();
    FrameHandler::new(FrameClock::new(), move |sender, frame, timer| {
        if sender.session_id().starts_with("acceptance-rtc-") {
            let batch = frame.payload["id"].as_u64().unwrap() as usize / RTC_SESSIONS;
            let result = (handlers[batch].dispatch)(sender, frame, timer);
            completed[batch].fetch_add(1, Ordering::SeqCst);
            return result;
        }
        for sequence in 0..3 {
            sender.push(json!({"type": "probe.cross_transport", "seq": sequence}));
        }
        json!({"id": frame.payload["id"], "ok": true, "result": {}})
    })
    .with_greeting_reads(
        |epoch| json!({"api_version": crate::api::API_VERSION, "bridge_epoch": epoch}),
    )
}

async fn assert_ordered_pushes(client: &mut HttpClient) {
    for sequence in 0..3 {
        let (name, event) = next_event(&mut client.events).await;
        assert_eq!(name, "event");
        assert_eq!(event["type"], "probe.cross_transport");
        assert_eq!(
            event["seq"], sequence,
            "SSE pushes preserve exact sequence without duplicates"
        );
    }
}

/// Pass: returning any stream's reservation admits a replacement from the same
/// family while other families and every WebRTC session remain carried.
#[tokio::test]
async fn cross_transport_cancel_stream_returns_family_and_pool_reservation() {
    let mut f = Fixture::new(handler());
    f.fill_stream_pool().await;
    let removed = f.http.remove(0);
    let family = removed.family.clone();
    drop(removed);
    assert_eq!(
        f.runtime.state.buffers.streams_available(),
        MAX_QUEUED_BYTES
    );
    let replacement = f.http_client(&family).await;
    assert_eq!(f.runtime.state.buffers.streams_available(), 0);
    for client in &f.rtc {
        client.send(&f.intake, 42).await.unwrap();
    }
    for client in &mut f.rtc {
        client.reply(42).await;
    }
    drop(replacement);
    f.http.clear();
    assert_eq!(
        f.runtime.state.buffers.streams_available(),
        STREAM_BUFFER_BYTES
    );
}

use super::fixture::*;
use super::*;
use crate::http::{limits::TOTAL_BUFFER_BYTES, listener::OutputAuthority, sse::MAX_QUEUED_BYTES};

/// Pass: 4 negotiated-channel sessions and all 32 HTTP streams coexist; each
/// transport completes 32 calls and ordered pushes, both stream admission
/// limits refuse overflow, and RPC/stream allowances return to their pools.
#[tokio::test]
async fn cross_transport_full_stream_pool_preserves_progress_and_bounds() {
    let mut f = Fixture::new(handler());
    f.fill_stream_pool().await;
    f.refused_stream("family-0").await;
    f.refused_stream("another-family").await;

    for batch in 0..8 {
        for (index, client) in f.rtc.iter().enumerate() {
            client
                .send(&f.intake, batch * RTC_SESSIONS + index)
                .await
                .unwrap();
        }
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
            flush(output).await;
            let (name, event) = next_event(&mut f.http[index].events).await;
            assert_eq!(name, "event");
            assert_eq!(event["type"], "probe.cross_transport");
        }
        for (index, client) in f.rtc.iter_mut().enumerate() {
            client.reply(batch * RTC_SESSIONS + index).await;
        }
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
    println!("cross transport: {RTC_SESSIONS} RTC sessions + {POOL_STREAMS} streams; 32 RTC + 32 REST calls; RPC pool {TOTAL_BUFFER_BYTES} bytes, SSE pool {STREAM_BUFFER_BYTES} bytes, queue {MAX_QUEUED_BYTES} bytes");
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

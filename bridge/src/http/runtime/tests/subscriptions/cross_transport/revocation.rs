use super::fixture::*;
use super::*;
use crate::http::{limits::TOTAL_BUFFER_BYTES, listener::BoundedIo};
use tokio::io::AsyncWriteExt;

/// Pass: invalidating the shared owner authority and closing its DataChannel
/// carriers cuts all 32 stalled SSE writers in one 25 ms recheck, restores the
/// complete pool, and makes every former opening refuse another request.
/// This measures from local authority invalidation, excluding hosted latency.
#[tokio::test(start_paused = true)]
async fn cross_transport_revocation_cuts_stalled_output_within_one_recheck() {
    let mut f = Fixture::new(handler());
    f.fill_stream_pool().await;
    let requests: Vec<_> = f.http.iter().map(|client| f.greeting(client)).collect();
    let outputs: Vec<_> = f.http.iter().map(|client| client.output.clone()).collect();
    let mut writers = tokio::task::JoinSet::new();
    for client in f.http.drain(..) {
        writers.spawn(async move {
            let mut writer = BoundedIo::new(NeverWakeWriter, client.output.clone());
            let error = writer.write_all(b"event: pending\n\n").await.unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
            drop(client);
        });
    }
    while outputs.iter().any(|output| !output.has_pending_write()) {
        tokio::task::yield_now().await;
    }
    let revoked = Instant::now();
    f.live.store(false, Ordering::SeqCst);
    f.close_rtc();
    while let Some(result) = writers.join_next().await {
        result.unwrap();
    }
    assert!(
        revoked.elapsed() <= Duration::from_millis(25),
        "{:?}",
        revoked.elapsed()
    );
    for client in &f.rtc {
        assert!(client.send(&f.intake, 1).await.is_err());
    }
    for request in requests {
        assert_eq!(
            f.app.clone().oneshot(request).await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
    }
    f.runtime.state.reap_ended();
    assert_eq!(f.runtime.opening_count(), 0);
    assert_eq!(
        f.runtime.state.buffers.streams_available(),
        STREAM_BUFFER_BYTES
    );
    assert_eq!(f.runtime.state.buffers.available(), TOTAL_BUFFER_BYTES);
    println!("cross transport local revocation: {POOL_STREAMS} pending writers cut in {:?}; {RTC_SESSIONS} closed RTC sessions refuse traffic", revoked.elapsed());
}

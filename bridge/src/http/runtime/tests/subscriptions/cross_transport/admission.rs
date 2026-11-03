use super::cancellation::Gate;
use super::fixture::*;
use super::*;
use crate::carrier::SessionSender;
use crate::http::{limits::TOTAL_BUFFER_BYTES, listener::OutputAuthority};

/// Pass: a full production DataChannel intake queue rejects excess traffic as
/// busy, while REST immutable reads complete with the SSE pool full. Closing
/// that carrier drops its queued work and a different RTC carrier still works.
#[tokio::test]
async fn cross_transport_rtc_admission_is_bounded_without_starving_rest() {
    let mut gate = Gate::new();
    let handler = gate
        .handler
        .clone()
        .with_snapshot_reads(|frame| json!({"id": frame.payload["id"], "ok": true, "result": {}}));
    let mut f = Fixture::new(handler);
    f.fill_stream_pool().await;
    let mut running = Vec::new();
    for id in 0..8 {
        f.rtc[0].send(&f.intake, id).await.unwrap();
        running.push(gate.started().await);
    }
    // No yield in the next 1025 admits: the production per-wire queue is
    // exactly 1024, regardless of how many dispatcher slots become available.
    for id in 8..8 + 1025 {
        f.rtc[0].send(&f.intake, id).await.unwrap();
    }
    let key = f.rtc[0].key.clone();
    let refused = std::iter::from_fn(|| f.rtc[0].outbound.try_recv().ok())
        .filter(|frame| matches!(frame, crate::carrier::OutboundEnvelope::Frame(_)))
        .map(|frame| SessionSender::decrypt_push(&key, &frame))
        .any(|value| value["error_code"] == "busy" && value["retryable"] == true);
    assert!(
        refused,
        "the production 1024-frame carrier queue refuses excess admission"
    );
    let request = request_as(
        &f.http[0].family,
        "GET",
        "/v1/projects",
        &f.http[0].session,
        None,
    );
    let output = request
        .extensions()
        .get::<OutputAuthority>()
        .unwrap()
        .clone();
    let response = tokio::time::timeout(WATCHDOG, f.app.clone().oneshot(request))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(json_response(response).await.0, StatusCode::OK);
    flush(output).await;
    assert_eq!(f.runtime.state.buffers.available(), TOTAL_BUFFER_BYTES);
    f.intake.close_carrier(&f.rtc[0].carrier);
    assert!(running.iter().all(|context| !context.session_is_open()));
    drop(running);
    gate.release();
    f.rtc[1].send(&f.intake, 42).await.unwrap();
    f.rtc[1].reply(42).await;
}

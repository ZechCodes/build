use super::fixture::*;
use super::*;
use crate::http::{limits::TOTAL_BUFFER_BYTES, listener::OutputAuthority};
use futures_util::StreamExt;

/// Pass: composing HTTP shutdown and DataChannel carrier teardown refuses new
/// calls on every old opening, ends every stream, joins every stream task and
/// returns both pools inside the documented 5 s socket teardown allowance.
#[tokio::test]
async fn cross_transport_update_drain_fences_openings_and_joins_streams() {
    let mut f = Fixture::new(handler());
    f.fill_stream_pool().await;
    for client in &f.rtc {
        client.send(&f.intake, 1).await.unwrap();
    }
    for client in &mut f.rtc {
        client.reply(1).await;
    }
    let requests: Vec<_> = f.http.iter().map(|client| f.greeting(client)).collect();
    let mut streams = tokio::task::JoinSet::new();
    for client in f.http.drain(..) {
        streams.spawn(async move {
            let mut frames = client.events;
            assert!(matches!(frames.next().await, None | Some(Err(_))));
        });
    }
    tokio::task::yield_now().await;
    tokio::time::pause();
    let started = Instant::now();
    assert_eq!(f.runtime.shutdown(), POOL_STREAMS);
    f.close_rtc();
    assert_eq!(f.intake.retained_admitters(), 0);
    for client in &f.rtc {
        assert!(
            client.send(&f.intake, 2).await.is_err(),
            "drained channels cannot admit application traffic"
        );
    }
    for request in requests {
        let response = f.app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
    let request = open_request(Body::from(r#"{"api_range":">=3 <4"}"#));
    assert_eq!(
        f.app.clone().oneshot(request).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    tokio::time::timeout(crate::http::limits::SOCKET_TEARDOWN_TIMEOUT, async {
        while let Some(result) = streams.join_next().await {
            result.unwrap();
        }
    })
    .await
    .expect("every in-flight stream task joins inside teardown bound");
    assert!(streams.is_empty());
    assert_eq!(f.runtime.opening_count(), 0);
    assert_eq!(
        f.runtime.state.buffers.streams_available(),
        STREAM_BUFFER_BYTES
    );
    assert_eq!(f.runtime.state.buffers.available(), TOTAL_BUFFER_BYTES);
    assert_eq!(f.runtime.shutdown(), 0, "drain is idempotent");
    println!("cross transport composed drain: {POOL_STREAMS} streams + {RTC_SESSIONS} RTC sessions, {:?}, no retained stream tasks", started.elapsed());
}

/// Pass: all admitted output, including a response already held by the socket,
/// remains fenced after drain; a delayed flush cannot restore authority.
#[tokio::test]
async fn cross_transport_update_drain_fences_an_inflight_http_reply() {
    let mut f = Fixture::new(handler());
    f.fill_stream_pool().await;
    let request = f.greeting(&f.http[0]);
    let authority = request
        .extensions()
        .get::<OutputAuthority>()
        .unwrap()
        .clone();
    let response = f.app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(f.runtime.state.buffers.available() < TOTAL_BUFFER_BYTES);
    f.runtime.shutdown();
    f.close_rtc();
    let _ = json_response(response).await;
    use tokio::io::AsyncWriteExt;
    let mut io = crate::http::listener::BoundedIo::new(tokio::io::sink(), authority);
    assert_eq!(
        io.flush().await.unwrap_err().kind(),
        std::io::ErrorKind::PermissionDenied
    );
    drop(io);
    f.http.clear();
    assert_eq!(f.runtime.state.buffers.available(), TOTAL_BUFFER_BYTES);
    assert_eq!(
        f.runtime.state.buffers.streams_available(),
        STREAM_BUFFER_BYTES
    );
}

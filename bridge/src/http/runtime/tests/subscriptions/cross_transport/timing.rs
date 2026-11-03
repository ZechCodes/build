use super::fixture::*;
use super::*;
use futures_util::StreamExt;

/// Pass: silent authority polling emits nothing before the unchanged 15 s
/// heartbeat. Healthy streams retain their original payload cadence.
#[tokio::test(start_paused = true)]
async fn cross_transport_idle_authority_wakes_preserve_healthy_heartbeat_timing() {
    let f = Fixture::new(handler());
    let mut client = f.http_client("healthy-family").await;
    let next = client.events.next();
    tokio::pin!(next);
    assert!(futures_util::poll!(&mut next).is_pending());
    tokio::time::advance(Duration::from_millis(14_999)).await;
    assert!(futures_util::poll!(&mut next).is_pending());
    tokio::time::advance(Duration::from_millis(1)).await;
    let heartbeat = next.await.unwrap().unwrap();
    assert_eq!(heartbeat.as_ref(), b": keepalive\n\n");
}

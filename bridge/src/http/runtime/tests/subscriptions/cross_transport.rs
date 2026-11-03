//! Cross-transport acceptance criteria use the production HTTP router and the
//! negotiated DataChannel carrier boundary, sharing one execution service.
//! No installed daemon, relay, account service or external network is used.
use super::*;
use crate::http::limits::{MAX_FAMILY_STREAMS, STREAM_BUFFER_BYTES};
use std::time::Duration;

mod admission;
mod cancellation;
mod drain;
mod fixture;
mod load;
mod revocation;

struct Families {
    live: Arc<AtomicBool>,
}

#[async_trait]
impl GrantVerifier for Families {
    async fn verify(&self, bearer: &str) -> Result<VerifiedAuthority, AuthError> {
        let mut authority = authority_with(self.live.clone(), Duration::from_secs(60));
        authority.identity.family_id = bearer.to_owned();
        Ok(authority)
    }

    async fn revalidate(&self, bearer: &str) -> Result<VerifiedAuthority, AuthError> {
        self.verify(bearer).await
    }
}

/// Pass criterion: an idle SSE stream already waiting for its next event must
/// end within the documented 25 ms authority recheck after revocation, without
/// another request, push, client read wake or heartbeat to discover it.
#[tokio::test(start_paused = true)]
async fn cross_transport_idle_revocation_within_one_authority_recheck() {
    let live = Arc::new(AtomicBool::new(true));
    let runtime = runtime_with(execution(), Arc::new(Families { live: live.clone() }));
    let app = runtime.router();
    let opened = opened(&app).await;
    let session_id = opened["session_id"].as_str().unwrap();
    let subscription = subscription(&app, session_id).await;
    let mut events = stream(&app, session_id, &subscription).await;
    assert_eq!(next_event(&mut events).await.0, "hello");
    let ended = tokio::spawn(async move { futures_util::StreamExt::next(&mut events).await });
    tokio::task::yield_now().await;
    live.store(false, Ordering::SeqCst);
    let revoked = Instant::now();
    tokio::time::advance(Duration::from_millis(25)).await;
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
    let finished = ended.is_finished();
    let next = tokio::time::timeout(
        crate::http::sse::HEARTBEAT_INTERVAL + Duration::from_secs(1),
        ended,
    )
    .await
    .expect("idle stream eventually notices invalid authority")
    .unwrap();
    assert!(matches!(next, None | Some(Err(_))));
    println!(
        "idle SSE revocation observed after {:?}; required <=25ms",
        revoked.elapsed()
    );
    runtime.shutdown();
    assert_eq!(
        runtime.state.buffers.streams_available(),
        STREAM_BUFFER_BYTES
    );
    assert!(
        finished,
        "idle revocation exceeded one 25 ms AUTHORITY_RECHECK (family limit {MAX_FAMILY_STREAMS})"
    );
}

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

/// Pass: the mounted runtime's production reaper must end all 32 idle streams
/// and sessions within 25 ms of local authority revocation, alongside teardown
/// of four RTC carriers. No request, push or heartbeat discovers the change.
/// Warm up real TLS/listener setup first, then pause only the deadline phase.
#[tokio::test]
async fn cross_transport_idle_revocation_within_one_authority_recheck() {
    use fixture::{handler, Fixture, POOL_STREAMS};
    use futures_util::StreamExt;
    let mut f = Fixture::new(handler());
    f.fill_stream_pool().await;
    let reaped = Arc::new(AtomicUsize::new(0));
    let observed = reaped.clone();
    *f.runtime.state.reap_observer.lock().unwrap() = Some(Arc::new(move || {
        observed.fetch_add(1, Ordering::SeqCst);
    }));
    let mounted = mounted_tls(f.runtime.clone()).await;
    tokio::time::timeout(fixture::WATCHDOG, async {
        while reaped.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the production mounted reaper starts");
    let mut ended = tokio::task::JoinSet::new();
    for client in f.http.drain(..) {
        ended.spawn(async move {
            let mut events = client.events;
            assert!(matches!(events.next().await, None | Some(Err(_))));
        });
    }
    tokio::task::yield_now().await;
    tokio::time::pause();
    let revoked = Instant::now();
    f.live.store(false, Ordering::SeqCst);
    f.close_rtc();
    tokio::time::advance(Duration::from_millis(25)).await;
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
    let mut closed = 0;
    while let Some(result) = ended.try_join_next() {
        result.unwrap();
        closed += 1;
    }
    let finished = closed == POOL_STREAMS && f.runtime.opening_count() == 0;
    tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(result) = ended.join_next().await {
            result.unwrap();
        }
    })
    .await
    .expect("production mounted cleanup eventually ends every idle stream");
    let elapsed = revoked.elapsed();
    assert_eq!(f.runtime.opening_count(), 0);
    assert_eq!(
        f.runtime.state.buffers.streams_available(),
        STREAM_BUFFER_BYTES
    );
    tokio::time::resume();
    mounted.shutdown().await;
    println!("mounted runtime idle revocation: {POOL_STREAMS} streams ended after {elapsed:?}; required <=25ms");
    assert!(
        finished,
        "idle revocation exceeded one 25 ms AUTHORITY_RECHECK (family limit {MAX_FAMILY_STREAMS})"
    );
}

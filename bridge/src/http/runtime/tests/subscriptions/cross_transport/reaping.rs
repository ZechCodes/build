use super::fixture::*;
use super::*;
use futures_util::{task::ArcWake, StreamExt};
use std::future::Future;

/// A body's registered notification synchronously attempts the same cleanup
/// that body finish/drop runs. This deliberately exercises stronger reentry
/// than Tokio's ordinary queued task wake.
struct ReapOnWake {
    state: HttpState,
    wakes: AtomicUsize,
}

impl ArcWake for ReapOnWake {
    fn wake_by_ref(wake: &Arc<Self>) {
        wake.wakes.fetch_add(1, Ordering::SeqCst);
        wake.state.reap_ended();
    }
}

async fn reentrant_cleanup() {
    let f = Fixture::new(handler());
    let client = f.http_client("reentrant-family").await;
    let mut body = client.events;
    let wake = Arc::new(ReapOnWake {
        state: f.runtime.state.clone(),
        wakes: AtomicUsize::new(0),
    });
    {
        let next = body.next();
        tokio::pin!(next);
        let waker = futures_util::task::waker(wake.clone());
        let mut context = std::task::Context::from_waker(&waker);
        assert!(next.as_mut().poll(&mut context).is_pending());
        f.live.store(false, Ordering::SeqCst);
        assert_eq!(f.runtime.state.reap_ended(), 1);
        assert!(matches!(next.await, None | Some(Err(_))));
    }
    assert!(wake.wakes.load(Ordering::SeqCst) > 0);
    assert_eq!(f.runtime.opening_count(), 0);
    drop(body);
    assert_eq!(
        f.runtime.state.buffers.streams_available(),
        STREAM_BUFFER_BYTES
    );
}

/// Pass: reaper-driven retirement can synchronously notify cleanup, then the
/// pending body ends and returns its reservation. An external real-time
/// watchdog also catches a std-mutex deadlock that would freeze Tokio's clock.
#[test]
fn cross_transport_reaper_notification_can_reenter_cleanup() {
    let (settled, result) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(reentrant_cleanup());
        drop(runtime);
        settled.send(()).unwrap();
    });
    result
        .recv_timeout(WATCHDOG)
        .expect("reaper and body cleanup finish without a registry-lock deadlock");
    worker.join().unwrap();
}

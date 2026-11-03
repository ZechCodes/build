use super::fixture::*;
use super::*;
use crate::execution::CallContext;
use crate::http::limits::TOTAL_BUFFER_BYTES;
use std::sync::Condvar;
use tokio::sync::mpsc;

/// Controlled external work. The watchdog is real time even in paused tests;
/// dropping this guard always frees its blocking threads after an assertion.
pub(super) struct Gate {
    released: Arc<(Mutex<bool>, Condvar)>,
    pub entered: mpsc::UnboundedReceiver<CallContext>,
    pub handler: FrameHandler,
}

impl Gate {
    pub fn new() -> Self {
        let released = Arc::new((Mutex::new(false), Condvar::new()));
        let held = released.clone();
        let (entered, observed) = mpsc::unbounded_channel();
        let handler = FrameHandler::new(FrameClock::new(), move |sender, frame, _| {
            entered.send(sender.clone()).unwrap();
            let (lock, wake) = &*held;
            let (ready, _) = wake
                .wait_timeout_while(lock.lock().unwrap(), WATCHDOG, |ready| !*ready)
                .unwrap();
            assert!(*ready, "test controller releases external work");
            json!({"id": frame.payload["id"], "ok": true, "result": {}})
        })
        .with_greeting_reads(
            |epoch| json!({"api_version": crate::api::API_VERSION, "bridge_epoch": epoch}),
        );
        Self {
            released,
            entered: observed,
            handler,
        }
    }

    pub fn release(&self) {
        let (lock, wake) = &*self.released;
        *lock.lock().unwrap() = true;
        wake.notify_all();
    }

    pub async fn started(&mut self) -> CallContext {
        tokio::time::timeout(WATCHDOG, self.entered.recv())
            .await
            .unwrap()
            .unwrap()
    }
}

impl Drop for Gate {
    fn drop(&mut self) {
        self.release();
    }
}

/// Pass: cancellation on both adapters fences replies immediately, retains
/// running HTTP work's allowance until it actually settles, then restores all
/// 32 request slots and every buffer byte without disturbing other streams.
#[tokio::test]
async fn cross_transport_cancel_calls_retains_running_work_then_releases() {
    let mut gate = Gate::new();
    let mut f = Fixture::new(gate.handler.clone());
    f.fill_stream_pool().await;
    f.rtc[0].send(&f.intake, 1).await.unwrap();
    let rtc_context = gate.started().await;
    let request = f.greeting(&f.http[0]);
    let task = tokio::spawn(f.app.clone().oneshot(request));
    let http_context = gate.started().await;
    assert!(f.runtime.state.buffers.available() < TOTAL_BUFFER_BYTES);
    task.abort();
    let _ = task.await;
    f.intake.close_carrier(&f.rtc[0].carrier);
    assert!(!rtc_context.session_is_open());
    assert!(!http_context.read_is_live());
    assert!(
        f.runtime.state.buffers.available() < TOTAL_BUFFER_BYTES,
        "running work retains its body/output allowance after client cancellation"
    );
    drop(http_context);
    drop(rtc_context);
    gate.release();
    tokio::time::timeout(WATCHDOG, async {
        while f.runtime.state.buffers.available() != TOTAL_BUFFER_BYTES {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("settled work returns every reservation");
    f.rtc[1].send(&f.intake, 2).await.unwrap();
    f.rtc[1].reply(2).await;
    assert_eq!(
        f.runtime.state.buffers.streams_available(),
        0,
        "unrelated streams survive call cancellation"
    );
}

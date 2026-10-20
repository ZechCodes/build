use super::*;
use crate::carrier::testing::{client_request, session_init};
use crate::carrier::FrameHandler;
use crate::test_support::off_lock::OffLockGate;
use crate::timing::FrameClock;
use bytes::BytesMut;
use webrtc::data_channel::{RTCDataChannelId, RTCDataChannelMessage, RTCDataChannelState};
use webrtc::error::Result;

struct TestChannel {
    closed: AtomicBool,
    events: mpsc::UnboundedSender<DataChannelEvent>,
    incoming: tokio::sync::Mutex<mpsc::UnboundedReceiver<DataChannelEvent>>,
}

impl TestChannel {
    fn new() -> Arc<Self> {
        let (events, incoming) = mpsc::unbounded_channel();
        Arc::new(Self {
            closed: AtomicBool::new(false),
            events,
            incoming: tokio::sync::Mutex::new(incoming),
        })
    }
}

#[async_trait]
impl DataChannel for TestChannel {
    async fn label(&self) -> Result<String> {
        Ok("app".into())
    }
    async fn ordered(&self) -> Result<bool> {
        Ok(true)
    }
    async fn max_packet_life_time(&self) -> Result<Option<u16>> {
        Ok(None)
    }
    async fn max_retransmits(&self) -> Result<Option<u16>> {
        Ok(None)
    }
    async fn protocol(&self) -> Result<String> {
        Ok(String::new())
    }
    async fn negotiated(&self) -> Result<bool> {
        Ok(true)
    }
    fn id(&self) -> RTCDataChannelId {
        0
    }
    async fn ready_state(&self) -> Result<RTCDataChannelState> {
        Ok(if self.closed.load(Ordering::SeqCst) {
            RTCDataChannelState::Closed
        } else {
            RTCDataChannelState::Open
        })
    }
    async fn buffered_amount_high_threshold(&self) -> Result<u32> {
        Ok(0)
    }
    async fn set_buffered_amount_high_threshold(&self, _: u32) -> Result<()> {
        Ok(())
    }
    async fn buffered_amount_low_threshold(&self) -> Result<u32> {
        Ok(0)
    }
    async fn set_buffered_amount_low_threshold(&self, _: u32) -> Result<()> {
        Ok(())
    }
    async fn send(&self, _: BytesMut) -> Result<()> {
        Ok(())
    }
    async fn send_text(&self, _: &str) -> Result<()> {
        Ok(())
    }
    async fn poll(&self) -> Option<DataChannelEvent> {
        self.incoming.lock().await.recv().await
    }
    async fn close(&self) -> Result<()> {
        self.closed.store(true, Ordering::SeqCst);
        self.events.send(DataChannelEvent::OnClose).unwrap();
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn channel_close_releases_an_intake_parked_behind_sql_backlog() {
    let (gate, held) = OffLockGate::new();
    let handler = FrameHandler::new(FrameClock::new(), move |_, frame, _| {
        if frame.payload["params"]["hold"] == true {
            gate.arrive();
        }
        json!({"id":frame.payload["id"],"ok":true})
    });
    let transport = crate::transport::generate_transport_keypair();
    let intake = FrameIntake::new(handler, transport.clone());
    let (carrier, mut outbound) = CarrierHandle::open_channel();
    let key = crate::transport::generate_session_key();
    intake
        .open(
            "blocked",
            &session_init("blocked", &transport.public_key_b64, &key),
            &carrier,
        )
        .unwrap();
    outbound.try_recv().unwrap();
    for id in 0..24 {
        let request = client_request(
            &key,
            "blocked",
            "data",
            json!({"id":id,"method":"tasks.get","params":{"id":id,"hold":true}}),
        );
        intake.accept(request, &carrier).await.unwrap();
        if id < 8 {
            held.wait_for_arrival();
        }
        outbound.recv().await.unwrap(); // admission receipt
    }
    let channel = TestChannel::new();
    let extra = client_request(
        &key,
        "blocked",
        "data",
        json!({"id":25,"method":"tasks.list"}),
    );
    channel
        .events
        .send(DataChannelEvent::OnMessage(RTCDataChannelMessage {
            is_string: true,
            data: BytesMut::from(serde_json::to_string(&extra).unwrap().as_bytes()),
        }))
        .unwrap();
    let mut reader = tokio::spawn(pump_channel_events(
        channel.clone(),
        intake.clone(),
        carrier.clone(),
        "blocked".into(),
        "app",
        Arc::new(ChannelSend::default()),
    ));
    let receipt = tokio::time::timeout(Duration::from_secs(2), outbound.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(SessionSender::decrypt_push(&key, &receipt)["id"], 25);
    channel.close().await.unwrap();
    let ended = tokio::time::timeout(Duration::from_secs(1), &mut reader).await;
    reader.abort();
    assert!(
        ended.is_ok(),
        "closed channel must release even while its ordered intake is parked"
    );
    assert!(matches!(
        intake.accept(extra, &carrier).await,
        Err(crate::carrier::CarrierError::UnknownSession(_))
    ));
}

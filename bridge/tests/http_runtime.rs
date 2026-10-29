// Independent runtime modules compile before the shared-execution wiring handoff.
#[path = "../src/http/sse.rs"]
mod sse;

use serde_json::json;
use sse::{channel, DeliveryError, SseEvent, MAX_EVENT_BYTES, MAX_QUEUED_BYTES, MAX_QUEUED_EVENTS};

#[tokio::test]
async fn sse_inflight_output_stays_in_byte_and_item_budget() {
    let (sender, mut receiver) = channel();
    let event =
        SseEvent::json("changes", &json!({"data":"x".repeat(MAX_QUEUED_BYTES / 2)})).unwrap();
    let length = event.len();
    sender.try_send(event).unwrap();
    let inflight = receiver.next().await.unwrap();
    assert_eq!(sender.buffered(), (1, length));
    assert_eq!(
        sender.try_send(
            SseEvent::json("changes", &json!({"data":"x".repeat(MAX_QUEUED_BYTES / 2)})).unwrap()
        ),
        Err(DeliveryError::Full)
    );
    assert!(sender.is_closed());
    drop(inflight);
    let reset = receiver.next().await.unwrap();
    assert!(reset.as_str().contains("event: reset_required\n"));
    drop(reset);
    assert!(receiver.next().await.is_none());
    assert_eq!(sender.buffered(), (0, 0));
}

#[tokio::test]
async fn sse_event_overflow_explicitly_resets_and_closes() {
    let (sender, mut receiver) = channel();
    for _ in 0..MAX_QUEUED_EVENTS {
        sender
            .try_send(SseEvent::json("changes", &json!({"items":[]})).unwrap())
            .unwrap();
    }
    assert_eq!(sender.buffered().0, MAX_QUEUED_EVENTS);
    assert_eq!(
        sender.try_send(SseEvent::json("changes", &json!({})).unwrap()),
        Err(DeliveryError::Full)
    );
    let reset = receiver.next().await.unwrap();
    assert!(reset.as_str().contains("current_state_refresh"));
    drop(reset);
    assert!(receiver.next().await.is_none());
    assert_eq!(
        sender.try_send(SseEvent::json("changes", &json!({})).unwrap()),
        Err(DeliveryError::Closed)
    );
}

#[tokio::test]
async fn sse_close_discards_pending_output_and_receiver_drop_fences_producer() {
    let (sender, mut receiver) = channel();
    sender
        .try_send(SseEvent::json("changes", &json!({"private":true})).unwrap())
        .unwrap();
    sender.close();
    assert!(receiver.next().await.is_none());
    assert_eq!(sender.buffered(), (0, 0));
    let (sender, receiver) = channel();
    drop(receiver);
    assert_eq!(
        sender.try_send(SseEvent::heartbeat()),
        Err(DeliveryError::Closed)
    );
}

#[test]
fn sse_encoding_never_emits_cursor_and_rejects_oversized_or_unclassified_event() {
    let event = SseEvent::json(
        "hello",
        &json!({"durable":false,"recovery":"current_state_refresh"}),
    )
    .unwrap();
    assert_eq!(
        event.as_str(),
        "event: hello\ndata: {\"durable\":false,\"recovery\":\"current_state_refresh\"}\n\n"
    );
    assert!(!event.as_str().contains("id:"));
    assert_eq!(
        SseEvent::json("term.output", &json!({})),
        Err(DeliveryError::Unsupported)
    );
    assert_eq!(
        SseEvent::json("changes\nid: forged", &json!({})),
        Err(DeliveryError::Unsupported)
    );
    assert_eq!(
        SseEvent::json("changes", &json!({"data":"x".repeat(MAX_EVENT_BYTES)})),
        Err(DeliveryError::TooLarge)
    );
    assert_eq!(SseEvent::heartbeat().as_str(), ": keepalive\n\n");
}

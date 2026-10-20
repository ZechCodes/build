use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

fn handler() -> FrameHandler {
    FrameHandler::new(
        crate::timing::FrameClock::new(),
        |_, _, _| json!({"ok":true}),
    )
}

fn enqueue(
    queue: &ObservationQueue,
    sender: &SessionSender,
    id: usize,
    method: &str,
    params: Value,
) -> bool {
    let frame = serde_json::from_value(json!({"session_id":"s","message_id":format!("m{id}"),
        "frame_type":"data","sender":"client","created_at":"",
        "payload":{"id":id,"method":method,"params":params}}))
    .unwrap();
    let mut job = Some(Job {
        sender: sender.clone(),
        frame,
        queued: crate::timing::FrameClock::new().queued(),
    });
    let mut admitted = None;
    queue.admit(&mut job, &mut admitted)
}

struct DropWitness(Arc<AtomicUsize>);
impl Drop for DropWitness {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}
fn witnessed(drops: &Arc<AtomicUsize>) -> Execution {
    let witness = DropWitness(drops.clone());
    Box::new(move || drop(witness))
}

#[test]
fn preparing_and_ready_keep_every_folded_caller_charged_until_io_admission() {
    let queue = Arc::new(ObservationQueue::default());
    let sender = SessionSender::detached("s");
    for id in 0..2 {
        assert!(enqueue(&queue, &sender, id, "tasks.review.get", json!({})));
    }
    let (work, preparing) = queue.take_metadata(&handler()).unwrap();
    assert_eq!(work.callers.len(), 2);
    for id in 2..SQL_BACKLOG {
        assert!(enqueue(
            &queue,
            &sender,
            id,
            "tasks.review.get",
            json!({"unique":id})
        ));
    }
    assert_eq!(queue.depths(), (SQL_BACKLOG, 1));
    assert!(!enqueue(&queue, &sender, 99, "fs.list", json!({})));
    let drops = Arc::new(AtomicUsize::new(0));
    preparing.complete(witnessed(&drops));
    assert_eq!(queue.depths(), (SQL_BACKLOG, 1));
    assert!(!enqueue(&queue, &sender, 99, "fs.list", json!({})));
    let execution = queue.take_io(&handler()).unwrap();
    assert_eq!(queue.depths(), (SQL_BACKLOG - 2, 1));
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    drop(execution);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    assert!(enqueue(&queue, &sender, 99, "fs.list", json!({})));
    assert!(enqueue(&queue, &sender, 100, "fs.list", json!({})));
    assert_eq!(queue.depths(), (SQL_BACKLOG, 1));
}

#[test]
fn folding_stops_when_metadata_starts_and_preparing_heads_do_not_block_pure_jobs() {
    let queue = Arc::new(ObservationQueue::default());
    let sender = SessionSender::detached("s");
    for id in 0..2 {
        assert!(enqueue(&queue, &sender, id, "tasks.review.get", json!({})));
    }
    let (first, preparing) = queue.take_metadata(&handler()).unwrap();
    assert_eq!(first.callers.len(), 2);
    assert!(enqueue(&queue, &sender, 2, "tasks.review.get", json!({})));
    assert!(enqueue(&queue, &sender, 3, "fs.list", json!({})));
    assert!(
        queue.take_io(&handler()).is_some(),
        "pure job can pass a Preparing entry"
    );
    let (second, _second_preparing) = queue.take_metadata(&handler()).unwrap();
    assert_eq!(
        second.callers.len(),
        1,
        "late callers never join running metadata"
    );
    preparing.complete(Box::new(|| {}));
    assert!(queue.take_io(&handler()).is_some());
}

#[test]
fn ready_publication_does_not_jump_other_eligible_openings() {
    let queue = Arc::new(ObservationQueue::default());
    let a = SessionSender::detached("a");
    let b = SessionSender::detached("b");
    assert!(enqueue(&queue, &a, 1, "tasks.review.get", json!({})));
    let (_work, preparing) = queue.take_metadata(&handler()).unwrap();
    assert!(enqueue(&queue, &b, 2, "fs.list", json!({})));
    preparing.complete(Box::new(|| {}));
    let waiting = queue.waiting.lock().unwrap();
    assert_eq!(
        waiting.io.iter().copied().collect::<Vec<_>>(),
        vec![b.still_open.ordinal, a.still_open.ordinal]
    );
}

#[test]
fn pruning_ready_work_drops_owned_inputs_and_admission_leases_immediately() {
    let queue = Arc::new(ObservationQueue::default());
    let sender = SessionSender::detached("s");
    assert!(enqueue(&queue, &sender, 1, "tasks.review.get", json!({})));
    let (_work, preparing) = queue.take_metadata(&handler()).unwrap();
    let drops = Arc::new(AtomicUsize::new(0));
    preparing.complete(witnessed(&drops));
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    sender.still_open.is_open.store(false, Ordering::SeqCst);
    queue.prune();
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    assert_eq!(queue.depths(), (0, 0));
}

#[tokio::test]
async fn closing_during_preparation_discards_late_results_without_recreating_queue_slots() {
    let queue = Arc::new(ObservationQueue::default());
    let sender = SessionSender::detached("s");
    assert!(enqueue(&queue, &sender, 1, "tasks.review.get", json!({})));
    let (_work, preparing) = queue.take_metadata(&handler()).unwrap();
    let token = preparing.token.clone();
    let opening = preparing.opening;
    let (done, complete) = oneshot::channel();
    queue.close();
    assert!(preparing.receive(complete).await.is_none());
    let drops = Arc::new(AtomicUsize::new(0));
    assert!(done.send(witnessed(&drops)).is_err());
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    queue.complete(opening, &token, witnessed(&drops));
    assert_eq!(drops.load(Ordering::SeqCst), 2);
    assert_eq!(queue.depths(), (0, 0));
}

#[tokio::test]
async fn aborted_or_failed_preparation_releases_its_admission_reservation() {
    let queue = Arc::new(ObservationQueue::default());
    let sender = SessionSender::detached("s");
    assert!(enqueue(&queue, &sender, 1, "tasks.review.get", json!({})));
    let (_work, preparing) = queue.take_metadata(&handler()).unwrap();
    let (done, complete) = oneshot::channel();
    drop(done);
    assert!(preparing.receive(complete).await.is_none());
    drop(preparing);
    assert_eq!(queue.depths(), (0, 0));
    assert!(enqueue(&queue, &sender, 2, "tasks.review.get", json!({})));
    let (_work, preparing) = queue.take_metadata(&handler()).unwrap();
    drop(preparing);
    assert_eq!(queue.depths(), (0, 0));
}

#[test]
fn old_preparation_cannot_publish_into_a_reopened_session() {
    let queue = Arc::new(ObservationQueue::default());
    let old = SessionSender::detached("same-session");
    assert!(enqueue(&queue, &old, 1, "tasks.review.get", json!({})));
    let (_work, preparing) = queue.take_metadata(&handler()).unwrap();
    old.still_open.is_open.store(false, Ordering::SeqCst);
    queue.prune();
    let new = SessionSender::detached("same-session");
    assert!(enqueue(&queue, &new, 2, "tasks.review.get", json!({})));
    let drops = Arc::new(AtomicUsize::new(0));
    preparing.complete(witnessed(&drops));
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    assert_eq!(queue.depths(), (1, 1));
    assert!(queue
        .waiting
        .lock()
        .unwrap()
        .openings
        .contains_key(&new.still_open.ordinal));
}

#[tokio::test]
async fn metadata_waiters_keep_their_fair_position_across_pure_traffic_and_pruning() {
    use std::future::Future;
    use std::task::Poll;
    let queue = Arc::new(ObservationQueue::default());
    let sender = SessionSender::detached("s");
    assert!(enqueue(&queue, &sender, 1, "tasks.review.get", json!({})));
    let permits = Arc::new(Semaphore::new(0));
    let metadata = queue.while_pending(permits.clone().acquire_owned());
    tokio::pin!(metadata);
    assert!(std::future::poll_fn(|cx| Poll::Ready(metadata.as_mut().poll(cx).is_pending())).await);
    let ordinary = permits.clone().acquire_owned();
    tokio::pin!(ordinary);
    assert!(std::future::poll_fn(|cx| Poll::Ready(ordinary.as_mut().poll(cx).is_pending())).await);
    for id in 2..18 {
        assert!(enqueue(
            &queue,
            &sender,
            id,
            "fs.list",
            json!({"unique":id})
        ));
        assert!(queue.take_io(&handler()).is_some());
        queue.prune();
        assert!(
            std::future::poll_fn(|cx| Poll::Ready(metadata.as_mut().poll(cx).is_pending())).await
        );
    }
    permits.add_permits(1);
    let first = std::future::poll_fn(|cx| Poll::Ready(metadata.as_mut().poll(cx))).await;
    let Poll::Ready(Some(Ok(first))) = first else {
        panic!("metadata lost its fair semaphore position")
    };
    assert!(std::future::poll_fn(|cx| Poll::Ready(ordinary.as_mut().poll(cx).is_pending())).await);
    drop(first);
    let _ordinary = ordinary.await.unwrap();
}

#[tokio::test]
async fn pure_io_does_not_broadcast_to_sql_resource_waiters() {
    use std::future::Future;
    use std::task::Poll;
    let queue = Arc::new(ObservationQueue::default());
    let sender = SessionSender::detached("s");
    let lifecycle = queue.lifecycle.notified();
    let capacity = queue.changed.notified();
    tokio::pin!(lifecycle, capacity);
    lifecycle.as_mut().enable();
    capacity.as_mut().enable();
    assert!(enqueue(&queue, &sender, 1, "fs.list", json!({})));
    assert!(queue.take_io(&handler()).is_some());
    assert!(std::future::poll_fn(|cx| Poll::Ready(lifecycle.as_mut().poll(cx).is_pending())).await);
    assert!(std::future::poll_fn(|cx| Poll::Ready(capacity.as_mut().poll(cx).is_ready())).await);
}

#[tokio::test]
async fn a_pure_only_queue_never_polls_sql_resource_checkout() {
    let queue = Arc::new(ObservationQueue::default());
    assert!(enqueue(
        &queue,
        &SessionSender::detached("s"),
        1,
        "fs.list",
        json!({})
    ));
    let result: Option<()> = queue
        .while_pending(async { panic!("pure reads cannot trigger SQL checkout") })
        .await;
    assert!(result.is_none());
}

#[test]
fn duplicate_ready_completion_does_not_remove_the_original_prepared_job() {
    let queue = Arc::new(ObservationQueue::default());
    let sender = SessionSender::detached("s");
    assert!(enqueue(&queue, &sender, 1, "tasks.review.get", json!({})));
    let (_work, preparing) = queue.take_metadata(&handler()).unwrap();
    let opening = preparing.opening;
    let token = preparing.token.clone();
    let original = Arc::new(AtomicUsize::new(0));
    let duplicate = Arc::new(AtomicUsize::new(0));
    preparing.complete(witnessed(&original));
    queue.complete(opening, &token, witnessed(&duplicate));
    assert_eq!(duplicate.load(Ordering::SeqCst), 1);
    assert_eq!(original.load(Ordering::SeqCst), 0);
    assert_eq!(queue.depths(), (1, 1));
    drop(queue.take_io(&handler()).unwrap());
    assert_eq!(original.load(Ordering::SeqCst), 1);
}

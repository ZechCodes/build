//! Actual MCP codec/ownership control, not authenticated mutation evidence.
use super::*;
use crate::app::tests::tracker::tracked;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn actual_mcp_decoded_notification_precedes_owned_socket_eof_tail() {
    let root = tempfile::tempdir().unwrap();
    let (_repository, app, _project) = tracked(root.path());
    let state = app.shared();
    let released = Arc::downgrade(&state);
    let registry = WriteWaitRegistry::new();
    let plane = {
        let app = state.lock().unwrap();
        ControlPlane::new(&app)
    };
    let original = "qual/mcp-codec-only/write/00000";
    let watch = ClientWatch::new(Arc::new(Instant::now));
    watch.register_at(original, Instant::now());
    let watch_guard = watch.watch();
    let (send_event, mut events) = tokio::sync::mpsc::unbounded_channel();
    watch
        .observe_decoded_replies(Arc::new(move |id, at| {
            send_event.send((id.to_owned(), at)).unwrap();
        }))
        .unwrap();
    watch.offer(original);
    let endpoint = Arc::new(Endpoint {
        // This direct codec fixture does not claim native credential admission.
        identity: McpIdentity {
            agent: "codec-only-endpoint".into(),
            token: "unused-codec-fixture".into(),
            read_only: false,
        },
        sockets: Arc::new(Semaphore::new(SOCKETS_PER_IDENTITY)),
        active: AtomicUsize::new(0),
        maximum_active: AtomicUsize::new(0),
    });
    let inner = Arc::new(Inner {
        state,
        registry,
        plane,
        endpoints: vec![Arc::clone(&endpoint)],
        watch: watch.clone(),
        jobs: Mutex::new(Jobs {
            accepting: true,
            owned: BTreeMap::new(),
            imported: BTreeMap::new(),
            evidence: McpEvidence {
                calls: BTreeMap::from([(original.into(), codec_row(original))]),
                ..Default::default()
            },
        }),
        active: Arc::new(arc_swap::ArcSwap::from_pointee(imbl::OrdMap::new())),
        failed: Arc::new(AtomicBool::new(false)),
    });
    let (unused_sender, _unused_receiver) = oneshot::channel();
    let worker = RequestWorker {
        inner,
        endpoint,
        original: original.into(),
        method: "codec-only-final".into(),
        bytes: b"{\"codec_control\":true}\n".to_vec(),
        send: unused_sender,
    };
    let (client, peer) = UnixStream::pair().unwrap();
    let (release_eof, held_eof) = oneshot::channel::<()>();
    let peer = tokio::spawn(async move {
        let mut peer = BufReader::new(peer);
        let mut request = String::new();
        peer.read_line(&mut request).await.unwrap();
        let response = crate::app::mcp::mcp_action_response(Ok(json!({
            "scope":"codec-only-no-semantic-effect-claim"
        })));
        peer.get_mut()
            .write_all(format!("{response}\n").as_bytes())
            .await
            .unwrap();
        peer.get_mut().flush().await.unwrap();
        held_eof.await.unwrap();
        drop(peer);
    });
    let exchange =
        tokio::spawn(async move { worker.exchange_inner(&mut BufReader::new(client)).await });
    // The watch settles just before it notifies. Wait on the event itself so
    // another runtime worker cannot observe settlement in that small interval.
    let observed = tokio::time::timeout(Duration::from_secs(2), events.recv()).await;
    let parsed_before_release = watch.snapshot()[0].final_at.is_some();
    let exchange_still_owned = !exchange.is_finished();
    let peer_still_owned = !peer.is_finished();
    // Release and join both real socket tasks before a missing-event RED.
    release_eof.send(()).unwrap();
    let actual = exchange.await.unwrap();
    peer.await.unwrap();
    let watch_report = watch_guard.finish();
    let until = Instant::now() + Duration::from_secs(10);
    while released.upgrade().is_some() && Instant::now() < until {
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    assert!(
        released.upgrade().is_none(),
        "codec fixture app remains owned"
    );
    assert!(!watch_report.panicked);
    let actual = actual.unwrap();
    assert!(parsed_before_release && exchange_still_owned && peer_still_owned);
    let (id, at) = observed
        .expect("actual MCP decoded final must notify before EOF/server-tail join")
        .expect("decoded observer stays owned through the socket tail");
    assert_eq!(id, original);
    assert_eq!(at, actual.final_reply_at);
    assert!(events.try_recv().is_err(), "one exact decoded notification");
}

fn codec_row(original: &str) -> McpCallRow {
    McpCallRow {
        original_id: original.into(),
        endpoint: 0,
        method: "codec-only-final".into(),
        read: false,
        queued_at: Instant::now(),
        socket_wait_started_at: None,
        socket_acquired_at: None,
        sent_at: None,
        final_reply_at: None,
        response_ok: None,
        response_bytes: None,
        response_sha256: None,
        actual_wire_id: None,
        client_closed_at: None,
        server_joined_at: None,
        worker_finished_at: None,
        worker_reaped_at: None,
        abandoned: false,
        error: None,
    }
}

use super::*;

// ---- rtc.* signaling (spec §Signaling) ---------------------------------

/// A bridge whose peer connections are recorded rather than negotiated: the
/// shared state, its handler, and the factory a test reads to see what
/// reached the peer.
pub(in crate::app::tests) fn signaling_fixture(
    repo: &std::path::Path,
    dir: &std::path::Path,
) -> (
    Arc<Mutex<AppState>>,
    FrameHandler,
    Arc<crate::rtc::recording::RecordingPeerFactory>,
) {
    let factory = crate::rtc::recording::RecordingPeerFactory::new();
    let mut app = AppState::new(
        repo.to_path_buf(),
        dir.join("wt"),
        "main",
        true,
        "/tmp/test-mcp.sock",
    );
    app.set_peer_factory(factory.clone());
    let state = app.shared();
    let handler = AppState::handler(Arc::clone(&state));
    (state, handler, factory)
}

/// Run one frame the way a carrier does: on a blocking thread of the
/// runtime, which is where a handler may finish a peer's async work.
async fn signal(
    handler: &FrameHandler,
    sender: &SessionSender,
    method: &str,
    params: Value,
) -> Value {
    let handler = handler.clone();
    let sender = sender.clone();
    let frame = req(method, params);
    tokio::task::spawn_blocking(move || handler.call(sender, frame))
        .await
        .expect("the handler finished")
}

pub(in crate::app::tests) fn offer(sdp: &str) -> Value {
    json!({
        "sdp": sdp,
        "ice_servers": [{ "urls": "stun:stun.cloudflare.com:3478" }],
    })
}

/// The whole negotiation as the browser drives it: an offer answered from
/// the ICE servers it fetched, its candidates trickled to the bridge, and
/// the bridge's own trickled back over the carrier the offer arrived on.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_offer_is_answered_and_candidates_trickle_both_ways() {
    let (dir, repo) = init_repo();
    let (_state, handler, factory) = signaling_fixture(&repo, dir.path());
    let (sender, mut pushes, key) = SessionSender::observable("s-peer");

    let answered = signal(&handler, &sender, "rtc.offer", offer("v=0 browser")).await;

    assert_eq!(answered["ok"], true, "{answered:?}");
    assert_eq!(answered["result"]["sdp"], "answer-to:v=0 browser");
    let peer = factory
        .peer_of("s-peer")
        .expect("the session opened a peer");
    assert_eq!(
        peer.offers(),
        vec![(
            "v=0 browser".to_string(),
            vec![json!({ "urls": "stun:stun.cloudflare.com:3478" })]
        )],
        "the offer reached the peer with the ICE servers the browser fetched"
    );

    let trickled = signal(
        &handler,
        &sender,
        "rtc.ice",
        json!({ "candidate": { "candidate": "candidate:1 1 udp", "sdpMid": "0" } }),
    )
    .await;
    assert_eq!(trickled["ok"], true, "{trickled:?}");
    assert_eq!(trickled["result"], json!({}));
    assert_eq!(
        peer.remote_candidates(),
        vec![json!({ "candidate": "candidate:1 1 udp", "sdpMid": "0" })]
    );

    assert!(peer.trickle(json!({ "candidate": "candidate:2 1 udp" })));
    let pushed = SessionSender::decrypt_push(
        &key,
        &pushes
            .try_recv()
            .expect("the bridge's candidate was pushed"),
    );
    assert_eq!(
        pushed,
        json!({ "type": "rtc.ice", "candidate": { "candidate": "candidate:2 1 udp" } })
    );
}

/// One peer per E2EE session: a second offer reconfigures the peer the
/// first one built (that is the ICE restart fresh TURN credentials arrive
/// on), never a second peer for the same session.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_second_offer_reconfigures_the_one_peer_the_session_has() {
    let (dir, repo) = init_repo();
    let (state, handler, factory) = signaling_fixture(&repo, dir.path());
    let (sender, _pushes, _key) = SessionSender::observable("s-restart");

    let first = signal(&handler, &sender, "rtc.offer", offer("v=0 first")).await;
    let restarted = signal(
        &handler,
        &sender,
        "rtc.offer",
        json!({ "sdp": "v=0 restart", "ice_servers": [{ "urls": "turn:turn.example:3478" }] }),
    )
    .await;

    assert_eq!(first["result"]["sdp"], "answer-to:v=0 first");
    assert_eq!(restarted["result"]["sdp"], "answer-to:v=0 restart");
    assert_eq!(factory.opened_count(), 1, "one session, one peer");
    assert_eq!(state.lock().unwrap().peers().count(), 1);
    let peer = factory
        .peer_of("s-restart")
        .expect("the session has a peer");
    assert_eq!(
        peer.offers(),
        vec![
            (
                "v=0 first".to_string(),
                vec![json!({ "urls": "stun:stun.cloudflare.com:3478" })]
            ),
            (
                "v=0 restart".to_string(),
                vec![json!({ "urls": "turn:turn.example:3478" })]
            ),
        ],
        "the restart carries its own ICE servers to the same peer"
    );
}

/// A candidate for a session that never offered has nowhere to go: it is
/// refused rather than opening a peer nothing negotiated.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_candidate_before_any_offer_is_refused() {
    let (dir, repo) = init_repo();
    let (_state, handler, factory) = signaling_fixture(&repo, dir.path());
    let (sender, _pushes, _key) = SessionSender::observable("s-early");

    let early = signal(
        &handler,
        &sender,
        "rtc.ice",
        json!({ "candidate": { "candidate": "candidate:1 1 udp" } }),
    )
    .await;

    assert_eq!(early["ok"], false, "{early:?}");
    assert_eq!(early["error"], "no peer connection for session s-early");
    assert_eq!(factory.opened_count(), 0, "no peer was built to hold it");
}

/// The same for a close: a session with no peer connection has nothing to
/// tear down, and saying so leaves it free to offer afterwards.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_close_before_any_offer_is_refused_and_the_session_can_still_offer() {
    let (dir, repo) = init_repo();
    let (_state, handler, factory) = signaling_fixture(&repo, dir.path());
    let (sender, _pushes, _key) = SessionSender::observable("s-nothing");

    let nothing = signal(&handler, &sender, "rtc.close", json!({})).await;

    assert_eq!(nothing["ok"], false, "{nothing:?}");
    assert_eq!(nothing["error"], "no peer connection for session s-nothing");

    let answered = signal(&handler, &sender, "rtc.offer", offer("v=0 later")).await;
    assert_eq!(answered["result"]["sdp"], "answer-to:v=0 later");
    assert_eq!(factory.opened_count(), 1);
}

/// A close that arrives while the offer is still being answered wins: the
/// browser gave up on the upgrade, so the peer is closed and the session is
/// left with none — a live peer nobody asked for would keep an ICE
/// negotiation running for the life of the session.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_close_during_an_offer_leaves_no_peer_behind() {
    let (dir, repo) = init_repo();
    let (state, handler, factory) = signaling_fixture(&repo, dir.path());
    let gate = factory.hold_answers();
    let (sender, _pushes, _key) = SessionSender::observable("s-abandoned");

    let offering = tokio::spawn({
        let handler = handler.clone();
        let sender = sender.clone();
        async move { signal(&handler, &sender, "rtc.offer", offer("v=0 abandoned")).await }
    });
    gate.wait_until_answering().await;

    let closed = signal(&handler, &sender, "rtc.close", json!({})).await;
    gate.release();
    let answered = offering.await.expect("the offer was answered");

    assert_eq!(closed["ok"], true, "{closed:?}");
    assert_eq!(answered["ok"], true, "{answered:?}");
    let peer = factory
        .peer_of("s-abandoned")
        .expect("the offer opened a peer");
    assert!(peer.is_closed(), "the peer the close took is torn down");
    assert!(
        state.lock().unwrap().peers().count() == 0,
        "the answer does not put a closed peer back"
    );
}

/// A session's end is its peer's end: the close frame takes the peer out
/// and tears it down, so no ICE negotiation outlives the session that asked
/// for it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_end_of_a_session_tears_down_its_peer() {
    let (dir, repo) = init_repo();
    let (state, handler, factory) = signaling_fixture(&repo, dir.path());
    let (sender, _pushes, _key) = SessionSender::observable("s-ending");
    signal(&handler, &sender, "rtc.offer", offer("v=0 ending")).await;
    let peer = factory.peer_of("s-ending").expect("the session has a peer");

    let close = Frame {
        session_id: "s-ending".into(),
        message_id: "m".into(),
        frame_type: transport::CLOSE_FRAME_TYPE.into(),
        sender: "client".into(),
        created_at: "t".into(),
        payload: Value::Null,
    };
    let closing = handler.clone();
    let closing_sender = sender.clone();
    tokio::task::spawn_blocking(move || closing.call(closing_sender, close))
        .await
        .expect("the close ran");

    assert_eq!(state.lock().unwrap().peers().count(), 0);
    tokio::time::timeout(Duration::from_secs(5), peer.closed())
        .await
        .expect("the session's end closed its peer");
}

/// An offer the peer connection cannot answer leaves the session with no
/// peer at all: the browser's retry builds a fresh one rather than being
/// routed back to the half-open peer that just failed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_offer_the_peer_cannot_answer_leaves_the_session_with_no_peer() {
    let (dir, repo) = init_repo();
    let (state, handler, factory) = signaling_fixture(&repo, dir.path());
    factory.fail_answers();
    let (sender, _pushes, _key) = SessionSender::observable("s-refused");

    let refused = signal(&handler, &sender, "rtc.offer", offer("v=0 refused")).await;

    assert_eq!(refused["ok"], false, "{refused:?}");
    assert_eq!(
        refused["error"],
        "the peer connection refused the offer: v=0 refused"
    );
    assert!(
        state.lock().unwrap().peers().count() == 0,
        "a peer that never negotiated is not the session's"
    );
    let failed = factory.peer_of("s-refused").expect("a peer was opened");
    tokio::time::timeout(Duration::from_secs(5), failed.closed())
        .await
        .expect("the peer that could not answer was torn down");

    signal(&handler, &sender, "rtc.offer", offer("v=0 retry")).await;
    assert_eq!(
        factory.opened_count(),
        2,
        "the retry builds a fresh peer, not the dead one"
    );
}

/// A bridge with no peer transport built in refuses the offer, and the
/// client stays on the relay carrier.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_bridge_with_no_peer_transport_refuses_the_offer() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_state_and_handler(&repo, dir.path());
    let (sender, _pushes, _key) = SessionSender::observable("s-relay-only");

    let refused = signal(&handler, &sender, "rtc.offer", offer("v=0 hopeful")).await;

    assert_eq!(refused["ok"], false, "{refused:?}");
    assert_eq!(refused["error"], "this bridge has no peer transport");
    assert_eq!(state.lock().unwrap().peers().count(), 0);
}

/// The offer's two params are both required. What the list they carry has
/// to contain is the peer's question, not this handler's: a browser whose
/// api answered nothing still offers, and host candidates still pair.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_offer_without_an_sdp_or_ice_servers_is_refused() {
    let (dir, repo) = init_repo();
    let (_state, handler, factory) = signaling_fixture(&repo, dir.path());
    let (sender, _pushes, _key) = SessionSender::observable("s-malformed");

    let no_sdp = signal(&handler, &sender, "rtc.offer", json!({ "ice_servers": [] })).await;
    let no_servers = signal(&handler, &sender, "rtc.offer", json!({ "sdp": "v=0" })).await;

    assert_eq!(no_sdp["error"], "missing required param: sdp");
    assert_eq!(no_servers["error"], "missing required param: ice_servers");
    assert_eq!(factory.opened_count(), 0);
}

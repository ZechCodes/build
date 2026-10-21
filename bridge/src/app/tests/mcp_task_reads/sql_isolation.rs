//! A saturated SQL tool cannot consume the capacity of other read types.
use super::*;
use crate::app::mcp::{SqlReadProbe, SqlReadProbeEvent};
use guardfree::held::{concurrent_group, Request};

const PINNED_PER_SLOW_TYPE: usize = 2;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mcp_one_pinned_sql_type_leaves_other_reads_with_capacity() {
    isolated_reads(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mcp_two_pinned_sql_types_leave_four_fast_sql_reads_with_capacity() {
    isolated_reads(true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelled_sql_query_keeps_its_class_permit_until_the_worker_finishes() {
    let mut fixture = Fixture::new();
    let (probe, mut pins) = SqlReadProbe::new(vec!["get_task"]);
    fixture.plane = ControlPlane::with_sql_probe(&fixture.state.lock().unwrap(), probe);
    let mut sockets = Vec::new();
    for _ in 0..PINNED_PER_SLOW_TYPE {
        sockets.push(begin_socket(&fixture).await);
        assert_eq!(
            tokio::time::timeout(WATCHDOG, pins.next()).await.unwrap(),
            SqlReadProbeEvent::Pinned("get_task")
        );
    }
    // Aborting a caller cannot give back the permit owned by its still-pinned
    // blocking query. The next caller must remain outside the SQL pool.
    let (client, server) = sockets.remove(0);
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
    drop(client);
    let (client, server) = begin_socket(&fixture).await;
    assert_eq!(
        tokio::time::timeout(WATCHDOG, pins.next()).await.unwrap(),
        SqlReadProbeEvent::Waiting("get_task")
    );
    // Cancellation while queued must remove its place and retain no permit.
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
    drop(client);
    let reading = start(&fixture, fixture.plane.clone(), "", fixture.get());
    assert_eq!(
        tokio::time::timeout(WATCHDOG, pins.next()).await.unwrap(),
        SqlReadProbeEvent::Waiting("get_task")
    );
    drop(pins);
    let reply = tokio::time::timeout(WATCHDOG, reading)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reply["ok"], true, "{reply}");
    // Close the remaining socket only after its worker is allowed to finish.
    for (client, server) in sockets {
        drop(client);
        tokio::time::timeout(WATCHDOG, server)
            .await
            .unwrap()
            .unwrap();
    }
}

async fn begin_socket(fixture: &Fixture) -> (tokio::net::UnixStream, tokio::task::JoinHandle<()>) {
    let (mut client, daemon) = tokio::net::UnixStream::pair().unwrap();
    let server = tokio::spawn(handle_done_stream(
        fixture.state.clone(),
        daemon,
        fixture.plane.clone(),
    ));
    client
        .write_all(format!("{}\n", fixture.frame(fixture.get())).as_bytes())
        .await
        .unwrap();
    (client, server)
}

async fn isolated_reads(two_types: bool) {
    let mut fixture = Fixture::new();
    history_reviews::history(&fixture);
    let comment = history_reviews::comment(&fixture);
    let review = history_reviews::review_actions(&fixture).remove(1);
    // The legacy deferred handler may drain and retire a fixture router;
    // capture its wire oracle before creating the router used by these reads.
    let expected_review = history_reviews::legacy_review(&fixture, review.clone()).await;
    let (_, router) = snapshot_reads::router(&fixture);
    let mut slow = vec![fixture.get()];
    if two_types {
        slow.push(BridgeAction::ReadConversation {
            entity_id: fixture.owner.clone(),
            agent_id: Some(fixture.agent.clone()),
            limit: 5,
        });
    }
    let fast = fast_requests(&fixture, &router, comment, review, expected_review).await;
    let (probe, mut pins) = SqlReadProbe::new(slow.iter().map(BridgeAction::tool_name).collect());
    let other_plane = {
        let app = fixture.state.lock().unwrap();
        fixture.plane = ControlPlane::with_sql_probe(&app, probe.clone());
        ControlPlane::with_sql_probe(&app, probe)
    };
    let mut readings = Vec::new();
    for action in slow {
        for _ in 0..PINNED_PER_SLOW_TYPE {
            readings.push(start(
                &fixture,
                fixture.plane.clone(),
                &router,
                action.clone(),
            ));
            assert_eq!(
                tokio::time::timeout(WATCHDOG, pins.next()).await.unwrap(),
                SqlReadProbeEvent::Pinned(action.tool_name())
            );
        }
        // Independently captured planes must share the same per-pool quota.
        readings.push(start(
            &fixture,
            other_plane.clone(),
            &router,
            action.clone(),
        ));
        assert_eq!(
            tokio::time::timeout(WATCHDOG, pins.next()).await.unwrap(),
            SqlReadProbeEvent::Waiting(action.tool_name()),
            "a third {} borrowed SQL capacity instead of waiting for its type quota",
            action.tool_name()
        );
        for _ in PINNED_PER_SLOW_TYPE + 1..SQL_READERS {
            readings.push(start(
                &fixture,
                other_plane.clone(),
                &router,
                action.clone(),
            ));
            assert_eq!(
                tokio::time::timeout(WATCHDOG, pins.next()).await.unwrap(),
                SqlReadProbeEvent::Waiting(action.tool_name())
            );
        }
    }
    // All fast reads start together while AppState and both writer turns are
    // held, with each decoded socket reply required within 100ms.
    concurrent_group(&fixture, fast, || drop(pins)).await;
    for reading in readings {
        let reply = tokio::time::timeout(WATCHDOG, reading)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(reply["ok"], true, "{reply}");
    }
}

fn start(
    fixture: &Fixture,
    plane: Arc<ControlPlane>,
    router: &str,
    action: BridgeAction,
) -> tokio::task::JoinHandle<Value> {
    let frame = if matches!(action, BridgeAction::ReadConversation { .. }) {
        snapshot_reads::router_frame(router, action)
    } else {
        fixture.frame(action)
    };
    tokio::spawn(socket_reply(fixture.state.clone(), plane, frame))
}

async fn fast_requests(
    fixture: &Fixture,
    router: &str,
    comment_id: String,
    review: BridgeAction,
    expected_review: Value,
) -> Vec<Request> {
    let mut requests = Vec::new();
    for action in [
        BridgeAction::TrackerTaskDependents {
            task_id: fixture.task.clone(),
            limit: Some(1),
            cursor: None,
        },
        BridgeAction::SearchConversation {
            query: crate::thread::ConversationQuery {
                text: Some("buried history witness".into()),
                ..Default::default()
            },
        },
        review,
        BridgeAction::TrackerReadComment { comment_id },
        BridgeAction::ListWorkspaces,
    ] {
        let expected = if matches!(action, BridgeAction::TrackerReadReview { .. }) {
            expected_review.clone()
        } else {
            fixture.reply(fixture.frame(action.clone())).await
        };
        assert_eq!(expected["ok"], true, "{}: {expected}", action.tool_name());
        requests.push(Request::new(fixture.frame(action), expected));
    }
    let frame = snapshot_reads::router_frame(router, BridgeAction::ListWork);
    let expected = fixture.reply(frame.clone()).await;
    assert_eq!(expected["ok"], true, "{expected}");
    requests.push(Request::new(frame, expected));
    requests
}

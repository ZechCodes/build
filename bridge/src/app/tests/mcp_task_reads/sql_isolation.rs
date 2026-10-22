//! A saturated SQL tool cannot consume the capacity of other read types.
use super::*;
use crate::app::mcp::{SqlReadProbe, SqlReadProbeEvent};
use guardfree::held::{concurrent_group, Request};

const PINNED_PER_SLOW_TYPE: usize = 2;
const PINNED_TASK_PAGES: usize = 3;
const TASK_PAGE_ROWS: usize = 20;

#[derive(Clone, Copy)]
enum ReadLoad {
    OneSlowType,
    TwoSlowTypes,
    TwoSlowTypesWithPages,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mcp_one_pinned_sql_type_leaves_other_reads_with_capacity() {
    isolated_reads(ReadLoad::OneSlowType).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mcp_two_pinned_sql_types_leave_four_fast_sql_reads_with_capacity() {
    isolated_reads(ReadLoad::TwoSlowTypes).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mcp_three_bounded_task_pages_leave_fast_reads_capacity_beside_two_slow_types() {
    isolated_reads(ReadLoad::TwoSlowTypesWithPages).await;
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

async fn isolated_reads(load: ReadLoad) {
    let mut fixture = Fixture::new();
    seed_task_pages(&fixture, load);
    history_reviews::history(&fixture);
    let comment = history_reviews::comment(&fixture);
    let review = history_reviews::review_actions(&fixture).remove(1);
    // The legacy deferred handler may drain and retire a fixture router;
    // capture its wire oracle before creating the router used by these reads.
    let expected_review = history_reviews::legacy_review(&fixture, review.clone()).await;
    let (_, router) = snapshot_reads::router(&fixture);
    let slow = slow_actions(&fixture, load);
    let fast = fast_requests(&fixture, &router, comment, review, expected_review).await;
    verify_task_page(&fixture, load).await;
    let (probe, mut pins) =
        SqlReadProbe::new(slow.iter().map(|(action, _)| action.tool_name()).collect());
    let other_plane = {
        let app = fixture.state.lock().unwrap();
        fixture.plane = ControlPlane::with_sql_probe(&app, probe.clone());
        ControlPlane::with_sql_probe(&app, probe)
    };
    let mut readings = Vec::new();
    for (action, pinned) in slow {
        let arrivals = slow_arrivals(
            fixture.plane.clone(),
            other_plane.clone(),
            action.tool_name(),
            pinned,
        );
        for (plane, expected) in arrivals {
            readings.push(start(&fixture, plane, &router, action.clone()));
            assert_eq!(
                tokio::time::timeout(WATCHDOG, pins.next()).await.unwrap(),
                expected,
                "{} must pin {pinned} requests and queue the remaining {}",
                action.tool_name(),
                SQL_READERS - pinned
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

fn seed_task_pages(fixture: &Fixture, load: ReadLoad) {
    if matches!(load, ReadLoad::TwoSlowTypesWithPages) {
        let mut app = fixture.state.lock().unwrap();
        for index in 0..TASK_PAGE_ROWS {
            super::super::tracker::filed(
                &mut app,
                &fixture.project,
                &format!("bounded isolation page task {index}"),
            );
        }
    }
}

async fn verify_task_page(fixture: &Fixture, load: ReadLoad) {
    if matches!(load, ReadLoad::TwoSlowTypesWithPages) {
        let reply = fixture.reply(fixture.frame(task_page())).await;
        assert_eq!(reply["ok"], true, "{reply}");
        assert_eq!(
            reply["result"]["tasks"].as_array().unwrap().len(),
            TASK_PAGE_ROWS
        );
    }
}

fn task_page() -> BridgeAction {
    BridgeAction::TrackerListTasks {
        state: None,
        status: None,
        label: None,
        limit: Some(TASK_PAGE_ROWS as u64),
        cursor: None,
    }
}

fn slow_actions(fixture: &Fixture, load: ReadLoad) -> Vec<(BridgeAction, usize)> {
    let mut actions = vec![(fixture.get(), PINNED_PER_SLOW_TYPE)];
    if !matches!(load, ReadLoad::OneSlowType) {
        actions.push((
            BridgeAction::ReadConversation {
                entity_id: fixture.owner.clone(),
                agent_id: Some(fixture.agent.clone()),
                limit: 5,
            },
            PINNED_PER_SLOW_TYPE,
        ));
    }
    if matches!(load, ReadLoad::TwoSlowTypesWithPages) {
        actions.push((task_page(), PINNED_TASK_PAGES));
    }
    actions
}

fn slow_arrivals(
    first_plane: Arc<ControlPlane>,
    other_plane: Arc<ControlPlane>,
    tool: &'static str,
    pinned: usize,
) -> impl Iterator<Item = (Arc<ControlPlane>, SqlReadProbeEvent)> {
    let first_pins = std::iter::repeat_n(
        (first_plane, SqlReadProbeEvent::Pinned(tool)),
        PINNED_PER_SLOW_TYPE,
    );
    // A third task page must borrow through an independently captured plane;
    // the subsequent callers on that plane must still share the pool's quota.
    let other_pins = std::iter::repeat_n(
        (other_plane.clone(), SqlReadProbeEvent::Pinned(tool)),
        pinned - PINNED_PER_SLOW_TYPE,
    );
    let waiting = std::iter::repeat_n(
        (other_plane, SqlReadProbeEvent::Waiting(tool)),
        SQL_READERS - pinned,
    );
    first_pins.chain(other_pins).chain(waiting)
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

//! Opt-in, file-backed busy-state regression; only temporary fixtures and
//! cargo-built test code are used. No daemon or installed bridge is contacted.
use super::*;
use crate::app::mcp::{handle_done_stream, ControlPlane};
use crate::app::read_view::ReadViewSlot;
use crate::app::tests::tracker::{filed, tracked};
use crate::carrier::testing::{client_request, session_init, ChannelWire};
use crate::carrier::{FrameIntake, OutboundEnvelope};
use crate::mcp::BridgeAction;
use crate::timing::phase_samples::PhaseSamples;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Instant;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

const WORKTREES: usize = 190;
const CONVERSATIONS: usize = 200;
const TASKS: usize = 700;
const AGENTS: usize = 12;
const ROUNDS: usize = 2;
const STATUS_OBSERVATIONS_PER_AGENT: usize = 2;
const HISTORY_ITEMS: usize = 32;
const RPC_SAMPLES: usize = 8;
const CONTROL_TOOLS: [&str; 7] = [
    "create_task",
    "move_task",
    "message_agent",
    "set_topic",
    "get_task",
    "comment_task",
    "post_thread_message",
];
const RPC_METHODS: [&str; 5] = [
    "entity.seen",
    "thread.post",
    "user.present",
    "tasks.review.get",
    "git.show",
];
const REQUEST_PATIENCE: Duration = Duration::from_secs(600);
const MAX_BUSY_LOCK_WAIT_MS: f64 = 5000.0;
const MAX_BUSY_RESPONSE_MS: f64 = 5000.0;
const BUSY_MCP_READ_METHOD: &str = "mcp.control:get_task";
const MAX_BUSY_MCP_READ_RESPONSE_MS: f64 = 100.0;

struct Busy {
    _home: tempfile::TempDir,
    _root: tempfile::TempDir,
    state: Arc<Mutex<AppState>>,
    owners: Vec<(String, String)>,
    tasks: Vec<String>,
    hash: String,
    samples: PhaseSamples,
    view: ReadViewSlot,
    sessions: Vec<(TabKey, Arc<dyn AgentSession>)>,
    logs: Vec<SessionLog>,
}

impl Busy {
    fn new() -> Self {
        Self::with_history_items(HISTORY_ITEMS)
    }

    fn with_history_items(history_items: usize) -> Self {
        let root = tempfile::tempdir().unwrap();
        let (home, mut app, project) = tracked(root.path());
        let repo = app.project_at(0).repo_path.clone();
        let tasks: Vec<String> = (0..TASKS)
            .map(|i| {
                filed(&mut app, &project, &format!("busy task {i}"))["id"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect();
        let seed = adopted_run(&mut app, &repo, root.path(), "busy-000");
        let template = app
            .store
            .as_ref()
            .unwrap()
            .load_all_runs()
            .unwrap()
            .into_iter()
            .find(|record| record.id == seed)
            .unwrap();
        let mut owners = vec![(seed.clone(), primary_agent_id(&app, &seed))];
        let repository = git2::Repository::open(&repo).unwrap();
        let hash = repository.head().unwrap().target().unwrap().to_string();
        let source = app.project_at(0).sources[0].id.clone();
        // Seed directly through persisted records. Every owner has a distinct
        // conversation and checkout; Git worktrees themselves are real.
        for i in 1..CONVERSATIONS {
            let owner = format!("run-busy-{i:03}");
            let path = if i < WORKTREES {
                let path = root.path().join(format!("busy-{i:03}"));
                repository
                    .worktree(&format!("busy-{i:03}"), &path, None)
                    .unwrap();
                app.workspaces.adopt_root(
                    &project,
                    owner.clone(),
                    owner.clone(),
                    path.clone(),
                    source.clone(),
                    true,
                );
                path
            } else {
                repo.clone()
            };
            let mut record = template.clone();
            record.id = owner.clone();
            record.goal = format!("busy conversation {i}");
            record.branch = if i < WORKTREES {
                format!("busy-{i:03}")
            } else {
                "main".into()
            };
            record.worktree_name = record.branch.clone();
            record.worktree_path = path.display().to_string();
            record.agents[0] = crate::agent::Agent::new(
                crate::agent::derived_agent_id(&owner),
                &owner,
                template.agents[0].choice.clone(),
                1,
                record.created_at.clone(),
            );
            record.agents[0].watched = true;
            let agent = record.agents[0].id.clone();
            let active = crate::orchestrator::ActiveRun::reattach(&record, String::new());
            app.projects.bind_entity(owner.clone(), project.clone());
            app.persist_run_record(&owner, &active).unwrap();
            app.runs.insert(owner.clone(), active);
            owners.push((owner, agent));
        }
        for (owner, _) in &owners {
            let mut active = app.take_run(owner).unwrap();
            seed_history(
                &mut active.agents.primary_mut().unwrap().thread,
                history_items,
            );
            app.persist_run_record(owner, &active).unwrap();
            app.runs.insert(owner.clone(), active);
        }
        for task in tasks.iter().take(8) {
            app.tracker_store()
                .unwrap()
                .save_review_snapshot(
                    task,
                    &seed,
                    0,
                    crate::reviews::model::ReviewSnapshot {
                        publication: None,
                        id: format!("busy-review-snapshot-{task}"),
                        number: 0,
                        created_at: crate::store::now_rfc3339(),
                        author: crate::tracker::Actor::User,
                        directories: Vec::new(),
                    },
                )
                .unwrap();
        }
        // These sessions accept turns without spawning provider processes.
        let mut logs = Vec::new();
        for (owner, agent) in owners.iter().take(AGENTS) {
            let path = app.runs[owner].worktree.path.clone();
            let key = TabKey::agent(&AppState::canonical_root(&path), agent);
            let log = SessionLog::default();
            let mut tab = dictated_agent_tab(
                &path,
                owner,
                agent,
                DictatedSession::reporting(AgentStatus::Working).recording_into(&log),
            );
            let choice = app.runs[owner].agents.by_id(agent).unwrap().choice.clone();
            let instance =
                app.record_agent_session_start(owner, agent, &path, &choice, "busy-fixture");
            tab.session_instance = instance;
            assert!(
                app.session_instance_uses_choice(tab.session_instance.as_ref().unwrap(), &choice),
                "fixture session lineage and launch choice match"
            );
            logs.push(log);
            app.session_registry.test_insert_tab(key, tab);
            app.test_install_mcp_token(agent.clone(), format!("busy-token-{agent}"));
        }
        app.session_locator_factory =
            Arc::new(|_, _| panic!("busy fixture must reuse simulated sessions"));
        let clock = crate::timing::FrameClock::reporting_to(Arc::new(|_| {}));
        app.frame_clock = Arc::clone(&clock);
        app.delivery_queue.clear_queued();
        app.seed_anchors_for_records_without_one();
        let state = app.shared();
        // Warm projections and read admission before the timed workload.
        let _handler = AppState::handler(Arc::clone(&state));
        {
            let app = state.lock().unwrap();
            assert_eq!(app.runs.len(), CONVERSATIONS);
            let canonical: std::collections::HashSet<_> = app
                .runs
                .values()
                .map(|run| run.agents.primary().unwrap().conversation_id())
                .collect();
            assert_eq!(
                canonical.len(),
                CONVERSATIONS,
                "all fixture conversations are canonical and distinct"
            );
            assert_eq!(
                git2::Repository::open(&app.project_at(0).repo_path)
                    .unwrap()
                    .worktrees()
                    .unwrap()
                    .len(),
                WORKTREES,
                "actual Git worktrees exclude compatibility workspace aliases"
            );
            assert_eq!(
                app.runs
                    .values()
                    .filter(|run| run.agents.primary().unwrap().watched)
                    .count(),
                CONVERSATIONS
            );
        }
        let sessions = {
            let app = state.lock().unwrap();
            owners
                .iter()
                .take(AGENTS)
                .map(|(owner, agent)| {
                    let key = TabKey::agent(
                        &AppState::canonical_root(&app.runs[owner].worktree.path),
                        agent,
                    );
                    let session = Arc::clone(&app.session_registry.test_tab(&key).unwrap().session);
                    (key, session)
                })
                .collect()
        };
        let view = state.lock().unwrap().read_view.clone();
        let samples = clock.capture_phases();
        Self {
            _home: home,
            _root: root,
            state,
            owners,
            tasks,
            hash,
            samples,
            view,
            sessions,
            logs,
        }
    }
}

fn seed_history(thread: &mut crate::thread::Thread, items: usize) {
    for i in 0..items / 2 {
        thread.post_user(format!("Busy request {i}: inspect the project files and preserve the established task and conversation identity while preparing the next review."),None,"2026-10-20T12:00:00Z");
        thread.post_agent(format!("Busy answer {i}: the implementation records the committed result, and the next request should observe that result through the saved task timeline and conversation history."),None,"2026-10-20T12:00:01Z");
    }
    // Historical prompts have already been consumed by these warm sessions.
    thread.read_unread("2026-10-20T12:00:02Z");
    assert!(!thread.has_unread());
}

struct ControlIdentity {
    owner: String,
    agent: String,
    task: String,
    recipient: String,
    view: ReadViewSlot,
}

async fn control_agent(
    state: Arc<Mutex<AppState>>,
    plane: Arc<ControlPlane>,
    identity: ControlIdentity,
    start: Arc<tokio::sync::Barrier>,
    completed: Arc<AtomicUsize>,
    samples: PhaseSamples,
) {
    let ControlIdentity {
        owner,
        agent,
        task,
        recipient,
        view,
    } = identity;
    let (client, daemon) = tokio::net::UnixStream::pair().unwrap();
    let server = tokio::spawn(handle_done_stream(state, daemon, plane));
    let mut client = tokio::io::BufReader::new(client);
    start.wait().await;
    for round in 0..ROUNDS {
        let actions = [
            BridgeAction::TrackerCreateTask {
                title: format!("busy MCP task {round}"),
                body: None,
                status: None,
                labels: vec![],
                priority: None,
                attachments: vec![],
                track: Some(false),
                notify_user: Some(false),
                mention_user: Some(false),
            },
            BridgeAction::TrackerMoveTask {
                task_id: task.clone(),
                status: if round % 2 == 0 {
                    "in_progress".into()
                } else {
                    "backlog".into()
                },
                track: Some(false),
            },
            BridgeAction::MessageAgent {
                agent_id: recipient.clone(),
                body: format!("busy handoff {round}"),
                attachments: vec![],
            },
            BridgeAction::SetTopic {
                topic: format!("Busy phase {round}"),
            },
            BridgeAction::TrackerGetTask {
                task_id: task.clone(),
            },
            BridgeAction::TrackerCommentTask {
                task_id: task.clone(),
                body: format!("busy evidence {round}"),
                attachments: vec![],
                refs: vec![],
                track: Some(false),
                notify_user: Some(false),
                mention_user: Some(false),
            },
            BridgeAction::PostThreadMessage {
                body: format!("busy progress {round}"),
                still_working: true,
                options: vec![],
                attachments: vec![],
            },
        ];
        for action in actions {
            let frame = json!({"task_id":agent,"session_token":format!("busy-token-{agent}"),"request":action});
            let requested_at = Instant::now();
            client
                .get_mut()
                .write_all(format!("{frame}\n").as_bytes())
                .await
                .unwrap();
            let mut response = String::new();
            tokio::time::timeout(REQUEST_PATIENCE, client.read_line(&mut response))
                .await
                .unwrap()
                .unwrap();
            let response: Value = serde_json::from_str(&response).unwrap();
            samples.record_client_total(
                &format!("mcp.control:{}", action.tool_name()),
                requested_at.elapsed(),
            );
            assert_eq!(response["ok"], true, "{response}");
            assert_acknowledged_snapshot(&view, &owner, &agent, &action, &response);
            completed.fetch_add(1, Ordering::Relaxed);
        }
    }
    drop(client);
    server.await.unwrap();
}

fn busy_rpc(
    intake: Arc<FrameIntake>,
    start: Arc<tokio::sync::Barrier>,
    method: &'static str,
    params: Value,
    samples: PhaseSamples,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut wire = ChannelWire::open();
        let key = crate::transport::generate_session_key();
        let session = format!("busy-{method}");
        wire.open_session(
            &intake,
            &session,
            &session_init(&session, intake.transport_public_key(), &key),
        )
        .unwrap();
        assert!(matches!(
            wire.outbound.try_recv(),
            Ok(OutboundEnvelope::SessionAccept { .. })
        ));
        start.wait().await;
        let mut submitted = std::collections::BTreeMap::<u64, Instant>::new();
        for i in 0..RPC_SAMPLES {
            let mut params = params.clone();
            if method == "tasks.review.get" {
                let tasks = params.as_object_mut().unwrap().remove("task_ids").unwrap();
                params["task_id"] = tasks[i].clone();
            }
            if method == "git.show" {
                params["max_bytes"] = json!(1024 + i);
            }
            assert!(submitted.insert(i as u64, Instant::now()).is_none());
            wire.accept(
                &intake,
                client_request(
                    &key,
                    &session,
                    "data",
                    json!({"id":i,"method":method,"params":params}),
                ),
            )
            .await
            .unwrap();
        }
        let mut answers = 0;
        while answers < RPC_SAMPLES {
            let outbound = tokio::time::timeout(REQUEST_PATIENCE, wire.outbound.recv())
                .await
                .unwrap()
                .unwrap();
            let reply = SessionSender::decrypt_push(&key, &outbound);
            if reply.get("accepted").is_none() {
                let id = reply["id"]
                    .as_u64()
                    .expect("a final reply names its request");
                let requested_at = submitted
                    .remove(&id)
                    .expect("each submitted request has exactly one reply");
                samples.record_client_total(method, requested_at.elapsed());
                assert_eq!(reply["ok"], true, "{method}: {reply}");
                if method == "tasks.review.get" {
                    assert!(
                        reply["result"]["review"].is_object(),
                        "saved review read: {reply}"
                    );
                }
                answers += 1;
            }
        }
        assert!(submitted.is_empty(), "every submitted RPC completed");
    })
}

/// One ordinary owner mutation for phase diagnosis between full acceptance
/// runs. This has no timing budget and is not part of the acceptance workload.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "diagnostic only: one SetTopic against the full busy-state seed"]
async fn diagnose_busy_topic_publication_phases() {
    let mut busy = Busy::new();
    let _ = wait_for_fixture_idle(&busy).await;
    let clock = Arc::clone(&busy.state.lock().unwrap().frame_clock);
    busy.samples = clock.capture_phases();
    let (owner, agent) = busy.owners[0].clone();
    let topic = "Diagnose topic publication";
    let state = Arc::clone(&busy.state);
    let acting_owner = owner.clone();
    let acting_agent = agent.clone();
    let response = tokio::task::spawn_blocking(move || {
        let timer = clock.frame("mcp.control");
        timer.sample_tool("set_topic");
        let response = timer.lock(&state).on_agent_mcp_action(
            &acting_owner,
            &acting_agent,
            BridgeAction::SetTopic {
                topic: topic.into(),
            },
        );
        drop(timer);
        response
    })
    .await
    .unwrap()
    .expect("ordinary same-owner topic mutation succeeds");
    assert_eq!(response["topic"], topic);
    assert_acknowledged_snapshot(
        &busy.view,
        &owner,
        &agent,
        &BridgeAction::SetTopic {
            topic: topic.into(),
        },
        &response,
    );
    let measurements = wait_for_fixture_idle(&busy).await;
    assert_eq!(
        measurements["mcp.control:set_topic"]["frame.held"]["count"],
        1
    );
    println!(
        "TASK709_TOPIC_DIAGNOSTIC {}",
        json!({"diagnostic_only":true,"conversations":CONVERSATIONS,"watched_worktrees":WORKTREES,"initial_tasks":TASKS,"seeded_history_items_per_conversation":HISTORY_ITEMS,"simulated_running_agents":AGENTS,"set_topic_mutations":1,"active_frames_at_capture":0,"phases_exclusive":true,"measurements":measurements})
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "realistic 200-conversation/190-worktree/700-task hold-time fixture"]
async fn busy_writers_keep_app_lock_holds_below_budget() {
    let busy = Busy::new();
    let completed = Arc::new(AtomicUsize::new(0));
    let done = Arc::new(AtomicBool::new(false));
    let observed = busy.samples.clone();
    let count = Arc::clone(&completed);
    let finished = Arc::clone(&done);
    let logger = tokio::spawn(async move {
        while !finished.load(Ordering::Relaxed) {
            tokio::time::sleep(Duration::from_secs(10)).await;
            println!(
                "TASK709_PROGRESS {}",
                json!({"completed_mcp_calls":count.load(Ordering::Relaxed),"measurements":observed.snapshot()})
            );
        }
    });
    let isolated_handler = AppState::handler(Arc::clone(&busy.state));
    let isolated = tokio::task::spawn_blocking(move || {
        isolated_handler.call(
            SessionSender::detached("busy-isolated"),
            req("user.present", json!({})),
        )
    })
    .await
    .unwrap();
    assert_eq!(isolated["ok"], true, "{isolated}");
    println!("TASK709_ISOLATED {}", busy.samples.snapshot());
    busy.samples.clear();
    let plane = ControlPlane::new(&busy.state.lock().unwrap());
    let intake = FrameIntake::with_pool(
        AppState::handler(Arc::clone(&busy.state)),
        crate::transport::generate_transport_keypair(),
        128,
        8,
    );
    // Each joined adapter drives the real private StatusPump::observe.
    // No watch notification can coalesce an observation out of this cohort.
    let start = Arc::new(tokio::sync::Barrier::new(AGENTS * 2 + 5 + 1));
    let status_calls = status_observers(&busy, &start);
    let mut calls = Vec::new();
    for (i, ((owner, agent), task)) in busy.owners.iter().take(AGENTS).zip(&busy.tasks).enumerate()
    {
        calls.push(tokio::spawn(control_agent(
            Arc::clone(&busy.state),
            Arc::clone(&plane),
            ControlIdentity {
                owner: owner.clone(),
                agent: agent.clone(),
                task: task.clone(),
                recipient: busy.owners[(i + 1) % AGENTS].1.clone(),
                view: busy.view.clone(),
            },
            Arc::clone(&start),
            Arc::clone(&completed),
            busy.samples.clone(),
        )));
    }
    let (owner, agent) = &busy.owners[0];
    let thread_id = busy
        .state
        .lock()
        .unwrap()
        .agent_conversation(owner, Some(agent))
        .unwrap()
        .id
        .clone();
    let rpcs = [
        busy_rpc(
            Arc::clone(&intake),
            Arc::clone(&start),
            "entity.seen",
            json!({"entity_id":owner,"agent_id":agent,"thread_id":thread_id}),
            busy.samples.clone(),
        ),
        busy_rpc(
            Arc::clone(&intake),
            Arc::clone(&start),
            "thread.post",
            json!({"entity_id":owner,"agent_id":agent,"thread_id":thread_id,"body":"busy user turn"}),
            busy.samples.clone(),
        ),
        busy_rpc(
            Arc::clone(&intake),
            Arc::clone(&start),
            "user.present",
            json!({}),
            busy.samples.clone(),
        ),
        busy_rpc(
            Arc::clone(&intake),
            Arc::clone(&start),
            "tasks.review.get",
            json!({"task_ids":busy.tasks.iter().take(8).collect::<Vec<_>>()}),
            busy.samples.clone(),
        ),
        busy_rpc(
            Arc::clone(&intake),
            Arc::clone(&start),
            "git.show",
            json!({"run_id":owner,"hash":busy.hash}),
            busy.samples.clone(),
        ),
    ];
    start.wait().await;
    let mut failures = Vec::new();
    for call in calls {
        if let Err(error) = call.await {
            failures.push(error.to_string());
        }
    }
    for status in status_calls {
        if let Err(error) = status.await {
            failures.push(error.to_string());
        }
    }
    for rpc in rpcs {
        if let Err(error) = rpc.await {
            failures.push(error.to_string());
        }
    }
    if !failures.is_empty() {
        println!(
            "TASK709_FAILED_CALLS {}",
            json!({"failures":failures,"completed_mcp_calls":completed.load(Ordering::Relaxed),"measurements":busy.samples.snapshot()})
        );
    }

    let measurements = wait_for_fixture_idle(&busy).await;
    {
        let app = busy.state.lock().unwrap();
        assert!(
            busy.owners.iter().all(|(owner, _)| !app
                .delivery_queue
                .has_in_flight_at_root(&app.runs[owner].worktree.path)),
            "all delivery callbacks drained before capture"
        );
        assert!(app.delivery_queue.fixture_is_drained());
        for (key, session) in &busy.sessions {
            let tab = app
                .session_registry
                .test_tab(key)
                .expect("simulated warm session retained");
            assert!(
                Arc::ptr_eq(session, &tab.session) && tab.session_is_live(),
                "all twelve exact warm sessions remain live"
            );
        }
        for (_, agent) in busy.owners.iter().take(AGENTS) {
            assert!(
                app.session_registry
                    .token_matches(agent, &format!("busy-token-{agent}")),
                "no fixture capability was replaced"
            );
        }
    }
    for log in &busy.logs {
        let turns = log.turns();
        for round in 0..ROUNDS {
            assert!(
                turns
                    .iter()
                    .any(|text| text.contains(&format!("busy handoff {round}"))),
                "every recipient consumed every queued handoff"
            );
        }
    }
    done.store(true, Ordering::Relaxed);
    logger.abort();
    let (worst_wait_method, overall_lock_wait_max_ms) =
        overall_max_frame_sample(&measurements, "frame.lock_wait");
    let (worst_queued_method, overall_queued_max_ms) =
        overall_max_frame_sample(&measurements, "frame.queued");
    let (worst_queue_and_lock_method, overall_queue_and_lock_wait_max_ms) =
        overall_max_frame_sample(&measurements, "frame.queue_and_lock_wait");
    let (worst_permit_wait_method, overall_permit_wait_max_ms) =
        overall_max_frame_sample(&measurements, "frame.permit_wait");
    let (worst_pool_wait_method, overall_pool_wait_max_ms) =
        overall_max_frame_sample(&measurements, "frame.pool_wait");
    let (worst_queue_permit_and_lock_method, overall_queue_permit_and_lock_wait_max_ms) =
        overall_max_frame_sample(&measurements, "frame.queue_permit_and_lock_wait");
    let (worst_frame_total_method, overall_frame_total_max_ms) =
        overall_max_frame_sample(&measurements, "frame.total");
    let (worst_client_total_method, overall_client_total_max_ms) =
        overall_max_client_total(&measurements);
    println!(
        "TASK709_BUSY_BASELINE {}",
        json!({"conversations":CONVERSATIONS,"watched_worktrees":WORKTREES,"workspace_aliases":busy.state.lock().unwrap().workspaces.list(None).len(),"initial_tasks":TASKS,"seeded_history_items_per_conversation":HISTORY_ITEMS,"delivered_handoffs":AGENTS*ROUNDS,"active_frames_at_capture":0,"simulated_running_agents":AGENTS,"status_observations":AGENTS*STATUS_OBSERVATIONS_PER_AGENT,"status_observer_path":"production StatusPump::observe via MeasuredStatusPump adapter","mcp_calls_per_agent":ROUNDS*7,"phases_exclusive":true,"client_total_is_exclusive_phase":false,"client_total_endpoints":{"mcp":"before socket write through parsed JSON reply","rpc":"before encrypted wire submission through decrypted reply keyed by exact request id","status":"before blocking-pool submission through joined production observe completion"},"response_limit_ms":MAX_BUSY_RESPONSE_MS,"mcp_read_latency_cohort":["get_task"],"mcp_read_client_total_limit_ms":MAX_BUSY_MCP_READ_RESPONSE_MS,"overall_permit_wait_max_ms":overall_permit_wait_max_ms,"worst_permit_wait_method":worst_permit_wait_method,"overall_pool_wait_max_ms":overall_pool_wait_max_ms,"worst_pool_wait_method":worst_pool_wait_method,"overall_queue_permit_and_lock_wait_max_ms":overall_queue_permit_and_lock_wait_max_ms,"worst_queue_permit_and_lock_method":worst_queue_permit_and_lock_method,"overall_frame_total_max_ms":overall_frame_total_max_ms,"worst_frame_total_method":worst_frame_total_method,"overall_client_total_max_ms":overall_client_total_max_ms,"worst_client_total_method":worst_client_total_method,"overall_lock_wait_max_ms":overall_lock_wait_max_ms,"worst_lock_wait_method":worst_wait_method,"lock_wait_limit_ms":MAX_BUSY_LOCK_WAIT_MS,"overall_queued_max_ms":overall_queued_max_ms,"worst_queued_method":worst_queued_method,"overall_queue_and_lock_wait_max_ms":overall_queue_and_lock_wait_max_ms,"worst_queue_and_lock_method":worst_queue_and_lock_method,"measurements":measurements})
    );
    assert!(
        failures.is_empty(),
        "all fixture calls completed: {failures:?}"
    );
    assert_expected_samples(&measurements);
}

// The adapter is owned by #694 and invokes exactly the production observer.
// The async watch loop is deliberately bypassed so its coalescing cannot hide
// a slow status guard. All twelve exact warm sessions remain installed.
fn status_observers(
    busy: &Busy,
    start: &Arc<tokio::sync::Barrier>,
) -> Vec<tokio::task::JoinHandle<()>> {
    busy.sessions
        .iter()
        .map(|(key, session)| {
            let instance = busy
                .state
                .lock()
                .unwrap()
                .session_registry
                .test_tab(key)
                .unwrap()
                .session_instance
                .clone()
                .unwrap();
            let mut pump = crate::app::runtime::pumps::status_timing::MeasuredStatusPump::new(
                &busy.state,
                key.clone(),
                session,
                instance,
            );
            let start = Arc::clone(start);
            let samples = busy.samples.clone();
            tokio::spawn(async move {
                start.wait().await;
                let mut previous = None;
                for _ in 0..STATUS_OBSERVATIONS_PER_AGENT {
                    let snapshot = crate::harness::SessionStatusSnapshot::new(AgentStatus::Working);
                    assert_ne!(
                        previous.as_ref(),
                        Some(&snapshot.changed_at),
                        "each ordinary Working observation has a fresh boundary"
                    );
                    previous = Some(snapshot.changed_at.clone());
                    let requested_at = Instant::now();
                    pump = tokio::task::spawn_blocking(move || {
                        pump.observe(&snapshot);
                        pump
                    })
                    .await
                    .unwrap();
                    samples.record_client_total("agent.poll_status", requested_at.elapsed());
                }
            })
        })
        .collect()
}

#[test]
fn ordinary_status_observer_records_a_complete_named_frame() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let owner = "run-status-frame";
    let root = insert_run(&mut app, &repo, dir.path(), owner, RunState::Building);
    let agent = crate::agent::derived_agent_id(owner);
    let key = insert_agent_tab(
        &mut app,
        &root,
        owner,
        &agent,
        DictatedSession::reporting(AgentStatus::Working),
    );
    let tab = app.session_registry.test_tab(&key).unwrap();
    let session = Arc::clone(&tab.session);
    let instance = tab.session_instance.clone().unwrap();
    let clock = crate::timing::FrameClock::reporting_to(Arc::new(|_| {}));
    app.frame_clock = Arc::clone(&clock);
    let state = app.shared();
    let mut pump = crate::app::runtime::pumps::status_timing::MeasuredStatusPump::new(
        &state, key, &session, instance,
    );
    let samples = clock.capture_phases();
    pump.observe(&crate::harness::SessionStatusSnapshot::new(
        AgentStatus::Working,
    ));
    assert_eq!(samples.active_frames(), 0);
    let captured = samples.snapshot();
    for phase in [
        "frame.held",
        "frame.lock_wait",
        "frame.queued",
        "frame.queue_and_lock_wait",
    ] {
        assert_eq!(
            captured["agent.poll_status"][phase]["count"], 1,
            "the actual status guard participates in the whole-frame cohort"
        );
    }
    assert_eq!(captured["agent.poll_status"]["entered.lock"]["count"], 1);
}

fn expected_request_samples() -> Vec<(String, usize)> {
    CONTROL_TOOLS
        .into_iter()
        .map(|tool| (format!("mcp.control:{tool}"), AGENTS * ROUNDS))
        .chain(
            RPC_METHODS
                .into_iter()
                .map(|method| (method.into(), RPC_SAMPLES)),
        )
        .chain([(
            String::from("agent.poll_status"),
            AGENTS * STATUS_OBSERVATIONS_PER_AGENT,
        )])
        .collect::<Vec<_>>()
}

fn assert_expected_samples(measurements: &Value) {
    let expected = expected_request_samples();
    for (method, count) in &expected {
        assert_eq!(
            measurements[method]["frame.held"]["count"], *count,
            "every requested method must complete: {method}"
        );
    }
    assert!(
        measurements["agent.deliver"]["frame.held"]["count"]
            .as_u64()
            .unwrap_or(0)
            >= 1,
        "delivery work must actually run"
    );
    let (method, maximum) = overall_max_frame_sample(measurements, "frame.lock_wait");
    assert!(
        maximum <= MAX_BUSY_LOCK_WAIT_MS,
        "whole busy fixture exceeded {MAX_BUSY_LOCK_WAIT_MS}ms frame lock-wait limit: {method} waited {maximum}ms"
    );
    for metric in ["frame.permit_wait", "frame.pool_wait"] {
        let _ = overall_max_frame_sample(measurements, metric);
    }
    let (method, maximum) =
        overall_max_frame_sample(measurements, "frame.queue_permit_and_lock_wait");
    assert!(maximum <= MAX_BUSY_RESPONSE_MS, "whole busy fixture queue+permit+lock wait exceeded {MAX_BUSY_RESPONSE_MS}ms: {method} waited {maximum}ms");
    let (method, total) = overall_max_frame_sample(measurements, "frame.total");
    assert!(
        total <= MAX_BUSY_RESPONSE_MS,
        "whole busy fixture frame total exceeded {MAX_BUSY_RESPONSE_MS}ms: {method} took {total}ms"
    );
    let (method, total) = overall_max_client_total(measurements);
    assert!(total <= MAX_BUSY_RESPONSE_MS, "whole busy fixture client/status response exceeded {MAX_BUSY_RESPONSE_MS}ms: {method} took {total}ms");
    assert_busy_mcp_read_latency(measurements);
    for method in expected
        .iter()
        .map(|(method, _)| method.as_str())
        .chain(["agent.deliver"])
    {
        let phases = &measurements[method];
        let held = &phases["frame.held"];
        assert!(
            held["p95_ms"].as_f64().unwrap() < 50.0 && held["max_ms"].as_f64().unwrap() < 200.0,
            "app-lock hold budget exceeded for {method}: {held}"
        );
    }
}

// This busy workload exercises get_task, one member of the supported MCP read
// class. Other read tools retain their focused progress and authorization tests.
fn assert_busy_mcp_read_latency(measurements: &Value) {
    let latency = &measurements[BUSY_MCP_READ_METHOD]["client.total"];
    assert_eq!(
        latency["count"].as_u64(),
        Some((AGENTS * ROUNDS) as u64),
        "every get_task caller must be measured"
    );
    let maximum = latency["max_ms"]
        .as_f64()
        .expect("get_task has an actual caller latency maximum");
    assert!(maximum.is_finite() && maximum >= 0.0);
    assert!(
        maximum <= MAX_BUSY_MCP_READ_RESPONSE_MS,
        "busy MCP get_task client.total exceeded {MAX_BUSY_MCP_READ_RESPONSE_MS}ms: {latency}"
    );
}

fn overall_max_client_total(measurements: &Value) -> (String, f64) {
    expected_request_samples()
        .into_iter()
        .map(|(method, expected)| {
            let phases = &measurements[&method];
            let count = phases["client.total"]["count"]
                .as_u64()
                .expect("every client/status submission completed");
            assert_eq!(
                count, expected as u64,
                "exact completed caller count for {method}"
            );
            assert_eq!(
                count,
                phases["frame.held"]["count"].as_u64().unwrap(),
                "every observed caller has a completed frame: {method}"
            );
            let maximum = phases["client.total"]["max_ms"].as_f64().unwrap();
            assert!(maximum.is_finite() && maximum >= 0.0);
            (method, maximum)
        })
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .expect("the busy fixture submits actual requests")
}

fn overall_max_frame_sample<'a>(measurements: &'a Value, sample: &str) -> (&'a str, f64) {
    measurements
        .as_object()
        .unwrap()
        .iter()
        .map(|(method, phases)| {
            let completed = phases["frame.held"]["count"]
                .as_u64()
                .expect("each captured method has completed frame samples");
            let sampled = phases[sample]["count"]
                .as_u64()
                .expect("every completed frame has this sample, including zero waits");
            assert!(completed > 0, "{method} completed at least one frame");
            assert_eq!(sampled, completed, "{method} {sample} samples are complete");
            let maximum = phases[sample]["max_ms"]
                .as_f64()
                .expect("each method has an exact sample maximum");
            assert!(maximum.is_finite() && maximum >= 0.0);
            (method.as_str(), maximum)
        })
        .max_by(|left, right| left.1.total_cmp(&right.1))
        .expect("completed fixture frames include exact lock-wait samples")
}

fn assert_acknowledged_snapshot(
    view: &ReadViewSlot,
    owner: &str,
    agent: &str,
    action: &BridgeAction,
    response: &Value,
) {
    let published = view.load();
    match action {
        BridgeAction::SetTopic { topic } => {
            let agents = published
                .get("agent.list", owner)
                .expect("published owner roster");
            let row = agents["agents"]
                .as_array()
                .unwrap()
                .iter()
                .find(|row| row["id"] == agent)
                .unwrap();
            assert_eq!(
                row["topic"], *topic,
                "acknowledged topic is visible through independent immutable read"
            );
        }
        BridgeAction::PostThreadMessage { body, .. } => {
            let history = published
                .history(agent)
                .expect("published canonical history")
                .render(&json!({}));
            assert!(
                history["items"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(
                        |item| item["data"]["id"] == response["result"]["message_id"]
                            && item["data"]["body"] == *body
                    ),
                "acknowledged message is visible through independent immutable history read"
            );
        }
        _ => {}
    }
}

async fn wait_for_fixture_idle(busy: &Busy) -> Value {
    tokio::time::timeout(REQUEST_PATIENCE, async {
        loop {
            {
                let app = busy.state.lock().unwrap();
                if app.delivery_queue.fixture_is_drained() && busy.samples.active_frames() == 0 {
                    // Freeze the report at the proven quiet cut. Periodic task
                    // polling may start another harmless frame after this guard.
                    return busy.samples.snapshot();
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("queued/settling/in-flight deliveries and sampled frames all finished")
}

/// Keep terminal admission independent of retained workspace and history size.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "realistic 200-conversation/190-worktree/700-task terminal hold fixture"]
async fn busy_term_create_keeps_app_lock_hold_below_50ms() {
    let busy = Busy::with_history_items(500);
    let handler = AppState::handler(Arc::clone(&busy.state));
    let workspace = busy.owners[AGENTS + 1].0.clone();
    let (created, work) = crate::app::snapshot_projection::work_counts::measure(|| {
        call(&handler, "term.create", json!({"workspace_id": workspace}))
    });
    assert_eq!(created["ok"], true, "{created}");
    let measurements = busy.samples.snapshot();
    eprintln!(
        "term.create busy phases: {}",
        serde_json::to_string_pretty(&measurements["term.create"]).unwrap()
    );
    let held = measurements["term.create"]["frame.held"]["max_ms"]
        .as_f64()
        .unwrap();
    let (_, follow_on) = crate::app::snapshot_projection::work_counts::measure(|| {
        busy.state
            .lock()
            .unwrap()
            .publish_owner_snapshot_projection(&busy.owners[0].0);
    });
    let child = {
        let app = busy.state.lock().unwrap();
        let key = app
            .tab_key_of_wire_id(created["result"]["term_id"].as_str().unwrap())
            .unwrap();
        Arc::clone(&app.session_registry.test_tab(&key).unwrap().session)
    };
    let closed = call(
        &handler,
        "term.close",
        json!({"term_id": created["result"]["term_id"]}),
    );
    child.end();
    assert!(
        child.exited_within(Duration::from_secs(2)),
        "the timing fixture shell must be reaped"
    );
    assert_eq!(closed["ok"], true, "{closed}");
    assert!(
        held < 50.0,
        "term.create held the app mutex for {held:.3}ms"
    );
    assert_eq!(
        work.workspace_details_full + work.workspace_details_scoped,
        0,
        "{work:?}"
    );
    assert_eq!(work.history_updates, 0, "{work:?}");
    assert_eq!(
        follow_on.workspace_details_full, 0,
        "a shell must not invalidate the next owner publication: {follow_on:?}"
    );
    let (_, after_close) = crate::app::snapshot_projection::work_counts::measure(|| {
        busy.state
            .lock()
            .unwrap()
            .publish_owner_snapshot_projection(&busy.owners[0].0);
    });
    assert_eq!(
        after_close.workspace_details_full, 0,
        "closing a shell must not invalidate the next owner publication: {after_close:?}"
    );
    assert_busy_terminal_spawn_allows_concurrent_frame(&busy, &handler, &workspace);
}

fn assert_busy_terminal_spawn_allows_concurrent_frame(
    busy: &Busy,
    handler: &FrameHandler,
    workspace: &str,
) {
    let (gate, held) = OffLockGate::new();
    busy.state.lock().unwrap().terminal_create_spawn_gate = Some(gate);
    let spawning = {
        let handler = handler.clone();
        let workspace = workspace.to_owned();
        std::thread::spawn(move || call(&handler, "term.create", json!({"workspace_id":workspace})))
    };
    held.wait_for_arrival();
    busy.state.lock().unwrap().terminal_create_spawn_gate = None;
    let (sent, received) = std::sync::mpsc::channel();
    let concurrent = {
        let handler = handler.clone();
        std::thread::spawn(move || {
            sent.send(call(&handler, "session.hello", json!({})))
                .unwrap();
        })
    };
    let reply = received.recv_timeout(Duration::from_millis(100));
    held.release();
    let created = spawning.join().unwrap();
    concurrent.join().unwrap();
    let child = {
        let app = busy.state.lock().unwrap();
        let key = app
            .tab_key_of_wire_id(created["result"]["term_id"].as_str().unwrap())
            .unwrap();
        Arc::clone(&app.session_registry.test_tab(&key).unwrap().session)
    };
    let closed = call(
        handler,
        "term.close",
        json!({"term_id":created["result"]["term_id"]}),
    );
    child.end();
    assert!(child.exited_within(Duration::from_secs(2)));
    assert_eq!(created["ok"], true, "{created}");
    assert_eq!(closed["ok"], true, "{closed}");
    assert_eq!(
        reply.expect("a concurrent frame must answer during the large-seed spawn")["ok"],
        true
    );
    let measurements = busy.samples.snapshot();
    eprintln!(
        "term.create busy final phases: {}",
        serde_json::to_string_pretty(&measurements["term.create"]).unwrap()
    );
    assert!(
        measurements["term.create"]["frame.held"]["max_ms"]
            .as_f64()
            .unwrap()
            < 50.0
    );
}

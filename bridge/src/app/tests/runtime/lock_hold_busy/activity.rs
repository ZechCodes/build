//! Concurrent real activity consumers beside the existing MCP writer cohort.
use super::*;
use crate::app::runtime::pumps::activity_probe::ActivityProbe;
use crate::harness::{ActivityReport, AgentActivity};

const REPORTS: usize = 8;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "release timing: concurrent activity publication in the busy resident fixture"]
async fn busy_activity_publication_finishes_within_budget() {
    let busy = Busy::new();
    let plane = ControlPlane::new(&busy.state.lock().unwrap());
    let handler = AppState::handler(Arc::clone(&busy.state));
    let (terminal, child) = prepare_terminal(&busy, &handler);
    let start = Arc::new(tokio::sync::Barrier::new(AGENTS * 2 + 2));
    let completed = Arc::new(AtomicUsize::new(0));
    let mut subscriber = ActivitySubscriber::new(&busy);
    let mut calls = Vec::new();
    for (i, ((owner, agent), (key, session))) in busy.owners.iter().zip(&busy.sessions).enumerate()
    {
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
        let pump = ActivityProbe::new(&busy.state, key.clone(), session, instance);
        calls.push(activity_agent(
            pump,
            i,
            &start,
            &completed,
            busy.samples.clone(),
        ));
        calls.push(tokio::spawn(control_agent(
            Arc::clone(&busy.state),
            Arc::clone(&plane),
            ControlIdentity {
                owner: owner.clone(),
                agent: agent.clone(),
                task: busy.tasks[i].clone(),
                recipient: busy.owners[(i + 1) % AGENTS].1.clone(),
                view: busy.view.clone(),
            },
            Arc::clone(&start),
            Arc::clone(&completed),
            busy.samples.clone(),
        )));
    }
    let close_start = Arc::clone(&start);
    let close_samples = busy.samples.clone();
    calls.push(tokio::spawn(async move {
        close_start.wait().await;
        let started = Instant::now();
        let closed = tokio::task::spawn_blocking(move || {
            call(&handler, "term.close", json!({"term_id":terminal}))
        })
        .await
        .unwrap();
        close_samples.record_client_total("term.close", started.elapsed());
        child.end();
        assert!(
            child.exited_within(Duration::from_secs(2)),
            "owned fixture shell is reaped"
        );
        assert_eq!(closed["ok"], true, "{closed}");
    }));
    // Every probe and handler is ready before the deliberate competing hold.
    let holder = competing_close_holder(&busy);
    start.wait().await;
    for call in calls {
        call.await.unwrap();
    }
    assert!(
        holder.join().unwrap(),
        "terminal close reached app-lock wait while the named holder was live"
    );
    assert_eq!(
        completed.load(Ordering::Relaxed),
        AGENTS * ROUNDS * CONTROL_TOOLS.len()
    );
    let idle = wait_for_fixture_idle(&busy).await;
    subscriber.wait_for_final_cursors(&busy).await;
    let captured = busy.samples.snapshot();
    let clock = Arc::clone(&busy.state.lock().unwrap().frame_clock);
    let publication = clock.stats()["publication"].clone();
    assert!(publication["push"]["count"].as_u64().unwrap() > 0);
    assert!(publication["push_pending"]["count"].as_u64().unwrap() > 0);
    println!(
        "TASK820_ACTIVITY {}",
        json!({"phases":captured,"idle":idle,"publication":publication})
    );
    assert_eq!(
        captured["agent.activity"]["frame.total"]["count"],
        AGENTS * REPORTS
    );
    assert_activity_state(&busy);
    // Each first report registers the canonical name and roster resume id;
    // every subsequent report needs only its own canonical activity commit.
    let expected_commits = AGENTS * (REPORTS + 2);
    for phase in ["entered.finisher.save", "entered.publication.install"] {
        assert_eq!(
            captured["agent.activity"][phase]["count"], expected_commits,
            "activity work must stay bounded independently of host load: {phase}"
        );
    }
    assert_latency_budget(&captured, "agent.activity");
    assert_latency_budget(&captured, "term.close");
}

fn activity_agent(
    pump: ActivityProbe,
    index: usize,
    start: &Arc<tokio::sync::Barrier>,
    completed: &Arc<AtomicUsize>,
    samples: PhaseSamples,
) -> tokio::task::JoinHandle<()> {
    let start = Arc::clone(start);
    let completed = Arc::clone(completed);
    tokio::spawn(async move {
        start.wait().await;
        let mut overlapped_writer = false;
        for round in 0..REPORTS {
            let summary = format!("activity witness {index}/{round}");
            let report = ActivityReport::own_work(AgentActivity::Narration { summary });
            let started = Instant::now();
            overlapped_writer |=
                completed.load(Ordering::Relaxed) < AGENTS * ROUNDS * CONTROL_TOOLS.len();
            assert!(pump.observed_report(report).await);
            samples.record_client_total("agent.activity", started.elapsed());
        }
        assert!(
            overlapped_writer,
            "every agent must report while MCP writes remain outstanding"
        );
    })
}

fn competing_close_holder(busy: &Busy) -> std::thread::JoinHandle<bool> {
    let clock = Arc::clone(&busy.state.lock().unwrap().frame_clock);
    let (close_waiting, close_wait) = std::sync::mpsc::channel();
    clock.observe_requests(Arc::new(move |frame, phase, _| {
        if frame.payload["method"] == "term.close" && phase == "app_wait_start" {
            let _ = close_waiting.send(());
        }
    }));
    let state = Arc::clone(&busy.state);
    let (ready, held) = std::sync::mpsc::channel();
    let holder = std::thread::spawn(move || {
        let timer = clock.frame(AGENT_DELIVERY_METHOD);
        let _guard = timer.lock(&state);
        ready.send(()).unwrap();
        close_wait.recv_timeout(Duration::from_secs(5)).is_ok()
    });
    held.recv_timeout(Duration::from_secs(5))
        .expect("the named competitor owns the app lock");
    holder
}

fn assert_latency_budget(captured: &Value, method: &str) {
    for metric in ["client.total", "frame.total", "frame.lock_wait"] {
        assert!(
            captured[method][metric]["max_ms"].as_f64().unwrap() <= 5000.0,
            "{method} exceeded 5 seconds: {metric}: {captured}"
        );
    }
}

fn assert_activity_state(busy: &Busy) {
    let app = busy.state.lock().unwrap();
    for (i, (owner, agent)) in busy.owners.iter().take(AGENTS).enumerate() {
        let saved = app.store.as_ref().unwrap().thread_items(agent).unwrap();
        let published: Vec<crate::thread::ThreadItem> = serde_json::from_value(
            busy.view.load().history(agent).unwrap().render(&json!({}))["items"].clone(),
        )
        .unwrap();
        let saved = activity_witnesses(saved);
        let published = activity_witnesses(published);
        assert_eq!(
            saved, published,
            "durable and published exact activity events for {owner}"
        );
        let actual: Vec<_> = saved
            .iter()
            .map(|item| match item {
                crate::thread::ThreadItem::Event(event) => event.summary.clone().unwrap(),
                _ => unreachable!(),
            })
            .collect();
        let expected: Vec<_> = (0..REPORTS)
            .map(|round| format!("activity witness {i}/{round}"))
            .collect();
        assert_eq!(
            actual, expected,
            "every event persists exactly once in stream order"
        );
    }
}

struct ActivitySubscriber {
    incoming: tokio::sync::mpsc::UnboundedReceiver<OutboundEnvelope>,
    key: String,
}

impl ActivitySubscriber {
    fn new(busy: &Busy) -> Self {
        let (sender, incoming, key) = SessionSender::observable("busy-activity-subscriber");
        busy.state.lock().unwrap().changes().subscribe(
            &sender,
            crate::changes::SubscriptionSpec {
                id: "activity-publication".into(),
                scope: crate::changes::Scope::All,
                kinds: [crate::changes::Kind::Thread].into_iter().collect(),
                mode: Default::default(),
                priority: Default::default(),
            },
        );
        Self { incoming, key }
    }

    async fn wait_for_final_cursors(&mut self, busy: &Busy) {
        let expected: std::collections::BTreeMap<_, _> = {
            let app = busy.state.lock().unwrap();
            busy.owners
                .iter()
                .take(AGENTS)
                .flat_map(|(owner, _)| app.thread_tips(owner, &[]))
                .map(|tip| {
                    (
                        tip.agent_id.clone(),
                        cursor(&serde_json::to_value(tip).unwrap()),
                    )
                })
                .collect()
        };
        assert_eq!(expected.len(), AGENTS);
        let mut observed = std::collections::BTreeMap::new();
        tokio::time::timeout(REQUEST_PATIENCE, async {
            while expected
                .iter()
                .any(|(agent, wanted)| observed.get(agent) != Some(wanted))
            {
                let envelope = self
                    .incoming
                    .recv()
                    .await
                    .expect("the subscribed push carrier stays live");
                let payload = SessionSender::decrypt_push(&self.key, &envelope);
                for tip in payload["items"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .flat_map(|item| item["thread"].as_array().into_iter().flatten())
                {
                    let agent = tip["agent_id"].as_str().unwrap();
                    if expected.contains_key(agent) {
                        observed.insert(agent.to_string(), cursor(tip));
                    }
                }
            }
        })
        .await
        .unwrap_or_else(|_| {
            panic!("final push cursors did not drain: expected {expected:?}; observed {observed:?}")
        });
        assert_eq!(
            observed, expected,
            "every agent's final committed generation and cursor reached the carrier"
        );
    }
}

fn cursor(tip: &Value) -> Value {
    json!([
        tip["last_sequence"],
        tip["thread_id"],
        tip["thread_generation_revision"]
    ])
}

fn prepare_terminal(busy: &Busy, handler: &FrameHandler) -> (String, Arc<dyn AgentSession>) {
    use std::os::unix::fs::PermissionsExt;
    let shell = busy._root.path().join("activity-fixture-shell");
    std::fs::write(&shell, "#!/bin/sh\nwhile IFS= read -r line; do :; done\n").unwrap();
    std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o700)).unwrap();
    busy.state.lock().unwrap().term_shell = shell.display().to_string();
    let created = call(
        handler,
        "term.create",
        json!({"workspace_id":busy.owners[AGENTS + 1].0}),
    );
    assert_eq!(created["ok"], true, "{created}");
    let terminal = created["result"]["term_id"].as_str().unwrap().to_string();
    let child = {
        let app = busy.state.lock().unwrap();
        let key = app.tab_key_of_wire_id(&terminal).unwrap();
        Arc::clone(&app.session_registry.test_tab(&key).unwrap().session)
    };
    (terminal, child)
}

fn activity_witnesses(items: Vec<crate::thread::ThreadItem>) -> Vec<crate::thread::ThreadItem> {
    items
        .into_iter()
        .filter(|item| {
            matches!(item, crate::thread::ThreadItem::Event(event)
        if event.summary.as_deref().is_some_and(|summary| summary.starts_with("activity witness ")))
        })
        .collect()
}

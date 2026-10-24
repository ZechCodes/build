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
    let mut calls = Vec::new();
    for (i, ((owner, agent), (key, session))) in busy.owners.iter().zip(&busy.sessions).enumerate() {
        let instance = busy.state.lock().unwrap().session_registry.test_tab(key)
            .unwrap().session_instance.clone().unwrap();
        let mut pump = ActivityProbe::new(&busy.state, key.clone(), session, instance);
        let barrier = Arc::clone(&start);
        let samples = busy.samples.clone();
        let writer_completions = Arc::clone(&completed);
        calls.push(tokio::spawn(async move {
            barrier.wait().await;
            let mut overlapped_writer = false;
            for round in 0..REPORTS {
                let summary = format!("activity witness {i}/{round}");
                let report = ActivityReport::own_work(AgentActivity::Narration { summary });
                let started = Instant::now();
                let writers = Arc::clone(&writer_completions);
                let (returned, live, overlap) = tokio::task::spawn_blocking(move || {
                    let overlap = writers.load(Ordering::Relaxed) < AGENTS * ROUNDS * CONTROL_TOOLS.len();
                    let live = pump.report(&report);
                    (pump, live, overlap)
                }).await.unwrap();
                samples.record_client_total("agent.activity", started.elapsed());
                assert!(live);
                overlapped_writer |= overlap;
                // Keep the same exact consumer for its full ordered stream.
                // A next event cannot overtake a prior persistence/publication.
                pump = returned;
            }
            assert!(overlapped_writer, "every agent must report while MCP writes remain outstanding");
        }));
        calls.push(tokio::spawn(control_agent(
            Arc::clone(&busy.state), Arc::clone(&plane),
            ControlIdentity {
                owner: owner.clone(), agent: agent.clone(), task: busy.tasks[i].clone(),
                recipient: busy.owners[(i + 1) % AGENTS].1.clone(), view: busy.view.clone(),
            }, Arc::clone(&start), Arc::clone(&completed), busy.samples.clone(),
        )));
    }
    let close_start = Arc::clone(&start);
    let close_samples = busy.samples.clone();
    calls.push(tokio::spawn(async move {
        close_start.wait().await;
        let started = Instant::now();
        let closed = tokio::task::spawn_blocking(move || {
            call(&handler, "term.close", json!({"term_id":terminal}))
        }).await.unwrap();
        close_samples.record_client_total("term.close", started.elapsed());
        child.end();
        assert!(child.exited_within(Duration::from_secs(2)), "owned fixture shell is reaped");
        assert_eq!(closed["ok"], true, "{closed}");
    }));
    start.wait().await;
    for call in calls { call.await.unwrap(); }
    assert_eq!(completed.load(Ordering::Relaxed), AGENTS * ROUNDS * CONTROL_TOOLS.len());
    let idle = wait_for_fixture_idle(&busy).await;
    let captured = busy.samples.snapshot();
    println!("TASK820_ACTIVITY {}", json!({"phases":captured,"idle":idle}));
    assert_eq!(captured["agent.activity"]["frame.total"]["count"], AGENTS * REPORTS);
    for metric in ["client.total", "frame.total", "frame.lock_wait"] {
        assert!(captured["agent.activity"][metric]["max_ms"].as_f64().unwrap() <= 5000.0,
            "activity exceeded 5 seconds: {metric}: {captured}");
    }
    for metric in ["client.total", "frame.total", "frame.lock_wait"] {
        assert!(captured["term.close"][metric]["max_ms"].as_f64().unwrap() <= 5000.0,
            "terminal close exceeded 5 seconds: {metric}: {captured}");
    }
    let app = busy.state.lock().unwrap();
    for (i, (owner, agent)) in busy.owners.iter().take(AGENTS).enumerate() {
        let saved = app.store.as_ref().unwrap().thread_items(agent).unwrap();
        let published: Vec<crate::thread::ThreadItem> = serde_json::from_value(
            busy.view.load().history(agent).unwrap().render(&json!({}))["items"].clone()
        ).unwrap();
        let saved = activity_witnesses(saved);
        let published = activity_witnesses(published);
        assert_eq!(saved, published, "durable and published exact activity events for {owner}");
        let actual: Vec<_> = saved.iter().map(|item| match item {
            crate::thread::ThreadItem::Event(event) => event.summary.clone().unwrap(),
            _ => unreachable!(),
        }).collect();
        let expected: Vec<_> = (0..REPORTS).map(|round| format!("activity witness {i}/{round}")).collect();
        assert_eq!(actual, expected, "every event persists exactly once in stream order");
    }
}

fn prepare_terminal(busy: &Busy, handler: &FrameHandler) -> (String, Arc<dyn AgentSession>) {
    use std::os::unix::fs::PermissionsExt;
    let shell = busy._root.path().join("activity-fixture-shell");
    std::fs::write(&shell, "#!/bin/sh\nwhile IFS= read -r line; do :; done\n").unwrap();
    std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o700)).unwrap();
    busy.state.lock().unwrap().term_shell = shell.display().to_string();
    let created = call(handler, "term.create", json!({"workspace_id":busy.owners[AGENTS + 1].0}));
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
    items.into_iter().filter(|item| matches!(item, crate::thread::ThreadItem::Event(event)
        if event.summary.as_deref().is_some_and(|summary| summary.starts_with("activity witness ")))).collect()
}

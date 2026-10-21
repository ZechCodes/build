use super::*;
use crate::harness::{OpenedSession, SessionOutput};
use crate::test_support::agent_session::FixtureSessions;
use std::os::unix::fs::PermissionsExt;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn actual_agent_start_uses_finite_factory_after_real_spawn_claim() {
    let (directory, repository) = init_repo();
    let mut app = qa_state(&repository, directory.path());
    app.default_harness = AgentProvider::Claude;
    let fallback = directory.path().join("factory-red-private-fallback");
    std::fs::write(
        &fallback,
        "#!/bin/sh\nprintf '\\033[?2004h'\nexec /bin/cat >/dev/null\n",
    )
    .unwrap();
    std::fs::set_permissions(&fallback, std::fs::Permissions::from_mode(0o700)).unwrap();
    let private_home = directory.path().join("private-home");
    std::fs::create_dir(&private_home).unwrap();
    let private_agent = Agent::Warm(
        HarnessSpec::new(fallback.to_string_lossy().into_owned())
            .env("PATH", "/nonexistent")
            .env("HOME", private_home.to_string_lossy().into_owned())
            .env("ENV", "/dev/null")
            .env("BASH_ENV", "/dev/null"),
    );
    app.agent = private_agent.clone();
    let worktrees = app.worktrees_root.clone();
    app.project_at_mut(0).orch = Orchestrator::new(
        repository.clone(),
        worktrees,
        private_agent,
        Templates::default(),
        test_bridge_exe(),
    );
    let owner = adopted_run(
        &mut app,
        &repository,
        directory.path(),
        "finite-agent-start",
    );
    let agent_id = primary_agent_id(&app, &owner);
    let agent = app.entity_agents(&owner).unwrap().by_id(&agent_id).unwrap();
    assert_eq!(agent.choice.provider, AgentProvider::Claude);
    let address = json!({
        "id": owner,
        "agent_id": agent_id,
        "conversation_id": agent.conversation_id(),
        "thread_id": agent.thread.id,
    });
    let root = AppState::canonical_root(&app.runs[&owner].worktree.path);
    let tab_key = TabKey::agent(&root, &agent_id);
    assert!(!app.session_registry.contains(&tab_key));
    assert_eq!(app.session_registry.test_counts().claims, 0);

    let opened = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&opened);
    let log = SessionLog::default();
    let logged = log.clone();
    let (activity, _) = broadcast::channel(8);
    let activity_out = activity.clone();
    let expected_root = root.clone();
    let sessions = FixtureSessions::install(
        std::fs::canonicalize(directory.path()).unwrap(),
        Arc::new(move |provider, request| {
            assert_eq!(request.root, expected_root);
            assert_eq!(provider, AgentProvider::Claude);
            assert_eq!(provider, request.choice.provider);
            observed.fetch_add(1, Ordering::SeqCst);
            Ok(OpenedSession {
                session: Arc::new(
                    DictatedSession::reporting(AgentStatus::Waiting).recording_into(&logged),
                ),
                output: SessionOutput::reporting(activity_out.subscribe(), None),
            })
        }),
    );
    let mut fixture = RareFixture::new("agent-start-factory", app);
    let _cleanup = EndFixtureSessions(Arc::clone(fixture.state()));
    let proof = fixture.call("agent.start", address).await;
    let facts = wait_for(Duration::from_secs(2), || {
        let app = fixture.state().lock().unwrap();
        if !app.delivery_queue.is_idle() {
            return None;
        }
        let tab = app.session_registry.test_tab(&tab_key)?;
        Some((
            app.session_registry.test_counts().claims,
            tab.session_instance.clone()?,
            tab.wire_id(),
            tab.role.clone(),
            agent_pid(tab),
        ))
    })
    .await;
    let diagnostic = {
        let app = fixture.state().lock().unwrap();
        json!({
            "claims":app.session_registry.test_counts().claims,
            "delivery_idle":app.delivery_queue.is_idle(),
            "tabs":app.session_registry.test_tabs().map(|(key,_)|format!("{key:?}")).collect::<Vec<_>>(),
            "start_error":app.entity_agents(&owner).ok().and_then(|roster|roster.by_id(&agent_id)).and_then(|agent|agent.start_error.clone()),
        })
    };
    // The RED scaffold still opens the private absolute drain child. Retire it
    // before asserting the missing hook so a failed test owns no live child.
    fixture
        .call(
            "agent.remove",
            json!({"entity_id": owner, "agent_id": agent_id}),
        )
        .await;
    let delivery_idle = wait_for(Duration::from_secs(2), || {
        fixture
            .state()
            .lock()
            .unwrap()
            .delivery_queue
            .is_idle()
            .then_some(())
    })
    .await
    .is_some();
    let reaped = match facts.as_ref().and_then(|facts| facts.4) {
        Some(pid) => tokio::task::spawn_blocking(move || process_reaped(pid))
            .await
            .unwrap(),
        None => true,
    };
    assert!(delivery_idle, "private delivery did not drain: {diagnostic}");
    assert!(reaped, "owned private fallback child was not reaped");
    let (claims, lineage, registered, role, _) = facts.unwrap_or_else(|| {
        panic!("real start delivery never published its tab/lineage: {diagnostic}")
    });
    assert!(matches!(
        role,
        TabRole::Agent {
            provider: AgentProvider::Claude,
            ..
        }
    ));
    assert_eq!(proof.reply["result"]["term_id"], registered);
    let calls = opened.load(Ordering::SeqCst);
    assert_eq!(claims, 0, "the real spawn reservation must settle");
    assert_eq!(lineage.entity_id, owner);
    assert_eq!(lineage.agent_id, agent_id);
    assert_eq!(calls, 1, "actual agent.start bypassed its finite factory");
    fixture
        .verify(
            proof,
            json!({"factory_calls": calls, "lineage": lineage.id}),
        )
        .await;
    assert!(log.ended());
    let steps = fixture.finish().await;
    assert_eq!(steps.len(), 1);
    drop(sessions);
    drop(activity);
}

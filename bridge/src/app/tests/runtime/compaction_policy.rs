use super::*;
use crate::app::runtime::spawning::{
    open_agent_session, publish_agent_tab, OpenedSession, ReservedSpawn,
};
use crate::compaction::{CompactionScope, CompactionSource};

const OWNER: &str = "run-policy-lifecycle";

struct PolicyLaunch {
    _dir: tempfile::TempDir,
    state: Arc<Mutex<AppState>>,
    handler: FrameHandler,
    key: TabKey,
    agent: String,
    choice: ModelChoice,
    builds: Arc<Mutex<Vec<SpawnOptions>>>,
}

impl PolicyLaunch {
    fn new(fail_open: bool) -> Self {
        let (dir, repo) = init_repo();
        let (state, handler, root) = agent_tab_fixture(&repo, dir.path(), OWNER);
        let agent = crate::agent::derived_agent_id(OWNER);
        let choice = ModelChoice {
            provider: AgentProvider::Claude,
            ..ModelChoice::default()
        };
        let builds = Arc::new(Mutex::new(Vec::new()));
        {
            let recorded = Arc::clone(&builds);
            let mut app = state.lock().unwrap();
            app.set_agent_model_choice(OWNER, &agent, choice.clone())
                .unwrap();
            app.resume_id_probe = Arc::new(|_, _, _| true);
            app.session_locator_factory = Arc::new(|_, _| None);
            let worktrees = app.worktrees_root.clone();
            app.project_at_mut(0).orch = Orchestrator::new(
                repo,
                worktrees,
                Agent::WarmBuilder(Arc::new(move |_, _, options| {
                    recorded.lock().unwrap().push(options.clone());
                    Ok(if fail_open {
                        HarnessSpec::new("/nonexistent/build-655-test-harness")
                    } else {
                        warm_tui_spec()
                    })
                })),
                Templates::default(),
                test_bridge_exe(),
            );
        }
        Self {
            _dir: dir,
            state,
            handler,
            key: TabKey::agent(&AppState::canonical_root(&root), &agent),
            agent,
            choice,
            builds,
        }
    }

    fn set_threshold(&self, tokens: u64) {
        self.state
            .lock()
            .unwrap()
            .patch_compaction_thresholds(CompactionScope::Device, &json!({"claude": tokens}))
            .unwrap();
    }

    fn reserve(&self) -> ReservedSpawn {
        reserve_agent_spawn(
            &mut self.state.lock().unwrap(),
            &self.key,
            &AgentSpawnRequest {
                owner: OWNER,
                agent_id: &self.agent,
                conversation_id: &self.agent,
                model_choice: &self.choice,
                force_fresh: false,
                phase: "test",
            },
        )
        .unwrap()
    }

    fn open(&self, reserved: ReservedSpawn) -> OpenedSession {
        open_agent_session(
            &self.state,
            reserved,
            &self.key,
            &self.agent,
            &a_frame(&self.state),
        )
        .unwrap()
        .unwrap()
    }

    fn publish(&self, opened: OpenedSession) -> Option<String> {
        publish_agent_tab(
            &self.state,
            &self.key,
            opened,
            &self.agent,
            &self.choice,
            "test",
            &a_frame(&self.state),
        )
    }

    fn session(&self) -> Arc<dyn AgentSession> {
        self.state
            .lock()
            .unwrap()
            .session_registry
            .agent_snapshot(&self.key)
            .unwrap()
            .session
    }
}

#[tokio::test]
async fn admitted_policy_survives_setting_edits_warm_reuse_and_browser_attach() {
    let fixture = PolicyLaunch::new(false);
    fixture.set_threshold(123_456);
    let reserved = fixture.reserve();
    fixture.set_threshold(234_567);
    let opened = fixture.open(reserved);
    let admitted = fixture.builds.lock().unwrap()[0]
        .compaction_policy
        .clone()
        .expect("builder receives the admitted policy");
    assert_eq!(admitted.requested.tokens, 123_456);
    assert_eq!(admitted.requested.source, CompactionSource::Device);
    assert_eq!(admitted.choice, fixture.choice);
    assert_eq!(opened.tab.session.compaction_policy(), Some(&admitted));
    let wire_id = fixture.publish(opened).unwrap();
    let original = fixture.session();
    let (_, reused) = ensure_agent_tab(
        &fixture.state,
        &fixture.key.root,
        OWNER,
        &fixture.agent,
        &fixture.choice,
        "test",
    )
    .unwrap();
    assert_eq!(reused, Spawned::Warm);
    let attached = fixture.handler.call(
        SessionSender::detached("policy-attach"),
        req(
            "agent.attach",
            json!({"id": OWNER, "agent_id": fixture.agent}),
        ),
    );
    assert_eq!(attached["ok"], true, "{attached:?}");
    assert_eq!(attached["result"]["term_id"], wire_id);
    assert!(Arc::ptr_eq(&original, &fixture.session()));
    assert_eq!(fixture.session().compaction_policy(), Some(&admitted));
    assert_eq!(fixture.builds.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn resumed_session_captures_new_policy_and_forgets_previous_context_reading() {
    let fixture = PolicyLaunch::new(false);
    fixture.set_threshold(123_456);
    fixture.publish(fixture.open(fixture.reserve())).unwrap();
    let first = fixture.session().compaction_policy().unwrap().clone();
    {
        let mut app = fixture.state.lock().unwrap();
        let instance = app
            .session_registry
            .agent_snapshot(&fixture.key)
            .unwrap()
            .instance
            .unwrap();
        app.note_self_report(
            OWNER,
            &fixture.agent,
            &instance,
            SelfReport {
                named: Some("policy-resume-session".into()),
                model: None,
                effort: None,
            },
        );
        app.edit_agent_record("old context", OWNER, &fixture.agent, |agent| {
            agent.last_context_tokens = Some(456_789);
            agent.last_context_at = Some("2026-10-18T12:00:00Z".into());
        });
        app.retire_tab(&fixture.key, "test restart");
    }
    fixture.set_threshold(234_567);
    fixture.publish(fixture.open(fixture.reserve())).unwrap();
    let session = fixture.session();
    let next = session.compaction_policy().unwrap();
    assert_eq!(next.requested.tokens, 234_567);
    assert_ne!(next.launch_identity.id, first.launch_identity.id);
    assert_eq!(
        next.launch_identity.thread_id,
        first.launch_identity.thread_id
    );
    let builds = fixture.builds.lock().unwrap();
    assert_eq!(builds.len(), 2);
    assert_eq!(
        builds[1].resume_session_id.as_deref(),
        Some("policy-resume-session")
    );
    let app = fixture.state.lock().unwrap();
    let agent = app
        .entity_agents(OWNER)
        .unwrap()
        .by_id(&fixture.agent)
        .unwrap();
    assert_eq!(agent.last_context_tokens, None);
    assert_eq!(agent.last_context_at, None);
}

#[test]
fn failed_open_never_publishes_an_active_policy() {
    let fixture = PolicyLaunch::new(true);
    fixture.set_threshold(123_456);
    let result = open_agent_session(
        &fixture.state,
        fixture.reserve(),
        &fixture.key,
        &fixture.agent,
        &a_frame(&fixture.state),
    );
    assert!(result.is_err());
    assert!(fixture.builds.lock().unwrap()[0]
        .compaction_policy
        .is_some());
    let app = fixture.state.lock().unwrap();
    assert!(!app.session_registry.contains(&fixture.key));
    assert!(!app.session_registry.claim_is_held(&fixture.key));
    assert!(app.session_registry.test_token(&fixture.agent).is_none());
}

#[tokio::test]
async fn a_session_opened_for_an_old_thread_cannot_publish_its_policy() {
    let fixture = PolicyLaunch::new(false);
    let mut opened = fixture.open(fixture.reserve());
    opened.recorded_name_is_gone = true;
    let old_thread = opened
        .tab
        .session
        .compaction_policy()
        .unwrap()
        .launch_identity
        .thread_id
        .clone();
    fixture.state.lock().unwrap().edit_agent_record(
        "replace generation during open",
        OWNER,
        &fixture.agent,
        |agent| {
            agent.thread = crate::thread::Thread::new(&fixture.agent);
            agent.active_model = Some("new-generation-model".into());
            agent.active_effort = Some("high".into());
            agent.resume_session_id = Some("new-generation-resume".into());
        },
    );
    let waiting = {
        let mut app = fixture.state.lock().unwrap();
        app.session_registry
            .install_mcp_token(fixture.agent.clone(), "new-generation-token".into());
        app.session_registry.waiting_screen_for_attach(
            fixture.key.clone(),
            "new-generation-screen",
            80,
            24,
        )
    };
    waiting.attach(&SessionSender::detached("new-generation-client"), None);
    assert!(fixture.publish(opened).is_none());
    let app = fixture.state.lock().unwrap();
    assert!(!app.session_registry.contains(&fixture.key));
    assert!(!app.session_registry.claim_is_held(&fixture.key));
    let thread = app.agent_conversation(OWNER, Some(&fixture.agent)).unwrap();
    assert_ne!(thread.id, old_thread);
    assert!(thread.sessions.is_empty());
    let agent = app
        .entity_agents(OWNER)
        .unwrap()
        .by_id(&fixture.agent)
        .unwrap();
    assert_eq!(agent.active_model.as_deref(), Some("new-generation-model"));
    assert_eq!(agent.active_effort.as_deref(), Some("high"));
    assert_eq!(
        agent.resume_session_id.as_deref(),
        Some("new-generation-resume")
    );
    assert_eq!(
        app.session_registry.test_token(&fixture.agent),
        Some("new-generation-token")
    );
    assert!(app
        .session_registry
        .waiting_screen_keys()
        .contains(&fixture.key));
    assert_eq!(waiting.attached(), 1);
}

#[test]
fn router_admission_needs_no_canonical_conversation_or_compaction_policy() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let (owner, agent) = captured(&mut app, "route without a coding conversation");
    let router = app.router_sessions[&owner].clone();
    let key = TabKey::agent(&AppState::canonical_root(router.scratch_dir()), &agent);
    let state = app.shared();
    let reserved = reserve_agent_spawn(
        &mut state.lock().unwrap(),
        &key,
        &AgentSpawnRequest {
            owner: &owner,
            agent_id: &agent,
            conversation_id: &agent,
            model_choice: router.choice(),
            force_fresh: false,
            phase: "route",
        },
    )
    .expect("routers have no conversation policy to resolve");
    assert!(reserved.plan.launch.compaction_policy.is_none());
    reserved.holding.abandon(&state, (), &a_frame(&state));
    let app = state.lock().unwrap();
    assert!(!app.session_registry.claim_is_held(&key));
    assert!(app.session_registry.test_token(&agent).is_none());
}

#[test]
fn frozen_provider_resolves_canonical_alias_scopes_after_the_choice_changes() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let canonical_project = app.project_at(0).id.clone();
    let alias_project = app.add_project(init_repo_named(dir.path(), "alias-policy"), "main".into());
    let (owner, canonical) =
        super::super::project_agent::project_agent(&mut app, &canonical_project);
    let (alias_owner, alias) = super::super::project_agent::project_agent(&mut app, &alias_project);
    app.edit_agent_record("bind alias", &alias_owner, &alias, |agent| {
        agent.bind_conversation(&canonical);
        agent.choice.provider = AgentProvider::CodexAppServer;
        agent.max_context_tokens = Some(1);
    });
    app.patch_compaction_thresholds(
        CompactionScope::Project(canonical_project.clone()),
        &json!({"claude": 123_456, "codex_app_server": 234_567}),
    )
    .unwrap();
    app.patch_compaction_thresholds(
        CompactionScope::Project(alias_project),
        &json!({"claude": 2}),
    )
    .unwrap();
    let captured = app
        .resolve_compaction_policy_for_provider(&alias_owner, &alias, AgentProvider::Claude)
        .unwrap();
    assert_eq!(captured.tokens, 123_456);
    assert_eq!(
        captured.source,
        CompactionSource::Project(canonical_project)
    );
    assert_eq!(
        app.resolve_compaction_policy(&alias_owner, &alias)
            .unwrap()
            .tokens,
        234_567
    );
    app.set_conversation_context_limit(&owner, &canonical, Some(345_678))
        .unwrap();
    assert_eq!(
        app.resolve_compaction_policy_for_provider(&alias_owner, &alias, AgentProvider::Claude,)
            .unwrap()
            .tokens,
        345_678
    );
}

use super::*;
use crate::app::runtime::sessions::SecretReleaseAuthority;
use crate::secrets::SecretCaller;

struct Fixture {
    _state: tempfile::TempDir,
    _repo: tempfile::TempDir,
    app: AppState,
    project: String,
    owner: String,
    agent: String,
}

impl Fixture {
    fn new() -> Self {
        let state = tempfile::tempdir().unwrap();
        let (repo, mut app, project) = super::tracker::tracked(state.path());
        let (owner, _) = super::project_agent::project_agent(&mut app, &project);
        let mut candidate = app.take_run(&owner).unwrap();
        let agent = candidate
            .agents
            .add(&owner, candidate.model_choice.clone(), "now")
            .id
            .clone();
        app.finish_run_mutation(owner.clone(), candidate).unwrap();
        app.session_registry.set_secret_device("synthetic-device");
        app.initialize_read_context();
        Self {
            _state: state,
            _repo: repo,
            app,
            project,
            owner,
            agent,
        }
    }

    fn launch(&mut self) -> (SecretReleaseAuthority, String, SecretCaller) {
        let issuer = self.app.live_mcp_issuer(&self.owner, &self.agent).unwrap();
        let launch = self.app.mint_secret_launch(&issuer, &self.project).unwrap();
        let spec = crate::harness::secret_launch::apply(
            crate::pty::HarnessSpec::new("synthetic"),
            Some(launch),
        );
        let token = spec
            .env
            .iter()
            .find(|(name, _)| name == "BRIDGE_SECRET_TOKEN")
            .unwrap()
            .1
            .clone();
        let authority = self.app.secret_release_authority();
        let caller = authority
            .authenticate(&self.agent, &token)
            .expect("saved current issuer");
        (authority, token, caller)
    }
}

#[test]
fn secret_session_restart_rotates_only_process_authority_and_keeps_generation() {
    let mut fixture = Fixture::new();
    let (authority, old_token, old) = fixture.launch();
    let (_, fresh_token, fresh) = fixture.launch();
    assert!(authority.authenticate(&fixture.agent, &old_token).is_none());
    assert!(authority
        .authenticate(&fixture.agent, &fresh_token)
        .is_some());
    assert_eq!(old.generation, fresh.generation);
    assert_ne!(old.session_incarnation, fresh.session_incarnation);
    assert!(!authority.revalidate(&old));
    assert!(authority.revalidate(&fresh));
}

#[test]
fn secret_session_current_membership_and_reset_are_checked_on_every_validation() {
    let mut fixture = Fixture::new();
    let (authority, token, caller) = fixture.launch();
    fixture
        .app
        .session_registry
        .set_mcp_resetting(&caller.conversation_id, true);
    assert!(authority.authenticate(&fixture.agent, &token).is_none());
    fixture
        .app
        .session_registry
        .set_mcp_resetting(&caller.conversation_id, false);
    assert!(authority.revalidate(&caller));
    let mut candidate = fixture.app.take_run(&fixture.owner).unwrap();
    candidate.agents.remove(&fixture.agent).unwrap();
    fixture
        .app
        .finish_run_mutation(fixture.owner.clone(), candidate)
        .unwrap();
    assert!(authority.authenticate(&fixture.agent, &token).is_none());
}

#[test]
fn secret_session_caller_selectors_cannot_change_issued_authority() {
    let mut fixture = Fixture::new();
    let (authority, _, caller) = fixture.launch();
    let issuer = fixture
        .app
        .live_mcp_issuer(&fixture.owner, &fixture.agent)
        .unwrap();
    assert!(fixture
        .app
        .mint_secret_launch(&issuer, "another-project")
        .is_none());
    let changes: [fn(&mut SecretCaller); 10] = [
        |c| c.project_id = "another-project".into(),
        |c| c.owner_id = "another-owner".into(),
        |c| c.agent_id = "another-agent".into(),
        |c| c.canonical_owner_id = "another-owner".into(),
        |c| c.conversation_id = "another-conversation".into(),
        |c| c.generation = "another-generation".into(),
        |c| c.thread_id = "another-thread".into(),
        |c| c.device_id = "another-device".into(),
        |c| c.channel = "another-channel".into(),
        |c| c.session_incarnation = "another-incarnation".into(),
    ];
    for change in changes {
        let mut spoofed = caller.clone();
        change(&mut spoofed);
        assert!(!authority.revalidate(&spoofed));
    }
}

#[test]
fn secret_session_saved_generation_change_invalidates_the_capability() {
    let mut fixture = Fixture::new();
    let (authority, token, caller) = fixture.launch();
    let mut candidate = fixture.app.take_run(&fixture.owner).unwrap();
    let agent = candidate.agents.by_id_mut(&fixture.agent).unwrap();
    agent.thread.id = uuid::Uuid::new_v4().to_string();
    agent.thread.generation_revision += 1;
    fixture
        .app
        .finish_run_mutation(fixture.owner.clone(), candidate)
        .unwrap();
    assert!(authority.authenticate(&fixture.agent, &token).is_none());
    assert!(!authority.revalidate(&caller));
}

#[test]
fn secret_session_device_rebinding_clears_previous_launch_capabilities() {
    let mut fixture = Fixture::new();
    let (authority, token, caller) = fixture.launch();
    fixture
        .app
        .session_registry
        .set_secret_device("replacement-device");
    assert!(authority.authenticate(&fixture.agent, &token).is_none());
    assert!(!authority.revalidate(&caller));
}

#[test]
fn secret_session_candidate_roster_is_denied_until_its_durable_save() {
    let mut fixture = Fixture::new();
    let authority = fixture.app.secret_release_authority();
    let mut candidate = fixture.app.take_run(&fixture.owner).unwrap();
    let new_agent = candidate
        .agents
        .add(&fixture.owner, candidate.model_choice.clone(), "now")
        .id
        .clone();
    fixture.app.runs.insert(fixture.owner.clone(), candidate);
    let issuer = fixture
        .app
        .live_mcp_issuer(&fixture.owner, &new_agent)
        .unwrap();
    let launch = fixture
        .app
        .mint_secret_launch(&issuer, &fixture.project)
        .unwrap();
    let spec = crate::harness::secret_launch::apply(
        crate::pty::HarnessSpec::new("synthetic"),
        Some(launch),
    );
    let token = &spec
        .env
        .iter()
        .find(|(name, _)| name == "BRIDGE_SECRET_TOKEN")
        .unwrap()
        .1;
    assert!(authority.authenticate(&new_agent, token).is_none());
    let candidate = fixture.app.take_run(&fixture.owner).unwrap();
    fixture
        .app
        .finish_run_mutation(fixture.owner.clone(), candidate)
        .unwrap();
    assert!(authority.authenticate(&new_agent, token).is_some());
}

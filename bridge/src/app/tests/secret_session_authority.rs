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

#[test]
fn audit_identifiers_never_persist_a_launch_capability_or_uuid_value_as_request_id() {
    use crate::secrets::{GrantScope, SecretContext, SecretService, SecretValue, StageContext};
    let mut fixture = Fixture::new();
    let (authority, capability, caller) = fixture.launch();
    let vault = tempfile::tempdir().unwrap();
    let writer = fixture.app.store.as_ref().unwrap().writer();
    let service = SecretService::open(vault.path(), "synthetic-device", writer.clone()).unwrap();
    let value = uuid::Uuid::new_v4().to_string();
    let record = service
        .stage(
            SecretContext {
                id: format!("sec_{}", uuid::Uuid::new_v4()),
                device_id: caller.device_id.clone(),
                creator: "synthetic-user".into(),
                name: "SYNTHETIC_UUID_TOKEN".into(),
                ttl_seconds: None,
                grant: GrantScope::Agent {
                    agent_id: caller.agent_id.clone(),
                    conversation_id: caller.conversation_id.clone(),
                    generation: caller.generation.clone(),
                },
                stage: StageContext {
                    owner_id: caller.owner_id.clone(),
                    conversation_id: caller.conversation_id.clone(),
                    generation: caller.generation.clone(),
                    draft_id: None,
                },
            },
            SecretValue::new(value.as_bytes().to_vec()).unwrap(),
            uuid::Uuid::new_v4().to_string(),
            "a".repeat(64),
        )
        .unwrap();
    service
        .attach(
            std::slice::from_ref(&record.context.id),
            "synthetic-message",
        )
        .unwrap();
    for supplied_request in [&capability, &value, &capability] {
        let released = service
            .release(&caller, &record.context.id, supplied_request, || {
                authority.revalidate(&caller)
            })
            .unwrap();
        assert_eq!(released.as_bytes(), value.as_bytes());
    }
    let audit = writer
        .secret_audit_page(&record.context.id, 0, 100)
        .unwrap();
    assert_eq!(audit.len(), 3);
    let stored = serde_json::to_string(&audit).unwrap();
    assert!(
        !stored.contains(&capability),
        "launch capability must never enter durable audit"
    );
    assert!(
        !stored.contains(&value),
        "UUID-shaped value must never enter durable audit"
    );
    let mut identities = std::collections::HashSet::new();
    for event in audit {
        assert!(uuid::Uuid::parse_str(&event.request_id).is_ok());
        assert!(
            identities.insert(event.request_id),
            "every attempt gets a server identity"
        );
    }
}

fn assert_app_minted_secret_member(fixture: &mut Fixture) {
    let (authority, _, caller) = fixture.launch();
    let result: crate::secrets::Result<()> = fixture
        .app
        .store
        .as_ref()
        .unwrap()
        .writer()
        .secrets_release(
            &caller,
            &format!("sec_{}", uuid::Uuid::new_v4()),
            &uuid::Uuid::new_v4().to_string(),
            1000,
            || authority.revalidate(&caller),
            |_, _| panic!("the membership probe uses an unknown secret ID"),
        );
    assert!(
        matches!(result, Err(crate::secrets::SecretError::Unavailable)),
        "an app-minted member must pass SQL membership, not return Unauthorized: {result:?}"
    );
}

#[test]
fn secret_sql_membership_accepts_app_minted_workspace_agent() {
    let mut fixture = Fixture::new();
    let workspace = super::project_agent::workspace(
        &mut fixture.app,
        &fixture.project,
        "secret-membership-probe",
    );
    let ensured = fixture.app.handle(req(
        "workspace.ensure_conversation",
        json!({"workspace_id": workspace}),
    ));
    assert_eq!(ensured["ok"], true, "{ensured:?}");
    fixture.owner = ensured["result"]["run_id"].as_str().unwrap().into();
    let added = fixture
        .app
        .handle(req("agent.add", json!({"entity_id": fixture.owner})));
    assert_eq!(added["ok"], true, "{added:?}");
    fixture.agent = added["result"]["agent"]["id"].as_str().unwrap().into();
    assert_app_minted_secret_member(&mut fixture);
}

#[test]
fn secret_sql_membership_accepts_app_minted_project_coordinator() {
    let mut fixture = Fixture::new();
    (fixture.owner, fixture.agent) =
        super::project_agent::project_agent(&mut fixture.app, &fixture.project);
    assert_app_minted_secret_member(&mut fixture);
}

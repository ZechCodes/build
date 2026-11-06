use super::*;
use crate::secrets::{GrantScope, SecretContext, SecretValue, StageContext};

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
        let (repo, mut app, project) = super::super::tracker::tracked(state.path());
        let (owner, agent) = super::super::project_agent::project_agent(&mut app, &project);
        app = app
            .with_secret_vault(state.path(), "synthetic-device")
            .unwrap();
        assert!(app.secret_service().is_some());
        Self {
            _state: state,
            _repo: repo,
            app,
            project,
            owner,
            agent,
        }
    }
    fn generation(&self) -> String {
        self.app.runs[&self.owner]
            .agents
            .by_id(&self.agent)
            .unwrap()
            .thread
            .id
            .clone()
    }
    fn stage(&self, ttl: Option<i64>, draft: bool) -> String {
        let id = format!("sec_{}", uuid::Uuid::new_v4());
        let stage = StageContext {
            owner_id: self.owner.clone(),
            conversation_id: if draft {
                "provisional".into()
            } else {
                self.agent.clone()
            },
            generation: if draft {
                "provisional".into()
            } else {
                self.generation()
            },
            draft_id: draft.then(|| "30ed4ff2-bab5-4baa-8373-912c03f88ba5".into()),
        };
        self.app
            .secret_service()
            .unwrap()
            .stage(
                SecretContext {
                    id: id.clone(),
                    device_id: "synthetic-device".into(),
                    creator: "device_user".into(),
                    name: format!("TEST_{}", &uuid::Uuid::new_v4().simple().to_string()[..8]),
                    ttl_seconds: ttl,
                    grant: GrantScope::Agent {
                        agent_id: if draft {
                            "provisional".into()
                        } else {
                            self.agent.clone()
                        },
                        conversation_id: stage.conversation_id.clone(),
                        generation: stage.generation.clone(),
                    },
                    stage,
                },
                SecretValue::new(b"synthetic-test-value".to_vec()).unwrap(),
                uuid::Uuid::new_v4().to_string(),
                "a".repeat(64),
            )
            .unwrap();
        id
    }
    fn params(&self, ids: &[String]) -> Value {
        json!({"entity_id":self.owner,"agent_id":self.agent,"conversation_id":self.agent,"thread_id":self.generation(),"operation_id":"secret-send","body":"Use these test credentials","secret_ids":ids})
    }
    fn caller(&self) -> crate::secrets::SecretCaller {
        let thread = &self.app.runs[&self.owner]
            .agents
            .by_id(&self.agent)
            .unwrap()
            .thread;
        crate::secrets::SecretCaller {
            device_id: "synthetic-device".into(),
            channel: "dev".into(),
            project_id: self.project.clone(),
            owner_id: self.owner.clone(),
            canonical_owner_id: self.owner.clone(),
            agent_id: self.agent.clone(),
            conversation_id: self.agent.clone(),
            thread_id: thread.id.clone(),
            generation: thread.id.clone(),
            generation_revision: thread.generation_revision,
            session_incarnation: "synthetic-session".into(),
        }
    }

    fn send(&mut self, params: &Value) -> Result<Value, String> {
        self.app
            .thread_post_authenticated(params, &SessionSender::detached("synthetic-user"))
    }
}

#[test]
fn secret_post_atomically_claims_multiple_and_replays_the_original_message() {
    let mut f = Fixture::new();
    let first = f.stage(Some(60), false);
    let second = f.stage(None, false);
    let params = f.params(&[first.clone(), second.clone()]);
    let accepted = f.send(&params).unwrap();
    let writer = f.app.store.as_ref().unwrap().writer();
    let bound = writer.secret_record(&first).unwrap().unwrap();
    let permanent = writer.secret_record(&second).unwrap().unwrap();
    assert!(bound.attached_at.is_some());
    assert_eq!(bound.expires_at, bound.attached_at.map(|at| at + 60));
    assert_eq!(permanent.expires_at, None);
    let receipt = f.app.operation_receipt("secret-send").unwrap().unwrap();
    let message = receipt
        .delivery
        .as_ref()
        .unwrap()
        .payload
        .as_ref()
        .unwrap()
        .messages
        .last()
        .unwrap();
    assert_eq!(message.secrets.len(), 2);
    assert!(message
        .secrets
        .iter()
        .any(|secret| secret.secret_id == second));
    let original = message.clone();
    writer
        .secrets_revoke(&first, time::OffsetDateTime::now_utc().unix_timestamp())
        .unwrap();
    let before_sequence = f.app.runs[&f.owner]
        .agents
        .by_id(&f.agent)
        .unwrap()
        .thread
        .last_sequence();
    let queued_before = f.app.delivery_queue.queued().count();
    let replay = f.send(&params).unwrap();
    assert_eq!(
        f.app.runs[&f.owner]
            .agents
            .by_id(&f.agent)
            .unwrap()
            .thread
            .last_sequence(),
        before_sequence
    );
    assert_eq!(f.app.delivery_queue.queued().count(), queued_before);
    assert_eq!(accepted["operation_id"], replay["operation_id"]);
    let receipt = f.app.operation_receipt("secret-send").unwrap().unwrap();
    assert_eq!(
        &original,
        receipt
            .delivery
            .as_ref()
            .unwrap()
            .payload
            .as_ref()
            .unwrap()
            .messages
            .last()
            .unwrap()
    );
    assert_eq!(
        writer.secret_record(&first).unwrap().unwrap().attached_at,
        bound.attached_at
    );
    assert!(!serde_json::to_string(&receipt)
        .unwrap()
        .contains("synthetic-test-value"));
}

#[test]
fn secret_post_refuses_partial_sets_and_unauthenticated_or_agent_posts() {
    let mut f = Fixture::new();
    let id = f.stage(None, false);
    let missing = format!("sec_{}", uuid::Uuid::new_v4());
    let params = f.params(&[id.clone(), missing]);
    assert!(f.send(&params).is_err());
    assert!(f
        .app
        .store
        .as_ref()
        .unwrap()
        .writer()
        .secret_record(&id)
        .unwrap()
        .unwrap()
        .attached_at
        .is_none());
    let params = f.params(&[id]);
    assert!(f.app.thread_post(&params).is_err());
    let requester = crate::operation::OperationRequester {
        entity_id: f.owner.clone(),
        agent_id: f.agent.clone(),
        conversation_id: f.agent.clone(),
    };
    assert!(f.app.thread_post_from_agent(&params, requester).is_err());
    assert!(f.app.operation_receipt("secret-send").unwrap().is_none());
}

#[test]
fn secret_post_persistence_failure_does_not_claim_or_acknowledge() {
    let mut f = Fixture::new();
    let id = f.stage(None, false);
    let params = f.params(std::slice::from_ref(&id));
    f.app.store.as_ref().unwrap().fail_next_write();
    assert!(f.send(&params).is_err());
    assert!(f.app.operation_receipt("secret-send").unwrap().is_none());
    assert!(f
        .app
        .store
        .as_ref()
        .unwrap()
        .writer()
        .secret_record(&id)
        .unwrap()
        .unwrap()
        .attached_at
        .is_none());
    assert!(f.send(&params).is_ok());
}

#[test]
fn post_metadata_failure_restores_the_taken_run() {
    let mut f = Fixture::new();
    let id = f.stage(None, false);
    let params = f.params(&[]);
    let address = f
        .app
        .resolve_conversation_params(&f.owner, &params)
        .unwrap();
    let choice_revision = f
        .app
        .entity_agents(&f.owner)
        .unwrap()
        .resolve(Some(&f.agent))
        .unwrap()
        .choice_revision;
    let previous_thread = f.app.runs[&f.owner]
        .agents
        .by_id(&f.agent)
        .unwrap()
        .thread
        .clone();
    let record = f
        .app
        .store
        .as_ref()
        .unwrap()
        .writer()
        .secret_record(&id)
        .unwrap()
        .unwrap();
    let origin = crate::app::conversations::PostOrigin {
        operation_id: Some("post-metadata-failure".into()),
        secret_metadata: vec![crate::thread::MessageSecret::from_record(&record).unwrap()],
        ..Default::default()
    };
    let messages = crate::app::conversations::parse_thread_post_messages(
        &params,
        crate::thread::ArtifactKind::Diff,
        false,
    )
    .unwrap();
    f.app.session_registry.set_secret_device("");
    let error = f
        .app
        .append_addressed_post(
            &params,
            &address,
            choice_revision,
            origin,
            messages,
            Vec::new(),
            None,
            None,
            None,
        )
        .unwrap_err();
    assert_eq!(error, "secret environment unavailable");
    assert_eq!(
        f.app.runs[&f.owner].agents.by_id(&f.agent).unwrap().thread,
        previous_thread
    );
}

#[test]
fn post_metadata_failure_restores_the_taken_plan() {
    let mut f = Fixture::new();
    let id = f.stage(None, false);
    let plan = file_legacy_task(&mut f.app, "secret metadata rollback");
    let agent = f.app.plans[&plan].agents.sole().id.clone();
    let generation = f.app.plans[&plan].agents.sole_thread().id.clone();
    let params = json!({
        "entity_id": plan,
        "agent_id": agent,
        "conversation_id": agent,
        "thread_id": generation,
        "operation_id": "plan-post-metadata-failure",
        "body": "Use this test credential"
    });
    let address = f.app.resolve_conversation_params(&plan, &params).unwrap();
    let choice_revision = f.app.plans[&plan]
        .agents
        .resolve(Some(&agent))
        .unwrap()
        .choice_revision;
    let previous_thread = f.app.plans[&plan].agents.sole_thread().clone();
    let record = f
        .app
        .store
        .as_ref()
        .unwrap()
        .writer()
        .secret_record(&id)
        .unwrap()
        .unwrap();
    let origin = crate::app::conversations::PostOrigin {
        operation_id: Some("plan-post-metadata-failure".into()),
        secret_metadata: vec![crate::thread::MessageSecret::from_record(&record).unwrap()],
        ..Default::default()
    };
    let messages = crate::app::conversations::parse_thread_post_messages(
        &params,
        crate::thread::ArtifactKind::Plan,
        false,
    )
    .unwrap();
    f.app.session_registry.set_secret_device("");
    let error = f
        .app
        .append_addressed_post(
            &params,
            &address,
            choice_revision,
            origin,
            messages,
            Vec::new(),
            None,
            None,
            None,
        )
        .unwrap_err();
    assert_eq!(error, "secret environment unavailable");
    assert_eq!(f.app.plans[&plan].agents.sole_thread(), &previous_thread);
}

#[test]
fn post_acceptance_failure_restores_the_taken_run() {
    let mut f = Fixture::new();
    let params = f.params(&[]);
    let address = f
        .app
        .resolve_conversation_params(&f.owner, &params)
        .unwrap();
    let choice_revision = f.app.runs[&f.owner]
        .agents
        .resolve(Some(&f.agent))
        .unwrap()
        .choice_revision;
    let previous_thread = f.app.runs[&f.owner].agents.sole_thread().clone();
    let error = f
        .app
        .append_addressed_post(
            &params,
            &address,
            choice_revision,
            crate::app::conversations::PostOrigin {
                operation_id: Some("post-acceptance-failure".into()),
                ..Default::default()
            },
            Vec::new(),
            Vec::new(),
            None,
            None,
            None,
        )
        .unwrap_err();
    assert_eq!(error, "thread.post did not append a message");
    assert_eq!(f.app.runs[&f.owner].agents.sole_thread(), &previous_thread);
}

#[test]
fn post_acceptance_failure_restores_the_taken_plan() {
    let mut f = Fixture::new();
    let plan = file_legacy_task(&mut f.app, "post acceptance rollback");
    f.app
        .plans
        .get_mut(&plan)
        .unwrap()
        .agents
        .sole_thread_mut()
        .items
        .clear();
    let agent = f.app.plans[&plan].agents.sole().id.clone();
    let generation = f.app.plans[&plan].agents.sole_thread().id.clone();
    let params = json!({
        "entity_id": plan,
        "agent_id": agent,
        "conversation_id": agent,
        "thread_id": generation,
        "operation_id": "plan-post-acceptance-failure",
        "body": "This message is intentionally omitted"
    });
    let address = f.app.resolve_conversation_params(&plan, &params).unwrap();
    let choice_revision = f.app.plans[&plan]
        .agents
        .resolve(Some(&agent))
        .unwrap()
        .choice_revision;
    let previous_thread = f.app.plans[&plan].agents.sole_thread().clone();
    let error = f
        .app
        .append_addressed_post(
            &params,
            &address,
            choice_revision,
            crate::app::conversations::PostOrigin {
                operation_id: Some("plan-post-acceptance-failure".into()),
                ..Default::default()
            },
            Vec::new(),
            Vec::new(),
            None,
            None,
            None,
        )
        .unwrap_err();
    assert_eq!(error, "thread.post did not append a message");
    assert_eq!(f.app.plans[&plan].agents.sole_thread(), &previous_thread);
}

#[test]
fn conversation_retirement_immediately_publishes_secret_changes() {
    let f = Fixture::new();
    let id = f.stage(None, false);
    let (browser, mut rx, key) = SessionSender::observable("secret-retirement");
    f.app.changes.subscribe(
        &browser,
        crate::changes::SubscriptionSpec {
            id: "secret-retirement".into(),
            scope: crate::changes::Scope::All,
            kinds: [crate::changes::Kind::SecretsChanged].into_iter().collect(),
            mode: Default::default(),
            priority: Default::default(),
        },
    );
    let agent = f.app.runs[&f.owner].agents.by_id(&f.agent).unwrap().clone();
    f.app
        .store
        .as_ref()
        .unwrap()
        .reset_conversation(
            &f.agent,
            &[agent],
            &std::collections::HashMap::new(),
            &std::collections::HashSet::new(),
        )
        .unwrap();
    assert_eq!(f.app.changes.flush(), 1);
    let event = SessionSender::decrypt_push(&key, &rx.try_recv().unwrap());
    assert_eq!(event["type"], "secrets.changed");
    assert_eq!(event["secret_ids"], json!([id]));
}

#[test]
fn secret_post_binds_provisional_recipient_without_rewriting_encrypted_context() {
    let mut f = Fixture::new();
    let id = f.stage(Some(60), true);
    let mut params = f.params(std::slice::from_ref(&id));
    assert!(f.send(&params).is_err());
    params["draft_id"] = json!("30ed4ff2-bab5-4baa-8373-912c03f88ba5");
    assert!(f.send(&params).is_ok());
    let record = f
        .app
        .store
        .as_ref()
        .unwrap()
        .writer()
        .secret_record(&id)
        .unwrap()
        .unwrap();
    assert_eq!(record.context.stage.generation, "provisional");
    let conn = f.app.store.as_ref().unwrap().writer_for_test();
    let generation: String = conn
        .query_row(
            "SELECT generation FROM secret_final_bindings WHERE secret_id=?1",
            [id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(generation, f.generation());
    assert_eq!(record.context.stage.owner_id, f.owner);
    assert!(!f.project.is_empty());
}

#[test]
fn secret_post_provisional_release_resolves_final_recipient_and_refuses_unbound() {
    let mut fixture = Fixture::new();
    let id = fixture.stage(None, true);
    let service = fixture.app.secret_service().unwrap();
    let caller = fixture.caller();
    assert!(matches!(
        service.release(&caller, &id, "before-bind", || true),
        Err(crate::secrets::SecretError::Unavailable)
    ));
    let mut params = fixture.params(std::slice::from_ref(&id));
    params["draft_id"] = json!("30ed4ff2-bab5-4baa-8373-912c03f88ba5");
    fixture.send(&params).unwrap();
    let value = service
        .release(&caller, &id, "after-bind", || true)
        .unwrap();
    assert_eq!(value.as_bytes(), b"synthetic-test-value");
    let mut wrong = caller.clone();
    wrong.generation = "provisional".into();
    wrong.thread_id = "provisional".into();
    assert!(service
        .release(&wrong, &id, "wrong-recipient", || true)
        .is_err());
    let record = fixture
        .app
        .store
        .as_ref()
        .unwrap()
        .writer()
        .secret_record(&id)
        .unwrap()
        .unwrap();
    assert_eq!(record.context.stage.generation, "provisional");
}

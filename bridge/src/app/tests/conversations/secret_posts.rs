use super::*;
use crate::execution::CallContext;
use crate::secrets::upload::{UploadContext, UploadOpener};
use crate::secrets::{GrantScope, SecretContext, SecretValue, StageContext};
use std::sync::Arc;

struct SyntheticSweepOpener;
impl UploadOpener for SyntheticSweepOpener {
    fn open(&self, sealed: &str, _: &UploadContext) -> crate::secrets::Result<SecretValue> {
        if sealed != "synthetic-sweep-ciphertext" {
            return Err(crate::secrets::SecretError::Invalid);
        }
        SecretValue::new(b"synthetic-test-value".to_vec())
    }
}

struct Fixture {
    state: tempfile::TempDir,
    repo: tempfile::TempDir,
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
            .unwrap()
            .with_secret_upload_opener(Arc::new(SyntheticSweepOpener));
        assert!(app.secret_service().is_some());
        Self {
            state,
            repo,
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
    fn stage_wire(&mut self) -> String {
        let request_id = uuid::Uuid::new_v4().to_string();
        let generation = self.generation();
        let created = self
            .app
            .dispatch_api_with_context(
                "secrets.create",
                &json!({
                    "request_id": request_id,
                    "device_id": "synthetic-device",
                    "creator": "device_user",
                    "scope": {
                        "owner_id": self.owner,
                        "conversation_id": self.agent,
                        "generation": generation
                    },
                    "grant": {
                        "scope": "agent",
                        "agent_id": self.agent,
                        "conversation_id": self.agent,
                        "generation": generation
                    },
                    "name": "SYNTHETIC_SWEEP_TOKEN",
                    "ttl_seconds": null,
                    "sealed_payload": "synthetic-sweep-ciphertext"
                }),
                Some(&CallContext::detached("synthetic-owner")),
            )
            .unwrap();
        created["secret_id"].as_str().unwrap().to_string()
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

    fn assert_no_incidental_secret(&self, needle: &[u8], id: &str, pushes: &[Value]) {
        let negative = tempfile::tempdir().unwrap();
        git(negative.path(), &["init"]);
        let synthetic_leak = negative.path().join("qualification-negative-control");
        std::fs::write(&synthetic_leak, needle).unwrap();
        git(negative.path(), &["add", "qualification-negative-control"]);
        git(
            negative.path(),
            &[
                "-c",
                "user.name=Synthetic Qualification",
                "-c",
                "user.email=synthetic@example.invalid",
                "commit",
                "-m",
                "synthetic negative control",
            ],
        );
        git(negative.path(), &["rm", "qualification-negative-control"]);
        git(
            negative.path(),
            &[
                "-c",
                "user.name=Synthetic Qualification",
                "-c",
                "user.email=synthetic@example.invalid",
                "commit",
                "-m",
                "remove synthetic negative control",
            ],
        );
        let negative_root = negative.path().to_owned();
        let negative_needle = needle.to_vec();
        assert!(std::panic::catch_unwind(move || {
            assert_git_excludes(&negative_root, &negative_needle)
        })
        .is_err());

        assert_tree_excludes(self.state.path(), needle);
        assert_tree_excludes(self.repo.path(), needle);
        assert_git_excludes(
            &self.app.projects.get(&self.project).unwrap().repo_path,
            needle,
        );
        let writer = self.app.store.as_ref().unwrap().writer();
        let audit = writer.secret_audit_page(id, 0, 100).unwrap();
        assert!(!serde_json::to_vec(&audit)
            .unwrap()
            .windows(needle.len())
            .any(|part| part == needle));
        let receipt = self.app.operation_receipt("secret-send").unwrap().unwrap();
        let saved = serde_json::to_vec(&receipt).unwrap();
        assert!(!saved.windows(needle.len()).any(|part| part == needle));
        let payload = receipt.delivery.as_ref().unwrap().payload.as_ref().unwrap();
        for provider in crate::models::AgentProvider::ALL {
            let prompt = payload.delivery_prompt("secret-send", false, provider);
            assert!(!prompt
                .as_bytes()
                .windows(needle.len())
                .any(|part| part == needle));
        }
        assert!(pushes.iter().any(push_has_secret_change));
        assert_pushes_exclude(pushes, needle);
        let leaking_push = json!({
            "type": "synthetic.negative_control",
            "payload": String::from_utf8(needle.to_vec()).unwrap()
        });
        let negative_needle = needle.to_vec();
        assert!(std::panic::catch_unwind(move || {
            assert_pushes_exclude(&[leaking_push], &negative_needle)
        })
        .is_err());
    }
}

fn git(root: &std::path::Path, args: &[&str]) -> Vec<u8> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success(), "synthetic git command failed");
    output.stdout
}

fn assert_git_excludes(root: &std::path::Path, needle: &[u8]) {
    for args in [
        &["log", "-p", "--all", "--no-ext-diff", "--binary"][..],
        &["cat-file", "--batch-all-objects", "--batch"][..],
    ] {
        let decoded = git(root, args);
        assert!(
            !decoded.windows(needle.len()).any(|part| part == needle),
            "synthetic secret found in decoded git history"
        );
    }
}

fn push_has_secret_change(push: &Value) -> bool {
    push["type"] == "secrets.changed"
        || push["items"]
            .as_array()
            .is_some_and(|items| items.iter().any(|item| item["kind"] == "secrets"))
}

fn assert_pushes_exclude(pushes: &[Value], needle: &[u8]) {
    for push in pushes {
        let encoded = serde_json::to_vec(push).unwrap();
        assert!(
            !encoded.windows(needle.len()).any(|part| part == needle),
            "synthetic secret found in decrypted push"
        );
    }
}

fn assert_tree_excludes(root: &std::path::Path, needle: &[u8]) {
    for entry in std::fs::read_dir(root).unwrap() {
        let entry = entry.unwrap();
        let file_type = entry.file_type().unwrap();
        if file_type.is_dir() {
            assert_tree_excludes(&entry.path(), needle);
        } else if file_type.is_file() {
            let bytes = std::fs::read(entry.path()).unwrap();
            assert!(
                !bytes.windows(needle.len()).any(|part| part == needle),
                "synthetic secret found in incidental artifact {}",
                entry.path().display()
            );
        }
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
fn accepted_secret_leak_sweep_excludes_every_incidental_bridge_artifact() {
    let mut fixture = Fixture::new();
    let (browser, mut rx, key) = SessionSender::observable("qualification-push");
    fixture.app.changes.subscribe(
        &browser,
        crate::changes::SubscriptionSpec {
            id: "qualification-push".into(),
            scope: crate::changes::Scope::All,
            kinds: crate::changes::Kind::ALL.into_iter().collect(),
            mode: Default::default(),
            priority: Default::default(),
        },
    );
    let id = fixture.stage_wire();
    fixture.app.changes.flush();
    let params = fixture.params(std::slice::from_ref(&id));
    fixture.send(&params).unwrap();
    fixture.app.changes.flush();
    let value = fixture
        .app
        .secret_service()
        .unwrap()
        .release(&fixture.caller(), &id, "qualification-release", || true)
        .unwrap();
    assert_eq!(value.as_bytes(), b"synthetic-test-value");
    fixture.app.changes.flush();
    fixture.app.secret_service().unwrap().revoke(&id).unwrap();
    fixture.app.changes.flush();
    let mut pushes = Vec::new();
    while let Ok(envelope) = rx.try_recv() {
        pushes.push(SessionSender::decrypt_push(&key, &envelope));
    }
    fixture.assert_no_incidental_secret(b"synthetic-test-value", &id, &pushes);
}

#[test]
fn project_delete_entry_point_revokes_project_scoped_secret() {
    let mut fixture = Fixture::new();
    let id = format!("sec_{}", uuid::Uuid::new_v4());
    let generation = fixture.generation();
    let caller = fixture.caller();
    fixture
        .app
        .secret_service()
        .unwrap()
        .stage(
            SecretContext {
                id: id.clone(),
                device_id: "synthetic-device".into(),
                creator: "device_user".into(),
                name: "SYNTHETIC_PROJECT_DELETE_TOKEN".into(),
                ttl_seconds: None,
                grant: GrantScope::Project {
                    project_id: fixture.project.clone(),
                },
                stage: StageContext {
                    owner_id: fixture.owner.clone(),
                    conversation_id: fixture.agent.clone(),
                    generation,
                    draft_id: None,
                },
            },
            SecretValue::new(b"synthetic-project-delete-value".to_vec()).unwrap(),
            uuid::Uuid::new_v4().to_string(),
            "b".repeat(64),
        )
        .unwrap();
    fixture
        .app
        .secret_service()
        .unwrap()
        .attach(std::slice::from_ref(&id), "synthetic-project-delete-send")
        .unwrap();
    assert!(fixture
        .app
        .secret_service()
        .unwrap()
        .release(&caller, &id, "before-project-delete", || true)
        .is_ok());

    let deleted = fixture.app.handle(req(
        "project.delete",
        json!({"project_id": fixture.project, "confirm": true}),
    ));
    assert_eq!(deleted["ok"], true, "{deleted:?}");
    let record = fixture
        .app
        .store
        .as_ref()
        .unwrap()
        .writer()
        .secret_record(&id)
        .unwrap()
        .unwrap();
    assert!(record.revoked_at.is_some());
    assert!(fixture
        .app
        .secret_service()
        .unwrap()
        .release(&caller, &id, "after-project-delete", || true)
        .is_err());
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

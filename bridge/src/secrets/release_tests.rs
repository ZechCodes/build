use super::*;
use crate::agent::Agent;
use crate::models::ModelChoice;
use crate::secrets::{GrantScope, SecretCaller, StageContext};
use crate::store::Store;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

struct TestClock(AtomicI64, AtomicU64);
impl Clock for TestClock {
    fn wall_seconds(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
    fn monotonic_seconds(&self) -> u64 {
        self.1.load(Ordering::SeqCst)
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    store: Store,
    service: SecretService,
    clock: Arc<TestClock>,
    caller: SecretCaller,
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("tasks")).unwrap();
        let agent = Agent::new(
            "synthetic-agent",
            "synthetic-project",
            ModelChoice::default(),
            1,
            "2026-11-04T00:00:00Z",
        );
        store
            .writer_for_test()
            .execute(
                "INSERT INTO agents(id,owner_id,ordinal,record) VALUES (?1,?2,1,?3)",
                rusqlite::params![
                    agent.id,
                    agent.owner_id,
                    serde_json::to_string(&agent).unwrap()
                ],
            )
            .unwrap();
        store.writer_for_test().execute_batch("INSERT INTO device_project_ids(path,project_id) VALUES ('/synthetic-project','synthetic-project'); INSERT INTO device_owner_projects(owner_id,project_path) VALUES ('synthetic-project','/synthetic-project')").unwrap();
        crate::store::secrets::seed_current_project_for_test(&store, "synthetic-project");
        let caller = SecretCaller {
            device_id: "synthetic-device".into(),
            channel: "dev".into(),
            project_id: "synthetic-project".into(),
            owner_id: agent.owner_id.clone(),
            canonical_owner_id: agent.owner_id.clone(),
            agent_id: agent.id.clone(),
            conversation_id: agent.id.clone(),
            generation: agent.thread.id.clone(),
            thread_id: agent.thread.id.clone(),
            generation_revision: agent.thread.generation_revision,
            session_incarnation: "synthetic-session".into(),
        };
        let clock = Arc::new(TestClock(AtomicI64::new(1000), AtomicU64::new(0)));
        let service = SecretService::open_with_clock(
            dir.path(),
            &caller.device_id,
            store.writer(),
            clock.clone(),
        )
        .unwrap();
        Self {
            _dir: dir,
            store,
            service,
            clock,
            caller,
        }
    }
    fn add_member(&self, id: &str, owner: &str, project: &str, coordinator: bool) -> SecretCaller {
        crate::store::secrets::seed_current_project_for_test(&self.store, project);
        let mut agent = Agent::new(id, owner, ModelChoice::default(), 2, "2026-11-04T00:00:00Z");
        if coordinator {
            agent.project_role = Some(crate::project_roster::ProjectRole::Coordinator);
        }
        let connection = self.store.writer_for_test();
        connection
            .execute(
                "INSERT INTO agents(id,owner_id,ordinal,record) VALUES(?1,?2,2,?3)",
                rusqlite::params![id, owner, serde_json::to_string(&agent).unwrap()],
            )
            .unwrap();
        let path = format!("/synthetic-project-{project}");
        connection
            .execute(
                "INSERT OR IGNORE INTO device_project_ids(path,project_id) VALUES(?1,?2)",
                rusqlite::params![path, project],
            )
            .unwrap();
        connection.execute("INSERT INTO device_owner_projects(owner_id,project_path) VALUES(?1,?2) ON CONFLICT(owner_id) DO UPDATE SET project_path=excluded.project_path",rusqlite::params![owner,path]).unwrap();
        SecretCaller {
            device_id: self.caller.device_id.clone(),
            channel: self.caller.channel.clone(),
            project_id: project.into(),
            owner_id: owner.into(),
            canonical_owner_id: owner.into(),
            agent_id: id.into(),
            conversation_id: id.into(),
            thread_id: agent.thread.id.clone(),
            generation: agent.thread.id,
            generation_revision: agent.thread.generation_revision,
            session_incarnation: format!("synthetic-session-{id}"),
        }
    }
    fn secret(&self, grant: GrantScope, ttl: Option<i64>) -> SecretRecord {
        let context = SecretContext {
            id: format!("sec_{}", uuid::Uuid::new_v4()),
            device_id: self.caller.device_id.clone(),
            creator: "synthetic-user".into(),
            name: "SYNTHETIC_TOKEN".into(),
            ttl_seconds: ttl,
            grant,
            stage: StageContext {
                owner_id: self.caller.owner_id.clone(),
                conversation_id: self.caller.conversation_id.clone(),
                generation: self.caller.generation.clone(),
                draft_id: None,
            },
        };
        let record = self
            .service
            .stage(
                context,
                SecretValue::new(b"synthetic-value-only".to_vec()).unwrap(),
                uuid::Uuid::new_v4().to_string(),
                "a".repeat(64),
            )
            .unwrap();
        self.service
            .attach(
                std::slice::from_ref(&record.context.id),
                &format!("synthetic-send-{}", record.context.id),
            )
            .unwrap();
        record
    }
    fn agent_grant(&self) -> GrantScope {
        GrantScope::Agent {
            agent_id: self.caller.agent_id.clone(),
            conversation_id: self.caller.conversation_id.clone(),
            generation: self.caller.generation.clone(),
        }
    }
    fn release(&self, id: &str) -> Result<SecretValue> {
        self.service
            .release(&self.caller, id, "synthetic-request", || true)
    }
}

#[test]
fn release_scope_matrix_hides_denied_tombstones_and_unknown_ids() {
    let f = Fixture::new();
    for grant in [
        f.agent_grant(),
        GrantScope::Project {
            project_id: f.caller.project_id.clone(),
        },
    ] {
        let rec = f.secret(grant, None);
        assert_eq!(
            f.release(&rec.context.id).unwrap().as_bytes(),
            b"synthetic-value-only"
        );
    }
    for grant in [
        GrantScope::Agent {
            agent_id: "synthetic-sibling".into(),
            conversation_id: f.caller.conversation_id.clone(),
            generation: f.caller.generation.clone(),
        },
        GrantScope::Agent {
            agent_id: f.caller.agent_id.clone(),
            conversation_id: f.caller.conversation_id.clone(),
            generation: "synthetic-old-generation".into(),
        },
        GrantScope::Project {
            project_id: "synthetic-other-project".into(),
        },
    ] {
        let rec = f.secret(grant, None);
        f.service.revoke(&rec.context.id).unwrap();
        assert!(matches!(
            f.release(&rec.context.id),
            Err(SecretError::Unavailable)
        ));
    }
    assert!(matches!(
        f.release(&format!("sec_{}", uuid::Uuid::new_v4())),
        Err(SecretError::Unavailable)
    ));
}
#[test]
fn authorized_expiry_revoke_and_callback_staleness_are_distinct() {
    let f = Fixture::new();
    let rec = f.secret(f.agent_grant(), Some(10));
    assert!(matches!(
        f.service
            .release(&f.caller, &rec.context.id, "synthetic-request", || false),
        Err(SecretError::Unauthorized)
    ));
    f.clock.0.store(1010, Ordering::SeqCst);
    f.clock.1.store(10, Ordering::SeqCst);
    assert!(matches!(
        f.release(&rec.context.id),
        Err(SecretError::Expired)
    ));
    let rec = f.secret(f.agent_grant(), None);
    f.service.revoke(&rec.context.id).unwrap();
    assert!(matches!(
        f.release(&rec.context.id),
        Err(SecretError::Revoked)
    ));
}
#[test]
fn no_durable_audit_means_no_release_and_summary_survives_detail_pruning() {
    let f = Fixture::new();
    let rec = f.secret(f.agent_grant(), None);
    f.store.writer_for_test().execute_batch("CREATE TRIGGER synthetic_fail_audit BEFORE INSERT ON secret_audit BEGIN SELECT RAISE(ABORT,'synthetic audit failure'); END").unwrap();
    assert!(f.release(&rec.context.id).is_err());
    f.store
        .writer_for_test()
        .execute_batch("DROP TRIGGER synthetic_fail_audit")
        .unwrap();
    f.release(&rec.context.id).unwrap();
    let writer = f.store.writer();
    assert_eq!(
        writer
            .secret_audit_summary(&rec.context.id)
            .unwrap()
            .unwrap()
            .release_count,
        1
    );
    assert_eq!(
        writer
            .secret_audit_page(&rec.context.id, 0, 100)
            .unwrap()
            .len(),
        1
    );
    writer.secrets_audit_maintain(1000 + 91 * 86400).unwrap();
    assert!(writer
        .secret_audit_page(&rec.context.id, 0, 100)
        .unwrap()
        .is_empty());
    assert_eq!(
        writer
            .secret_audit_summary(&rec.context.id)
            .unwrap()
            .unwrap()
            .release_count,
        1
    );
}
#[test]
fn durable_removed_member_cannot_read_project_before_app_publication() {
    let f = Fixture::new();
    let rec = f.secret(
        GrantScope::Project {
            project_id: f.caller.project_id.clone(),
        },
        None,
    );
    f.store
        .writer_for_test()
        .execute("DELETE FROM agents WHERE id=?1", [&f.caller.agent_id])
        .unwrap();
    assert!(matches!(
        f.release(&rec.context.id),
        Err(SecretError::Unauthorized)
    ));
}

#[test]
fn expiry_is_sampled_after_waiting_for_release_metadata_lock() {
    let f = Fixture::new();
    let rec = f.secret(f.agent_grant(), Some(10));
    let result = f
        .service
        .release(&f.caller, &rec.context.id, "synthetic-request", || {
            f.clock.0.store(1010, Ordering::SeqCst);
            f.clock.1.store(10, Ordering::SeqCst);
            true
        });
    assert!(matches!(result, Err(SecretError::Expired)));
}

#[test]
fn release_and_revoke_have_one_linearization_order() {
    let f = Fixture::new();
    let rec = f.secret(f.agent_grant(), None);
    let entered = Arc::new(std::sync::Barrier::new(2));
    let leave = Arc::new(std::sync::Barrier::new(2));
    std::thread::scope(|scope| {
        let release = scope.spawn(|| {
            f.service
                .release(&f.caller, &rec.context.id, "synthetic-request", || {
                    entered.wait();
                    leave.wait();
                    true
                })
        });
        entered.wait();
        let revoke = scope.spawn(|| f.service.revoke(&rec.context.id));
        leave.wait();
        assert!(release.join().unwrap().is_ok());
        revoke.join().unwrap().unwrap();
    });
    assert!(matches!(
        f.release(&rec.context.id),
        Err(SecretError::Revoked)
    ));
    assert_eq!(
        f.store
            .writer()
            .secret_audit_summary(&rec.context.id)
            .unwrap()
            .unwrap()
            .release_count,
        1
    );
}

#[test]
fn reset_durable_generation_rejects_stale_project_capability() {
    let f = Fixture::new();
    let rec = f.secret(
        GrantScope::Project {
            project_id: f.caller.project_id.clone(),
        },
        None,
    );
    let raw: String = f
        .store
        .writer_for_test()
        .query_row(
            "SELECT record FROM agents WHERE id=?1",
            [&f.caller.agent_id],
            |row| row.get(0),
        )
        .unwrap();
    let mut agent: Agent = serde_json::from_str(&raw).unwrap();
    agent.thread.generation_revision += 1;
    agent.thread.id = "thread:synthetic-reset-generation".into();
    f.store
        .writer_for_test()
        .execute(
            "UPDATE agents SET record=?2 WHERE id=?1",
            rusqlite::params![agent.id, serde_json::to_string(&agent).unwrap()],
        )
        .unwrap();
    assert!(matches!(
        f.release(&rec.context.id),
        Err(SecretError::Unauthorized)
    ));
}

#[test]
fn unauthenticated_failures_are_coalesced_and_never_store_selectors() {
    let f = Fixture::new();
    for _ in 0..1000 {
        f.service.record_unauthenticated_failure();
    }
    let connection = f.store.writer_for_test();
    let rows: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM secret_unauthenticated_summary",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let count: i64 = connection
        .query_row(
            "SELECT attempt_count FROM secret_unauthenticated_summary",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(rows, 1);
    assert_eq!(count, 1);
    drop(connection);
    f.service.sweep().unwrap();
    let count: i64 = f
        .store
        .writer_for_test()
        .query_row(
            "SELECT attempt_count FROM secret_unauthenticated_summary",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1000);
}

#[test]
fn audit_pagination_is_bounded_and_every_release_is_individually_attributable() {
    let f = Fixture::new();
    let rec = f.secret(f.agent_grant(), None);
    for request in ["synthetic-request-1", "synthetic-request-2"] {
        f.service
            .release(&f.caller, &rec.context.id, request, || true)
            .unwrap();
    }
    let writer = f.store.writer();
    assert!(writer.secret_audit_page(&rec.context.id, 0, 101).is_err());
    assert!(writer.secret_audit_page(&rec.context.id, 0, 0).is_err());
    let page = writer.secret_audit_page(&rec.context.id, 0, 1).unwrap();
    assert_eq!(page.len(), 1);
    assert!(uuid::Uuid::parse_str(&page[0].request_id).is_ok());
    assert_ne!(page[0].request_id, "synthetic-request-1");
    assert_eq!(page[0].session_incarnation, f.caller.session_incarnation);
    let next = writer
        .secret_audit_page(&rec.context.id, page[0].sequence, 1)
        .unwrap();
    assert!(uuid::Uuid::parse_str(&next[0].request_id).is_ok());
    assert_ne!(next[0].request_id, "synthetic-request-2");
    assert_ne!(page[0].request_id, next[0].request_id);
}

#[test]
fn durable_project_rebind_closes_prepublication_window() {
    let f = Fixture::new();
    let rec = f.secret(
        GrantScope::Project {
            project_id: f.caller.project_id.clone(),
        },
        None,
    );
    f.store
        .writer_for_test()
        .execute(
            "UPDATE device_project_ids SET project_id='synthetic-other-project'",
            [],
        )
        .unwrap();
    assert!(matches!(
        f.release(&rec.context.id),
        Err(SecretError::Unauthorized)
    ));
}

#[test]
fn release_job_budget_is_shared_with_uploads_and_fails_without_audit() {
    let f = Fixture::new();
    let rec = f.secret(f.agent_grant(), None);
    let _held = f
        .service
        .inner
        .jobs
        .clone()
        .try_acquire_many_owned(MAX_CONCURRENT_JOBS as u32)
        .unwrap();
    assert!(matches!(
        f.release(&rec.context.id),
        Err(SecretError::Limit)
    ));
    assert!(f
        .store
        .writer()
        .secret_audit_page(&rec.context.id, 0, 100)
        .unwrap()
        .is_empty());
}

#[test]
fn ignored_audit_or_summary_write_cannot_release() {
    for table in ["secret_audit", "secret_audit_summaries"] {
        let f = Fixture::new();
        let rec = f.secret(f.agent_grant(), None);
        f.store.writer_for_test().execute_batch(&format!("CREATE TRIGGER synthetic_ignore BEFORE INSERT ON {table} BEGIN SELECT RAISE(IGNORE); END")).unwrap();
        assert!(f.release(&rec.context.id).is_err());
        assert!(f
            .store
            .writer()
            .secret_audit_page(&rec.context.id, 0, 100)
            .unwrap()
            .is_empty());
    }
}

#[test]
fn detail_audit_and_summaries_survive_store_restart_without_secret_value() {
    let f = Fixture::new();
    let rec = f.secret(f.agent_grant(), None);
    f.release(&rec.context.id).unwrap();
    let events = f
        .store
        .writer()
        .secret_audit_page(&rec.context.id, 0, 100)
        .unwrap();
    let json = serde_json::to_string(&events).unwrap();
    assert!(!json.contains("synthetic-value-only"));
    assert!(!json.contains("SYNTHETIC_TOKEN"));
    let Fixture {
        _dir,
        store,
        service,
        ..
    } = f;
    drop(service);
    drop(store);
    let store = Store::new(_dir.path().join("tasks")).unwrap();
    assert_eq!(
        store
            .writer()
            .secret_audit_page(&rec.context.id, 0, 100)
            .unwrap(),
        events
    );
    assert_eq!(
        store
            .writer()
            .secret_audit_summary(&rec.context.id)
            .unwrap()
            .unwrap()
            .release_count,
        1
    );
}

#[test]
fn failed_unauthenticated_counter_flush_does_not_retry_on_every_attempt() {
    let f = Fixture::new();
    f.store.writer_for_test().execute_batch("CREATE TRIGGER synthetic_fail_unauth BEFORE INSERT ON secret_unauthenticated_summary BEGIN SELECT RAISE(ABORT,'synthetic counter failure'); END").unwrap();
    f.service.record_unauthenticated_failure();
    let connection = f.store.writer_for_test();
    let (sent, received) = std::sync::mpsc::channel();
    let completed = std::thread::scope(|scope| {
        scope.spawn(|| {
            f.service.record_unauthenticated_failure();
            sent.send(()).unwrap();
        });
        let completed = received
            .recv_timeout(std::time::Duration::from_secs(2))
            .is_ok();
        drop(connection);
        completed
    });
    assert!(
        completed,
        "coalesced failure must not wait for the database writer"
    );
}

#[test]
fn canonical_owner_project_rebind_rejects_alias_before_app_publication() {
    let mut f = Fixture::new();
    let raw: String = f
        .store
        .writer_for_test()
        .query_row(
            "SELECT record FROM agents WHERE id=?1",
            [&f.caller.agent_id],
            |row| row.get(0),
        )
        .unwrap();
    let mut alias: Agent = serde_json::from_str(&raw).unwrap();
    alias.id = "synthetic-alias".into();
    alias.owner_id = "synthetic-alias-owner".into();
    alias.conversation_id = Some(f.caller.conversation_id.clone());
    f.store
        .writer_for_test()
        .execute(
            "INSERT INTO agents(id,owner_id,ordinal,record) VALUES(?1,?2,1,?3)",
            rusqlite::params![
                alias.id,
                alias.owner_id,
                serde_json::to_string(&alias).unwrap()
            ],
        )
        .unwrap();
    f.store.writer_for_test().execute_batch("INSERT INTO device_owner_projects(owner_id,project_path) VALUES('synthetic-alias-owner','/synthetic-alias-project'); INSERT INTO device_project_ids(path,project_id) VALUES('/synthetic-alias-project','synthetic-project')").unwrap();
    f.caller.agent_id = alias.id;
    f.caller.owner_id = alias.owner_id;
    let rec = f.secret(
        GrantScope::Project {
            project_id: f.caller.project_id.clone(),
        },
        None,
    );
    assert!(f.release(&rec.context.id).is_ok());
    // The alias still maps to the issued project; only its canonical owner moved.
    f.store.writer_for_test().execute("UPDATE device_project_ids SET project_id='synthetic-other-project' WHERE path='/synthetic-project'",[]).unwrap();
    assert!(matches!(
        f.release(&rec.context.id),
        Err(SecretError::Unauthorized)
    ));
}

#[test]
fn audit_cleanup_outpaces_admission_with_bounded_transactions_and_total_work() {
    let f = Fixture::new();
    let rec = f.secret(f.agent_grant(), None);
    f.release(&rec.context.id).unwrap();
    f.store.writer_for_test().execute("WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i<9000) INSERT INTO secret_audit(secret_id,creator,project_id,owner_id,agent_id,conversation_id,generation,session_incarnation,request_id,occurred_at,outcome) SELECT ?1,'synthetic-user','synthetic-project','synthetic-project','synthetic-agent','synthetic-agent','synthetic-generation','synthetic-session','synthetic-request',0,'released' FROM n",[&rec.context.id]).unwrap();
    let removed = f
        .store
        .writer()
        .secrets_audit_maintain(90 * 86400 + 1)
        .unwrap();
    assert_eq!(removed, 8000);
    let remaining: i64 = f
        .store
        .writer_for_test()
        .query_row(
            "SELECT COUNT(*) FROM secret_audit WHERE occurred_at=0",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(remaining, 1000);
    // Summary counts are persistent and never reconstructed from pruned detail.
    assert_eq!(
        f.store
            .writer()
            .secret_audit_summary(&rec.context.id)
            .unwrap()
            .unwrap()
            .release_count,
        1
    );
    assert_eq!(
        f.store
            .writer()
            .secrets_audit_maintain(90 * 86400 + 1)
            .unwrap(),
        1000
    );
}

#[test]
fn caller_matrix_allows_project_siblings_and_coordinator_but_hides_agent_grants() {
    let f = Fixture::new();
    let project = f.secret(
        GrantScope::Project {
            project_id: f.caller.project_id.clone(),
        },
        None,
    );
    let agent = f.secret(f.agent_grant(), None);
    for caller in [
        f.add_member(
            "synthetic-sibling",
            "synthetic-project",
            &f.caller.project_id,
            false,
        ),
        f.add_member(
            "project-synthetic-coordinator",
            "synthetic-project",
            &f.caller.project_id,
            true,
        ),
    ] {
        assert!(f
            .service
            .release(
                &caller,
                &project.context.id,
                "synthetic-project-read",
                || true
            )
            .is_ok());
        assert!(matches!(
            f.service
                .release(&caller, &agent.context.id, "synthetic-agent-read", || true),
            Err(SecretError::Unavailable)
        ));
    }
    let other = f.add_member(
        "synthetic-other-agent",
        "synthetic-other-owner",
        "synthetic-other-project",
        false,
    );
    for record in [project, agent] {
        assert!(matches!(
            f.service.release(
                &other,
                &record.context.id,
                "synthetic-cross-project-read",
                || true
            ),
            Err(SecretError::Unavailable)
        ));
    }
    let mut restarted = f.caller.clone();
    restarted.session_incarnation = "synthetic-fresh-incarnation".into();
    let agent = f.secret(f.agent_grant(), None);
    assert!(f
        .service
        .release(
            &restarted,
            &agent.context.id,
            "synthetic-restart-read",
            || true
        )
        .is_ok());
}

#[test]
fn both_scope_lifecycles_hold_through_reset_removal_and_membership_revalidation() {
    let f = Fixture::new();
    let sibling = f.add_member(
        "synthetic-lifecycle-sibling",
        "synthetic-lifecycle-sibling-owner",
        &f.caller.project_id,
        false,
    );
    let agent_before_reset = f.secret(f.agent_grant(), None);
    let project = f.secret(
        GrantScope::Project {
            project_id: f.caller.project_id.clone(),
        },
        None,
    );
    f.store
        .reset_conversation(
            &f.caller.conversation_id,
            &[],
            &std::collections::HashMap::new(),
            &std::collections::HashSet::new(),
        )
        .unwrap();
    assert!(matches!(
        f.release(&agent_before_reset.context.id),
        Err(SecretError::Revoked)
    ));
    assert_eq!(
        f.service
            .release(&sibling, &project.context.id, "after-reset", || true)
            .unwrap()
            .as_bytes(),
        b"synthetic-value-only"
    );

    let agent_before_removal = f.secret(f.agent_grant(), None);
    f.store.delete_run(&f.caller.owner_id).unwrap();
    assert!(f
        .store
        .writer()
        .secret_record(&agent_before_removal.context.id)
        .unwrap()
        .unwrap()
        .revoked_at
        .is_some());
    assert!(matches!(
        f.release(&agent_before_removal.context.id),
        Err(SecretError::Unauthorized)
    ));
    assert!(matches!(
        f.service
            .release(&f.caller, &project.context.id, "removed-member", || true),
        Err(SecretError::Unauthorized)
    ));
    assert!(f
        .service
        .release(&sibling, &project.context.id, "current-member", || true)
        .is_ok());
}

#[test]
fn schema_24_upgrade_adds_audit_without_changing_existing_metadata_or_value() {
    let f = Fixture::new();
    let rec = f.secret(f.agent_grant(), None);
    let Fixture {
        _dir,
        store,
        service,
        clock,
        caller,
    } = f;
    drop(service);
    drop(store);
    let connection = rusqlite::Connection::open(_dir.path().join("tasks/build.db")).unwrap();
    connection.execute_batch("DROP TABLE secret_audit; DROP TABLE secret_audit_summaries; DROP TABLE secret_unauthenticated_summary; UPDATE meta SET value='24' WHERE key='schema_version'").unwrap();
    drop(connection);
    let store = Store::new_with_secret_config(_dir.path().join("tasks"), &serde_json::json!({"environments":[{"id":"synthetic-environment"}],"projects":[{"id":caller.project_id,"environment_id":"synthetic-environment"}]})).unwrap();
    let service =
        SecretService::open_with_clock(_dir.path(), &caller.device_id, store.writer(), clock)
            .unwrap();
    assert_eq!(
        service
            .release(
                &caller,
                &rec.context.id,
                "synthetic-after-migration",
                || true
            )
            .unwrap()
            .as_bytes(),
        b"synthetic-value-only"
    );
    assert_eq!(
        store
            .writer()
            .secret_audit_page(&rec.context.id, 0, 100)
            .unwrap()
            .len(),
        1
    );
    let version: String = store
        .writer_for_test()
        .query_row(
            "SELECT value FROM meta WHERE key='schema_version'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(version, "27");
}

#[test]
fn audit_identifiers_use_fixed_target_for_unknown_and_wrong_scope_requests() {
    let f = Fixture::new();
    let sensitive_uuid = uuid::Uuid::new_v4().to_string();
    let unknown = format!("sec_{sensitive_uuid}");
    let wrong_scope = f.secret(
        GrantScope::Project {
            project_id: "synthetic-other-project".into(),
        },
        None,
    );
    for supplied_id in [&unknown, &wrong_scope.context.id] {
        assert!(matches!(
            f.service
                .release(&f.caller, supplied_id, &sensitive_uuid, || true),
            Err(SecretError::Unavailable)
        ));
    }
    let connection = f.store.writer_for_test();
    let mut query = connection
        // Lifecycle events now coexist in this table; this security assertion
        // concerns the two denied attempts and their fixed unavailable target.
        .prepare("SELECT secret_id,request_id FROM secret_audit WHERE outcome='secret_unavailable' ORDER BY sequence")
        .unwrap();
    let identifiers = query
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(identifiers.len(), 1);
    for (target, request_id) in identifiers {
        assert_eq!(
            target, "unavailable",
            "denied audit targets must be independent of supplied selectors"
        );
        assert!(
            !request_id.contains(&sensitive_uuid),
            "client bytes must not enter audit request identifiers"
        );
        assert!(uuid::Uuid::parse_str(&request_id).is_ok());
    }
}

#[test]
fn denial_audit_coalesces_and_caller_reader_includes_unavailable_targets() {
    let f = Fixture::new();
    let rec = f.secret(f.agent_grant(), None);
    f.service.revoke(&rec.context.id).unwrap();
    for _ in 0..20 {
        assert!(matches!(
            f.release(&rec.context.id),
            Err(SecretError::Revoked)
        ));
        assert!(matches!(
            f.release("sec_00000000-0000-4000-8000-000000000001"),
            Err(SecretError::Unavailable)
        ));
    }
    let writer = f.store.writer();
    let page = writer
        .secret_audit_by_caller_page(&f.caller.agent_id, 0, 100)
        .unwrap();
    assert_eq!(page.len(), 2);
    assert!(page.iter().all(|row| row.attempt_count == 20));
    assert!(page.iter().any(|row| row.secret_id == "unavailable"));
    assert_eq!(
        writer
            .secret_audit_summary(&rec.context.id)
            .unwrap()
            .unwrap()
            .denial_count,
        20
    );
    assert_eq!(
        writer
            .secret_audit_by_caller_page(&f.caller.agent_id, page[0].sequence, 1)
            .unwrap(),
        page[1..]
    );
    assert!(writer
        .secret_audit_by_caller_page(&f.caller.agent_id, 0, 101)
        .is_err());
    assert!(writer.secret_audit_by_caller_page("", 0, 1).is_err());
    let conn = f.store.writer_for_test();
    let plan: String = conn.query_row("EXPLAIN QUERY PLAN SELECT sequence FROM secret_audit WHERE agent_id=?1 AND sequence>?2 ORDER BY sequence LIMIT 100", rusqlite::params![f.caller.agent_id, 0], |row| row.get(3)).unwrap();
    assert!(plan.contains("secret_audit_by_caller"), "{plan}");
}

#[test]
fn denial_detail_cap_preserves_release_rows_and_totals() {
    let f = Fixture::new();
    let rec = f.secret(f.agent_grant(), None);
    f.release(&rec.context.id).unwrap();
    f.store.writer_for_test().execute("WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i<4096) INSERT INTO secret_audit(secret_id,project_id,owner_id,agent_id,conversation_id,generation,session_incarnation,request_id,occurred_at,last_occurred_at,outcome) SELECT 'unavailable','synthetic-project','synthetic-owner','synthetic-other','synthetic-other','synthetic-generation','synthetic-session','synthetic-request',0,0,'secret_unavailable' FROM n", []).unwrap();
    f.service.revoke(&rec.context.id).unwrap();
    assert!(matches!(
        f.release(&rec.context.id),
        Err(SecretError::Revoked)
    ));
    let conn = f.store.writer_for_test();
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM secret_audit WHERE outcome IN ('secret_unavailable','secret_expired','secret_revoked')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 4096);
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM secret_audit WHERE outcome='released'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn denial_groups_separate_windows_and_sessions_without_coalescing_releases() {
    let f = Fixture::new();
    let rec = f.secret(f.agent_grant(), None);
    f.release(&rec.context.id).unwrap();
    f.release(&rec.context.id).unwrap();
    f.service.revoke(&rec.context.id).unwrap();
    assert!(f.release(&rec.context.id).is_err());
    f.clock.0.store(1020, Ordering::SeqCst);
    assert!(f.release(&rec.context.id).is_err());
    let mut restarted = f.caller.clone();
    restarted.session_incarnation = "synthetic-restart".into();
    assert!(f
        .service
        .release(&restarted, &rec.context.id, "synthetic-request", || true)
        .is_err());
    let rows = f
        .store
        .writer()
        .secret_audit_page(&rec.context.id, 0, 100)
        .unwrap();
    assert_eq!(rows.len(), 5);
    assert!(rows.iter().all(|row| row.attempt_count == 1));
    assert_eq!(
        rows.iter().filter(|row| row.outcome == "released").count(),
        2
    );
}

#[test]
fn existing_audit_table_gains_group_fields_and_caller_index_on_reopen() {
    let f = Fixture::new();
    let rec = f.secret(f.agent_grant(), None);
    f.release(&rec.context.id).unwrap();
    let Fixture {
        _dir: dir,
        store,
        service,
        ..
    } = f;
    drop((service, store));
    let path = dir.path().join("tasks/build.db");
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch("DROP INDEX secret_audit_by_caller; ALTER TABLE secret_audit DROP COLUMN attempt_count; ALTER TABLE secret_audit DROP COLUMN last_occurred_at;").unwrap();
    // The old endpoint could retain arbitrarily many denial rows. Seed the
    // pre-column table so reopening exercises the one-time migration trim.
    conn.execute_batch("WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i<4100) INSERT INTO secret_audit(secret_id,project_id,owner_id,agent_id,conversation_id,generation,session_incarnation,request_id,occurred_at,outcome) SELECT 'unavailable','synthetic-project','synthetic-owner','synthetic-legacy','synthetic-legacy','synthetic-generation','synthetic-session','synthetic-request',i,'secret_unavailable' FROM n").unwrap();
    drop(conn);
    let reopened = Store::new(dir.path().join("tasks")).unwrap();
    let rows = reopened
        .writer()
        .secret_audit_by_caller_page("synthetic-agent", 0, 100)
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].attempt_count, 1);
    assert_eq!(rows[0].occurred_at, rows[0].last_occurred_at);
    assert_eq!(rows[0].outcome, "released");
    assert_eq!(rows[0].secret_id, rec.context.id);
    let conn = reopened.writer_for_test();
    let retained: (i64, i64, i64) = conn
        .query_row(
            "SELECT COUNT(*),MIN(occurred_at),MAX(occurred_at) FROM secret_audit WHERE outcome='secret_unavailable'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(retained, (4096, 5, 4100), "only oldest denials are trimmed");
    let unbackfilled: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM secret_audit WHERE attempt_count!=1 OR last_occurred_at!=occurred_at",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        unbackfilled, 0,
        "all legacy groups receive count and timestamp"
    );
    let release_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM secret_audit WHERE outcome='released'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(release_count, 1);
    drop(conn);
    assert_eq!(
        reopened
            .writer()
            .secret_audit_summary(&rec.context.id)
            .unwrap()
            .unwrap()
            .release_count,
        1
    );
}

#[test]
fn release_invalidations_require_a_committed_revision_change() {
    let f = Fixture::new();
    let record = f.secret(f.agent_grant(), Some(10));
    let changes = Arc::new(std::sync::Mutex::new(Vec::new()));
    let observed = changes.clone();
    f.service.set_change_observer(Arc::new(move |record| {
        observed.lock().unwrap().push(record.revision);
    }));

    f.release(&record.context.id).unwrap();
    f.release(&record.context.id).unwrap();
    assert!(
        changes.lock().unwrap().is_empty(),
        "releases do not change metadata revision"
    );
    assert!(matches!(
        f.service
            .release(&f.caller, &record.context.id, "synthetic-denial", || false),
        Err(SecretError::Unauthorized)
    ));
    assert!(
        changes.lock().unwrap().is_empty(),
        "denials do not change metadata revision"
    );

    f.clock.0.store(1010, Ordering::SeqCst);
    f.clock.1.store(10, Ordering::SeqCst);
    f.store.writer_for_test().execute_batch("CREATE TRIGGER synthetic_fail_expiry_audit BEFORE INSERT ON secret_audit BEGIN SELECT RAISE(ABORT,'synthetic audit failure'); END").unwrap();
    assert!(f.release(&record.context.id).is_err());
    assert!(
        changes.lock().unwrap().is_empty(),
        "rolled back expiry must not invalidate"
    );
    f.store
        .writer_for_test()
        .execute_batch("DROP TRIGGER synthetic_fail_expiry_audit")
        .unwrap();
    assert!(matches!(
        f.release(&record.context.id),
        Err(SecretError::Expired)
    ));
    let revision = f
        .store
        .writer()
        .secret_record(&record.context.id)
        .unwrap()
        .unwrap()
        .revision;
    assert_eq!(*changes.lock().unwrap(), vec![revision]);
    assert!(matches!(
        f.release(&record.context.id),
        Err(SecretError::Expired)
    ));
    assert_eq!(
        *changes.lock().unwrap(),
        vec![revision],
        "repeated expiry does not invalidate again"
    );
}

#[test]
fn lifecycle_metadata_survives_denial_group_cap_and_retention_without_entering_denial_readers() {
    let f = Fixture::new();
    let expired = f.secret(f.agent_grant(), Some(60));
    let revoked = f.secret(f.agent_grant(), None);
    f.store
        .writer()
        .secrets_expire(&expired.context.id, 1000)
        .unwrap();
    f.service.revoke(&revoked.context.id).unwrap();
    let metadata_count = || {
        f.store.writer_for_test().query_row("SELECT COUNT(*) FROM secret_audit WHERE outcome IN ('created','attached','expired','revoked')",[],|row|row.get::<_,i64>(0)).unwrap()
    };
    assert_eq!(metadata_count(), 6);
    f.store.writer_for_test().execute("WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i<4090) INSERT INTO secret_audit(secret_id,project_id,owner_id,agent_id,conversation_id,generation,session_incarnation,request_id,occurred_at,last_occurred_at,outcome) SELECT 'unavailable','synthetic-project','synthetic-project','synthetic-legacy','synthetic-legacy','synthetic-generation','synthetic-session','synthetic-request',0,0,'secret_unavailable' FROM n",[]).unwrap();
    for minute in 0..7 {
        f.clock.0.store(1000 + 60 * minute, Ordering::SeqCst);
        assert!(f.release(&expired.context.id).is_err());
        if minute == 0 {
            assert!(f.release(&expired.context.id).is_err());
        }
    }
    assert_eq!(
        metadata_count(),
        6,
        "denial group cap must preserve every lifecycle fact"
    );
    let denials: i64 = f.store.writer_for_test().query_row("SELECT COUNT(*) FROM secret_audit WHERE outcome IN ('secret_unavailable','secret_expired','secret_revoked')",[],|row|row.get(0)).unwrap();
    assert_eq!(denials, 4096);
    let caller = f
        .store
        .writer()
        .secret_audit_by_caller_page(&f.caller.agent_id, 0, 100)
        .unwrap();
    assert_eq!(caller.len(), 7, "caller reader excludes lifecycle facts");
    assert_eq!(caller[0].attempt_count, 2);
    let facts = f.store.writer_for_test().query_row("SELECT COUNT(*) FROM secret_audit WHERE outcome IN ('created','attached','expired','revoked') AND attempt_count=1 AND last_occurred_at=occurred_at",[],|row|row.get::<_,i64>(0)).unwrap();
    assert_eq!(
        facts, 6,
        "lifecycle appends populate D audit fields without coalescing"
    );
    f.store
        .writer()
        .secrets_audit_maintain(90 * 86400 + 2000)
        .unwrap();
    assert_eq!(
        metadata_count(),
        6,
        "retention prunes release/denial detail only"
    );
    assert!(f
        .store
        .writer()
        .secret_audit_by_caller_page(&f.caller.agent_id, 0, 100)
        .unwrap()
        .is_empty());
    let page = f
        .store
        .writer()
        .secret_metadata_audit(
            "synthetic-device",
            "synthetic-user",
            &crate::api::v1::secrets::AuditParams {
                scope: crate::api::v1::secrets::SecretScope {
                    environment_id: None,
                    project_id: f.caller.project_id.clone(),
                    stage: Some(expired.context.stage.clone()),
                },
                secret_id: expired.context.id.clone(),
                after: 0,
                limit: 100,
            },
        )
        .unwrap();
    assert_eq!(page.events.len(), 3);
    assert!(page
        .events
        .iter()
        .all(|event| event.attempt_count == 1 && event.last_occurred_at == event.occurred_at));
}

#[path = "environment_tests.rs"]
mod environments;

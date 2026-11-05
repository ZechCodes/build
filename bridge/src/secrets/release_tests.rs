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
    assert_eq!(page[0].request_id, "synthetic-request-1");
    assert_eq!(page[0].session_incarnation, f.caller.session_incarnation);
    let next = writer
        .secret_audit_page(&rec.context.id, page[0].sequence, 1)
        .unwrap();
    assert_eq!(next[0].request_id, "synthetic-request-2");
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
    let store = Store::new(_dir.path().join("tasks")).unwrap();
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
    assert_eq!(version, "25");
}

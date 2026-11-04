use crate::secrets::{GrantScope, SecretContext, SecretError, SecretRecord, StageContext};
use crate::store::Store;
use std::collections::{HashMap, HashSet};

const NOW: i64 = 1_800_000_000;

fn record(creator: &str, grant: GrantScope) -> SecretRecord {
    SecretRecord {
        context: SecretContext {
            id: format!("sec_{}", uuid::Uuid::new_v4()),
            device_id: "synthetic-device".into(), creator: creator.into(), name: "TEST_ONLY_TOKEN".into(),
            ttl_seconds: Some(60), grant,
            stage: StageContext { owner_id: "synthetic-owner".into(), conversation_id: "synthetic-conversation".into(), generation: "synthetic-generation".into(), draft_id: None },
        },
        created_at: NOW, attached_at: None, expires_at: None, revoked_at: None, revision: 1,
        message_operation_id: None, upload_request_id: uuid::Uuid::new_v4().to_string(),
        upload_digest: "ab".repeat(32), blob_present: true,
    }
}

fn agent_grant() -> GrantScope {
    GrantScope::Agent { agent_id: "synthetic-agent".into(), conversation_id: "synthetic-conversation".into(), generation: "synthetic-generation".into() }
}
fn project_grant() -> GrantScope { GrantScope::Project { project_id: "synthetic-project".into() } }

#[test]
fn metadata_survives_restart_and_migrates_existing_database_without_history_changes() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    store.writer_for_test().execute("INSERT INTO meta VALUES ('synthetic-marker','kept')", []).unwrap();
    drop(store);
    let db = rusqlite::Connection::open(dir.path().join("build.db")).unwrap();
    db.execute_batch("DROP TABLE secret_records; DROP TABLE secret_clock; UPDATE meta SET value='23' WHERE key='schema_version';").unwrap();
    drop(db);
    let store = Store::new(dir.path()).unwrap();
    assert!(!store.writer().secrets_have_records().unwrap());
    let rec = record("synthetic-user", agent_grant());
    store.writer().secrets_stage(&rec).unwrap();
    drop(store);
    let store = Store::new(dir.path()).unwrap();
    assert_eq!(store.writer().secret_record(&rec.context.id).unwrap(), Some(rec));
    let marker: String = store.writer_for_test().query_row("SELECT value FROM meta WHERE key='synthetic-marker'", [], |r|r.get(0)).unwrap();
    assert_eq!(marker,"kept");
    let version: String = store.writer_for_test().query_row("SELECT value FROM meta WHERE key='schema_version'", [], |r|r.get(0)).unwrap();
    assert_eq!(version,"24");
}

#[test]
fn quota_is_per_creator_and_device_and_retained_blob_quota_is_atomic() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let writer = store.writer();
    for _ in 0..100 { writer.secrets_stage(&record("synthetic-user",agent_grant())).unwrap(); }
    assert!(matches!(writer.secrets_stage(&record("synthetic-user",agent_grant())),Err(SecretError::Limit)));
    writer.secrets_stage(&record("synthetic-other-user",agent_grant())).unwrap();
    for index in 101..1000 { writer.secrets_stage(&record(&format!("synthetic-user-{index}"),agent_grant())).unwrap(); }
    assert!(matches!(writer.secrets_stage(&record("synthetic-user-overflow",project_grant())),Err(SecretError::Limit)));
    let first = writer.secret_records().unwrap().remove(0);
    writer.secrets_revoke(&first.context.id,NOW+1).unwrap();
    assert!(matches!(writer.secrets_stage(&record("synthetic-user-overflow",project_grant())),Err(SecretError::Limit)));
    writer.secrets_mark_deleted(&first.context.id).unwrap();
    writer.secrets_stage(&record("synthetic-user-overflow",project_grant())).unwrap();
}

#[test]
fn attach_is_all_or_nothing_sets_deadlines_and_replays_original_even_after_revoke() {
    let dir=tempfile::tempdir().unwrap(); let store=Store::new(dir.path()).unwrap(); let writer=store.writer();
    let first=record("synthetic-user",agent_grant()); let mut second=record("synthetic-user",project_grant()); second.context.ttl_seconds=None;
    for rec in [&first,&second] {writer.secrets_stage(rec).unwrap();}
    assert!(writer.secrets_attach(&[first.context.id.clone(),"sec_invalid".into()],"synthetic-operation",NOW+5).is_err());
    assert_eq!(writer.secret_record(&first.context.id).unwrap().unwrap().attached_at,None);
    let ids=vec![first.context.id.clone(),second.context.id.clone()];
    let attached=writer.secrets_attach(&ids,"synthetic-operation",NOW+5).unwrap();
    assert_eq!(attached[0].expires_at,Some(NOW+65)); assert_eq!(attached[1].expires_at,None);
    assert_eq!(attached[0].attached_at,Some(NOW+5));
    assert!(writer.secrets_attach(&ids,"other-operation",NOW+6).is_err());
    writer.secrets_revoke(&first.context.id,NOW+10).unwrap();
    let replay=writer.secrets_attach(&ids,"synthetic-operation",NOW+1000).unwrap();
    assert_eq!(replay[0].attached_at,Some(NOW+5)); assert_eq!(replay[0].expires_at,Some(NOW+65));
}

#[test]
fn stage_boundary_and_ttl_overflow_refuse_without_partial_claim() {
    let dir=tempfile::tempdir().unwrap(); let store=Store::new(dir.path()).unwrap(); let writer=store.writer();
    let first=record("synthetic-user",agent_grant()); writer.secrets_stage(&first).unwrap();
    assert!(writer.secrets_attach(&[first.context.id.clone()],"operation",NOW+86400).is_err());
    let mut second=record("synthetic-user",agent_grant()); second.context.ttl_seconds=Some(i64::MAX); writer.secrets_stage(&second).unwrap();
    assert!(writer.secrets_attach(&[second.context.id.clone()],"operation",NOW+1).is_err());
    assert_eq!(writer.secret_record(&second.context.id).unwrap().unwrap().attached_at,None);
}

#[test]
fn clock_highwater_survives_restart_and_never_moves_backwards() {
    let dir=tempfile::tempdir().unwrap(); let store=Store::new(dir.path()).unwrap();
    assert!(store.writer().secret_clock_observe(NOW).unwrap());
    assert!(!store.writer().secret_clock_observe(NOW-1).unwrap()); drop(store);
    let store=Store::new(dir.path()).unwrap(); assert!(!store.writer().secret_clock_observe(NOW-1).unwrap());
    assert!(store.writer().secret_clock_observe(NOW).unwrap());
}

#[test]
fn reset_and_project_delete_have_independent_scope_lifetimes() {
    let dir=tempfile::tempdir().unwrap(); let store=Store::new(dir.path()).unwrap(); let writer=store.writer();
    let agent=record("synthetic-user",agent_grant()); let project=record("synthetic-user",project_grant());
    writer.secrets_stage(&agent).unwrap(); writer.secrets_stage(&project).unwrap();
    store.reset_conversation("synthetic-conversation",&[],&HashMap::new(),&HashSet::new()).unwrap();
    assert!(writer.secret_record(&agent.context.id).unwrap().unwrap().revoked_at.is_some());
    assert_eq!(writer.secret_record(&project.context.id).unwrap().unwrap().revoked_at,None);
    store.clear_retained_project_messages("synthetic-project").unwrap();
    assert!(writer.secret_record(&project.context.id).unwrap().unwrap().revoked_at.is_some());
}

#[test]
fn metadata_rejects_tainted_stage_and_request_reuse() {
    let dir=tempfile::tempdir().unwrap(); let store=Store::new(dir.path()).unwrap(); let writer=store.writer();
    let rec=record("synthetic-user",agent_grant()); writer.secrets_stage(&rec).unwrap();
    let mut other=rec.clone(); other.context.id=format!("sec_{}",uuid::Uuid::new_v4());
    assert!(writer.secrets_stage(&other).is_err());
    let mut bad=record("synthetic-user",agent_grant()); bad.expires_at=Some(NOW+1);
    assert!(writer.secrets_stage(&bad).is_err());
    assert_eq!(writer.secret_records().unwrap().len(),1);
}

#[test]
fn removing_roster_member_or_owner_retires_agent_grants_and_preserves_project_grants() {
    for remove_owner in [false,true] {
        let dir=tempfile::tempdir().unwrap(); let store=Store::new(dir.path()).unwrap(); let writer=store.writer();
        let agent=crate::agent::Agent::new("synthetic-agent","synthetic-owner",crate::models::ModelChoice::default(),1,"2026-11-04T00:00:00Z");
        store.in_transaction(|tx| Store::write_agents(tx,"synthetic-owner",std::slice::from_ref(&agent))).unwrap();
        let a=record("synthetic-user",agent_grant()); let p=record("synthetic-user",project_grant());
        writer.secrets_stage(&a).unwrap(); writer.secrets_stage(&p).unwrap();
        if remove_owner {store.delete_run("synthetic-owner").unwrap();}
        else {store.in_transaction(|tx| Store::write_agents(tx,"synthetic-owner",&[])).unwrap();}
        assert!(writer.secret_record(&a.context.id).unwrap().unwrap().revoked_at.is_some());
        assert_eq!(writer.secret_record(&p.context.id).unwrap().unwrap().revoked_at,None);
    }
}

#[test]
fn attachment_hook_shares_operation_transaction_and_rolls_back_with_it() {
    let dir=tempfile::tempdir().unwrap(); let store=Store::new(dir.path()).unwrap(); let writer=store.writer();
    let rec=record("synthetic-user",agent_grant()); writer.secrets_stage(&rec).unwrap();
    {
        let mut connection=writer.connection(); let tx=connection.transaction().unwrap();
        tx.execute("INSERT INTO meta (key,value) VALUES ('synthetic-operation-marker','not-committed')",[]).unwrap();
        super::attach_in(&tx,std::slice::from_ref(&rec.context.id),"synthetic-operation",NOW+1).unwrap();
    }
    assert_eq!(writer.secret_record(&rec.context.id).unwrap().unwrap().attached_at,None);
    let marker:bool=writer.connection().query_row("SELECT EXISTS(SELECT 1 FROM meta WHERE key='synthetic-operation-marker')",[],|r|r.get(0)).unwrap();
    assert!(!marker);
}

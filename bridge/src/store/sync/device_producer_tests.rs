use crate::agent::AgentRoster;
use crate::attention::Attention;
use crate::capture::Capture;
use crate::models::ModelChoice;
use crate::run::RunState;
use crate::session_summary::UserSession;
use crate::store::{DeliveryIntent, PersistedRun, Store};
use crate::tracker::{Actor, Task};
use std::collections::{HashMap, HashSet};

const NOW: i64 = 1_800_000_000_000;
const AT: &str = "2026-01-01T00:00:00Z";

fn run_record(id: &str) -> PersistedRun {
    PersistedRun {
        id: id.into(),
        plan_id: None,
        goal: "synthetic run".into(),
        project_path: "/device-producers".into(),
        base_branch: "main".into(),
        state: RunState::Building,
        branch: format!("build/{id}"),
        worktree_name: id.into(),
        worktree_path: format!("/worktrees/{id}"),
        base_sha: None,
        stages: vec![],
        current_stage_id: None,
        revising_stage_id: None,
        auto_advance: false,
        adopted: false,
        publication_attempt: None,
        provider: Default::default(),
        model: None,
        effort: None,
        agents: AgentRoster::with_first(id, ModelChoice::default(), AT)
            .agents()
            .to_vec(),
        legacy_thread: Default::default(),
        pinned_agent_id: None,
        last_summary: None,
        last_error: None,
        created_at: AT.into(),
        updated_at: AT.into(),
        state_changed_at: None,
    }
}

async fn device_rows(store: &Store) -> Vec<serde_json::Value> {
    let lease = store
        .writer()
        .admit_device_lease("producer-test", "epoch", NOW)
        .unwrap();
    let mut read = store.read_pool().acquire_sync().await;
    let answer = read
        .read(|tx| tx.sync_device_snapshot(&lease.id, None, 500, "epoch", NOW + 1))
        .unwrap()
        .unwrap();
    answer["records"].as_array().unwrap().clone()
}

#[tokio::test]
async fn task_capture_and_user_session_writers_publish_device_rows_and_deletions() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::new(directory.path()).unwrap();
    let mut task = Task::drafted(
        "/device-producers",
        "watched task",
        Actor::User,
        "2026-01-01T00:00:00Z",
    );
    task.watched = true;
    let mut task = store.create_tracker_task(task, &[]).unwrap();
    let capture = Capture::new(
        "capture-one",
        "private body must not enter the device row",
        "2026-01-01T00:00:00Z",
    );
    store.save_capture(&capture).unwrap();
    let session = UserSession::default().updated(NOW);
    store.save_user_session(&session).unwrap();

    let rows = device_rows(&store).await;
    let mut kinds = rows
        .iter()
        .map(|row| row["kind"].as_str().unwrap())
        .collect::<Vec<_>>();
    kinds.sort_unstable();
    assert_eq!(kinds, ["inbox_capture", "inbox_task", "user_session"]);
    let capture = rows
        .iter()
        .find(|row| row["kind"] == "inbox_capture")
        .unwrap();
    assert!(capture["row"].get("text").is_none());
    assert_eq!(capture["row"]["id"], "capture-one");
    let task_row = rows.iter().find(|row| row["kind"] == "inbox_task").unwrap();
    assert_eq!(task_row["row"]["id"], task.id);
    assert_eq!(task_row["row"]["selection"]["pending_present"], false);

    store
        .accept_delivery_intent(
            &task,
            &[],
            DeliveryIntent {
                id: String::new(),
                task_id: task.id.clone(),
                project_path: task.project_path.clone(),
                generation: 0,
                target: serde_json::json!({"kind":"agent","agent_id":"Actual\u{0}suffix"}),
                actor: Actor::User,
                sender: None,
                note: None,
                operation_id: String::new(),
                state: "queued".into(),
                reservation: serde_json::json!(null),
                admitted: None,
                error: None,
            },
        )
        .unwrap();
    let rows = device_rows(&store).await;
    let task_row = rows.iter().find(|row| row["kind"] == "inbox_task").unwrap();
    assert_eq!(task_row["row"]["selection"]["pending_present"], true);
    assert_eq!(
        task_row["row"]["deferred_assignment"]["target"]["agent_id"],
        "Actual\u{0}suffix"
    );

    task.watched = false;
    store.save_tracker_task_activity(&task, &[], &[]).unwrap();
    store.delete_capture("capture-one").unwrap();
    let rows = device_rows(&store).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["kind"], "user_session");

    let device_deletes: i64 = store
        .writer_for_test()
        .query_row(
            "SELECT COUNT(*) FROM sync_journal j JOIN sync_routes r ON r.seq=j.seq
             WHERE r.stream_kind='device' AND j.operation='delete'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(device_deletes, 2);
}

#[tokio::test]
async fn agent_and_conversation_writers_publish_current_rows_and_roster_deletions() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::new(directory.path()).unwrap();
    store.writer_for_test().execute("INSERT INTO device_project_ids VALUES('/device-producers','proj-registry')",[]).unwrap();
    let mut run = run_record("run-one");
    let agent_id = run.agents[0].id.clone();
    store.save_run(&run).unwrap();

    let rows = device_rows(&store).await;
    assert_eq!(
        rows.iter()
            .map(|row| row["kind"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["agent", "identity", "inbox_conversation"]
    );
    let agent = rows.iter().find(|row| row["kind"] == "agent").unwrap();
    assert_eq!(agent["key"], agent_id);
    assert_eq!(agent["row"]["owner_id"], "run-one");
    assert_eq!(agent["row"]["project_id"], "proj-registry");
    for ephemeral in ["active_model", "state", "working", "available"] {
        assert!(
            agent["row"].get(ephemeral).is_none(),
            "process-lifetime field escaped: {ephemeral}"
        );
    }
    let identity = rows.iter().find(|row| row["kind"] == "identity").unwrap();
    assert!(identity["row"].get("available").is_none());
    let conversation = rows
        .iter()
        .find(|row| row["kind"] == "inbox_conversation")
        .unwrap();
    assert_eq!(conversation["row"]["last_sequence"], "0");

    run.agents[0].thread.post_user("hello", None, AT);
    store.save_run(&run).unwrap();
    let rows = device_rows(&store).await;
    let conversation = rows
        .iter()
        .find(|row| row["kind"] == "inbox_conversation")
        .unwrap();
    assert_eq!(conversation["row"]["last_sequence"], "1");

    let mut attention = Attention::default();
    attention.read_through(&agent_id, 1);
    store
        .save_attention(
            &HashMap::from([("run-one".into(), attention)]),
            &HashSet::from(["run-one".into()]),
        )
        .unwrap();
    let rows = device_rows(&store).await;
    let conversation = rows
        .iter()
        .find(|row| row["kind"] == "inbox_conversation")
        .unwrap();
    assert_eq!(conversation["row"]["read_through_sequence"], "1");

    run.agents.clear();
    store.save_run(&run).unwrap();
    assert!(device_rows(&store).await.is_empty());
    let deletes: i64 = store
        .writer_for_test()
        .query_row(
            "SELECT COUNT(*) FROM sync_journal j JOIN sync_routes r ON r.seq=j.seq
             WHERE r.stream_kind='device' AND j.operation='delete'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(deletes, 3);
}

#[test]
fn committed_conversation_items_have_linear_or_better_journal_amplification() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::new(directory.path()).unwrap();
    let mut run = run_record("run-amplification");
    store.save_run(&run).unwrap();
    let baseline: i64 = store
        .writer_for_test()
        .query_row("SELECT COUNT(*) FROM sync_journal", [], |row| row.get(0))
        .unwrap();

    store.save_run(&run).unwrap();
    assert_eq!(
        store
            .writer_for_test()
            .query_row("SELECT COUNT(*) FROM sync_journal", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        baseline,
        "an unchanged roster save must add no device entries"
    );

    const COMMITTED: i64 = 32;
    for index in 0..COMMITTED {
        run.agents[0]
            .thread
            .post_user(format!("committed {index}"), None, AT);
        store.save_run(&run).unwrap();
    }
    let added: i64 = store
        .writer_for_test()
        .query_row("SELECT COUNT(*) FROM sync_journal", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap()
        - baseline;
    assert!(
        added <= 2 * COMMITTED,
        "{COMMITTED} committed items added {added} journal entries"
    );

    let mut batched = run_record("run-batched-amplification");
    store.save_run(&batched).unwrap();
    let before_batch: i64 = store
        .writer_for_test()
        .query_row("SELECT COUNT(*) FROM sync_journal", [], |row| row.get(0))
        .unwrap();
    for index in 0..COMMITTED {
        batched.agents[0]
            .thread
            .post_user(format!("batched {index}"), None, AT);
    }
    store.save_run(&batched).unwrap();
    let batch_added: i64 = store
        .writer_for_test()
        .query_row("SELECT COUNT(*) FROM sync_journal", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap()
        - before_batch;
    assert!(
        batch_added <= 2,
        "one transaction with {COMMITTED} items added {batch_added} entries"
    );
}

#[test]
fn device_sql_source_and_projection_share_rollback_and_identical_writes_are_quiet() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::new(directory.path()).unwrap();
    store
        .writer_for_test()
        .execute_batch(
            "CREATE TRIGGER reject_device_publication BEFORE INSERT ON sync_journal
             BEGIN SELECT RAISE(ABORT,'synthetic device publication failure'); END;",
        )
        .unwrap();
    let capture = Capture::new("capture-rollback", "private", AT);
    assert!(store.save_capture(&capture).is_err());
    let conn = store.writer_for_test();
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM captures WHERE id='capture-rollback'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap(),
        0
    );
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM sync_projection", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap(),
        0
    );
    conn.execute_batch("DROP TRIGGER reject_device_publication")
        .unwrap();
    drop(conn);

    let session = UserSession::default().updated(NOW);
    store.save_user_session(&session).unwrap();
    store.save_user_session(&session).unwrap();
    assert_eq!(
        store
            .writer_for_test()
            .query_row("SELECT COUNT(*) FROM sync_journal", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
        1,
        "an identical committed source row must not advance the device stream"
    );
}

#[test]
fn implemented_device_sql_writers_have_common_transactional_hooks() {
    let entities = include_str!("../entities.rs");
    for owner in [
        "touch_device_agent(tx, gone)",
        "touch_device_agent(tx, &agent.id)",
        "touch_device_owner(tx, run_id)",
        "Entity::new(\"inbox_capture\"",
    ] {
        assert!(entities.contains(owner), "missing device owner: {owner}");
    }
    let session = include_str!("../user_session.rs");
    assert!(session.contains("Entity::new(\"user_session\", \"current\")"));
    let task = include_str!("task_publication.rs");
    assert!(task.contains("Entity::new(\"inbox_task\", &id)"));
}


fn projection(store: &Store, kind: &str, id: &str) -> serde_json::Value {
    let raw: String = store.writer_for_test().query_row(
        "SELECT record FROM sync_projection WHERE kind=?1 AND key=?2 AND deleted=0",
        rusqlite::params![kind,id], |row| row.get(0)).unwrap();
    serde_json::from_str(&raw).unwrap()
}

#[test]
fn device_summary_ignores_status_progress_and_still_working() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::new(directory.path()).unwrap();
    let mut run = run_record("summary-stability");
    run.agents[0].thread.post_user("durable question",None, AT);
    store.save_run(&run).unwrap();
    let id = run.agents[0].id.clone();
    let before = projection(&store,"inbox_conversation",&id);
    run.agents[0].thread.post_agent_progress("progress",None,"2026-01-01T01:00:00Z");
    run.agents[0].thread.push_event(crate::thread::ThreadEventKind::ToolUse, Some("work".into()),None,None,"2026-01-01T02:00:00Z");
    store.save_run(&run).unwrap();
    assert_eq!(projection(&store,"inbox_conversation",&id),before,"ephemeral work changed summary");
    run.agents[0].thread.post_agent("durable answer",None,"2026-01-01T03:00:00Z");
    store.save_run(&run).unwrap();
    let after = projection(&store,"inbox_conversation",&id);
    assert_ne!(after,before,"negative control: handback must change summary");
    assert_eq!(after["last_message_summary"],"durable answer");
    assert_eq!(after["last_activity_ms"],1_767_236_400_000_i64);
    assert_eq!(after["session_started_ms"],1_767_225_600_000_i64);
    assert_eq!(after["unread_count"],1);
}

#[test]
fn device_canonical_thread_changes_reach_every_alias_and_reset_transactionally() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::new(directory.path()).unwrap();
    let mut run = run_record("canonical");
    let canonical = run.agents[0].id.clone();
    store.save_run(&run).unwrap();
    let mut alias = run_record("alias");
    alias.agents[0].conversation_id=Some(canonical.clone());
    let alias_id=alias.agents[0].id.clone();
    store.save_run(&alias).unwrap();
    let before=projection(&store,"inbox_conversation",&alias_id);
    run.agents[0].thread.post_agent("answer",None,AT);
    store.save_run(&run).unwrap();
    assert_eq!(projection(&store,"inbox_conversation",&alias_id)["unread_count"],1);
    assert_ne!(projection(&store,"inbox_conversation",&alias_id),before);
    let mut unrelated=run_record("unrelated");
    store.save_run(&unrelated).unwrap();
    let quiet=projection(&store,"inbox_conversation",&alias_id);
    unrelated.agents[0].thread.post_user("unrelated",None,AT);
    store.save_run(&unrelated).unwrap();
    assert_eq!(projection(&store,"inbox_conversation",&alias_id),quiet,"negative control: unrelated transcript leaked");
    run.agents[0].thread=crate::thread::Thread::new(&canonical);
    store.reset_conversation(&canonical,&run.agents,&HashMap::new(),&HashSet::from([run.id.clone()])).unwrap();
    assert_eq!(projection(&store,"inbox_conversation",&alias_id)["unread_count"],0);
}

#[test]
fn device_unread_counters_are_incremental_and_last_summary_uses_indexed_seek() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::new(directory.path()).unwrap();
    let mut run=run_record("counter");
    run.agents[0].thread.post_agent("first",None,AT);
    run.agents[0].thread.post_agent("second",None,AT);
    store.save_run(&run).unwrap();
    let id=run.agents[0].id.clone();
    let nodes:i64=store.writer_for_test().query_row("SELECT COUNT(*) FROM device_attention_counts",[],|row|row.get(0)).unwrap();
    assert!(nodes>0);
    let mut attention=Attention::default();
    attention.read_through(&id,1);
    store.save_attention(&HashMap::from([(run.id.clone(),attention)]),&HashSet::from([run.id.clone()])).unwrap();
    assert_eq!(projection(&store,"inbox_conversation",&id)["unread_count"],1);
    let plan:String=store.writer_for_test().query_row("EXPLAIN QUERY PLAN SELECT sequence,item FROM thread_items INDEXED BY device_thread_summary WHERE agent_id=?1 AND (attention=1 OR (message=1 AND COALESCE(json_extract(item,'$.data.still_working'),0)=0)) ORDER BY sequence DESC LIMIT 1",[&id],|row|row.get(3)).unwrap();
    assert!(plan.contains("SEARCH"),"{plan}");
    run.agents[0].thread.items.clear();
    store.save_run(&run).unwrap();
    assert_eq!(projection(&store,"inbox_conversation",&id)["unread_count"],0,"negative control: deleting attention must subtract");
}

#[test]
fn device_upgrade_backfills_existing_sql_sources_before_leases() {
    let directory=tempfile::tempdir().unwrap();
    let store=Store::new(directory.path()).unwrap();
    let run=run_record("old-sql");
    store.save_run(&run).unwrap();
    {
        let conn=store.writer_for_test();
        conn.execute("DELETE FROM sync_projection",[]).unwrap();
        conn.execute("DELETE FROM sync_projection_keys",[]).unwrap();
        conn.execute("DELETE FROM meta WHERE key LIKE 'device_backfill%'",[]).unwrap();
    }
    drop(store);
    let reopened=Store::new(directory.path()).unwrap();
    assert_eq!(projection(&reopened,"agent",&run.agents[0].id)["id"],run.agents[0].id);
    assert!(reopened.writer().admit_device_lease("upgrade","epoch",NOW).is_ok());
    let head:i64=reopened.writer_for_test().query_row("SELECT head FROM sync_state",[],|row|row.get(0)).unwrap();
    drop(reopened);
    let unchanged=Store::new(directory.path()).unwrap();
    assert_eq!(unchanged.writer_for_test().query_row("SELECT head FROM sync_state",[],|row|row.get::<_,i64>(0)).unwrap(),head,"negative control: completed backfill must be quiet");
}

#[test]
fn device_delivery_ack_and_terminal_attention_publish_durable_summary_changes() {
    let directory=tempfile::tempdir().unwrap();
    let store=Store::new(directory.path()).unwrap();
    let mut run=run_record("delivery");
    run.agents[0].thread.post_user("queued question",None,AT);
    let id=run.agents[0].id.clone();
    store.save_run(&run).unwrap();
    assert_eq!(projection(&store,"inbox_conversation",&id)["pending"],true);
    store.set_legacy_delivery_status(&id,1,1,crate::thread::MessageDeliveryStatus::Sent).unwrap();
    assert_eq!(projection(&store,"inbox_conversation",&id)["pending"],false);
    store.acknowledge_native_legacy_messages(&id,1,1,"2026-01-01T00:01:00Z").unwrap();
    assert_eq!(projection(&store,"inbox_conversation",&id)["pending"],false,"negative control: seen messages do not become pending");
    store.save_terminal_interaction(&run.id,"2026-01-01T01:00:00Z",Some(AT),true,None).unwrap();
    assert_eq!(projection(&store,"inbox_conversation",&id)["last_interaction_at"],"2026-01-01T01:00:00Z");
}

#[test]
fn device_followup_watch_floor_and_muting_do_not_resurrect_old_attention() {
    let directory=tempfile::tempdir().unwrap();let store=Store::new(directory.path()).unwrap();
    let mut run=run_record("watch-floor");let id=run.agents[0].id.clone();
    run.agents[0].thread.post_agent("old answer",None,AT);
    run.agents[0].watch_started_after=1;
    store.save_run(&run).unwrap();
    assert_eq!(projection(&store,"inbox_conversation",&id)["unread_count"],0);
    run.agents[0].thread.post_agent("new answer",None,AT);store.save_run(&run).unwrap();
    assert_eq!(projection(&store,"inbox_conversation",&id)["unread_count"],1);
    let attention=Attention{muted:true,..Default::default()};
    store.save_attention(&HashMap::from([(run.id.clone(),attention)]),&HashSet::from([run.id.clone()])).unwrap();
    assert_eq!(projection(&store,"inbox_conversation",&id)["unread_count"],0);
}

#[test]
fn device_followup_counter_reset_has_constant_writer_work_and_bounded_cleanup() {
    let directory=tempfile::tempdir().unwrap();let store=Store::new(directory.path()).unwrap();
    let run=run_record("reset-cost");store.save_run(&run).unwrap();let id=&run.agents[0].id;
    {let conn=store.writer_for_test();conn.execute("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<1000) INSERT INTO device_attention_counts(conversation_id,node,total) SELECT ?1,x,1 FROM n",[id]).unwrap();}
    let before=store.total_changes();
    store.reset_conversation(id,&run.agents,&HashMap::new(),&HashSet::from([run.id.clone()])).unwrap();
    assert!(store.total_changes()-before<50,"counter reset must fence a generation, not erase history");
    let progress=store.writer().sync_gc_batch(NOW).unwrap();assert!(progress.changed_rows<=GC_ROWS as u64);
}

#[test]
fn device_followup_build_notices_do_not_advance_session_timing() {
    let directory=tempfile::tempdir().unwrap();let store=Store::new(directory.path()).unwrap();
    let mut run=run_record("session-notice");let id=run.agents[0].id.clone();
    run.agents[0].thread.post_user("human",None,AT);store.save_run(&run).unwrap();
    let before=projection(&store,"inbox_conversation",&id);
    run.agents[0].thread.post_user_from_build("notice","2026-01-03T00:00:00Z");
    store.save_run(&run).unwrap();
    assert_eq!(projection(&store,"inbox_conversation",&id)["last_activity_ms"],before["last_activity_ms"]);
}

#[test]
fn device_followup_archived_worktree_and_workspace_lifecycle_publish_durable_rows() {
    let directory=tempfile::tempdir().unwrap();let store=Store::new(directory.path()).unwrap();
    let archive=crate::store::PersistedArchivedWorktree{status:Default::default(),project_path:"/device-producers".into(),worktree_id:"archived".into(),worktree_name:"retained".into(),worktree_path:"/synthetic/archive".into(),branch:None,head_sha:"synthetic".into(),upstream:None,unpushed:None,dirty_files:0,uncommitted_files:0,uncommitted_insertions:0,uncommitted_deletions:0,action:crate::store::WorktreeFinishAction::Cleanup,archived_at:Some(AT.into())};
    store.save_archived_worktree(&archive).unwrap();
    assert_eq!(projection(&store,"archived_worktree","archived")["id"],"archived");
    let lifecycle=crate::reclaim::LifecycleRecord::default();
    store.save_workspace_lifecycle(&HashMap::from([("workspace".to_string(),lifecycle)])).unwrap();
    assert!(projection(&store,"workspace_lifecycle","workspace").is_object());
}

#[test]
fn device_followup_pending_only_receipts_refresh_all_canonical_aliases() {
    let directory=tempfile::tempdir().unwrap();let store=Store::new(directory.path()).unwrap();
    let run=run_record("receipt-owner");let canonical=run.agents[0].id.clone();store.save_run(&run).unwrap();
    let mut alias=run_record("receipt-alias");alias.agents[0].conversation_id=Some(canonical.clone());store.save_run(&alias).unwrap();
    let alias_id=alias.agents[0].id.clone();
    let receipt=crate::operation::OperationReceipt{operation_id:"device-pending-only".into(),method:"thread.post".into(),entity_id:alias.id.clone(),agent_id:alias_id.clone(),conversation_id:canonical.clone(),choice_revision:alias.agents[0].choice_revision,posted_sequence:0,message_start_sequence:0,status:crate::operation::OperationStatus::Queued,execution_error:None,request_hash:"synthetic".into(),delivery:None,requested_by:None};
    store.writer().sync_transaction(|tx,_|crate::store::operations::insert_operation(tx,&receipt)).unwrap();
    assert_eq!(projection(&store,"inbox_conversation",&canonical)["pending"],true);
    assert_eq!(projection(&store,"inbox_conversation",&alias_id)["pending"],true);
    store.transition_operation(&receipt.operation_id,crate::operation::OperationStatus::Queued,crate::operation::OperationStatus::Delivered,None).unwrap();
    assert_eq!(projection(&store,"inbox_conversation",&alias_id)["pending"],false);
}

thread_local! {static DEVICE_SQL_COST:std::cell::Cell<(u64,u64)>=const{std::cell::Cell::new((0,0))};}
fn measure_device_sql(event:rusqlite::trace::TraceEvent<'_>) {
    if let rusqlite::trace::TraceEvent::Profile(statement,_) = event {
        let sql=statement.sql();
        let legacy_scan=sql.contains("SELECT json_extract(item, '$.data.created_at')") || sql.contains("SELECT json_extract(t.item, '$.data.created_at')");
        let device=sql.contains("device_") || sql.contains("sync_") || (sql.contains("thread_items") && sql.starts_with("SELECT") && !legacy_scan);
        if device {DEVICE_SQL_COST.with(|cost|{let (count,steps)=cost.get();cost.set((count+1,steps+statement.get_status(rusqlite::StatementStatus::VmStep) as u64));});}
    }
}
fn retained_deletion_cost(items:usize,remove_agent:bool)->(u64,u64) {
    let directory=tempfile::tempdir().unwrap();let store=Store::new(directory.path()).unwrap();
    let mut run=run_record("retention-cost");
    for index in 0..items {run.agents[0].thread.post_agent(format!("message {index}"),None,AT);}
    store.save_run(&run).unwrap();
    {let conn=store.writer_for_test();conn.flush_prepared_statement_cache();conn.trace_v2(rusqlite::trace::TraceEventCodes::SQLITE_TRACE_PROFILE,Some(measure_device_sql));}
    DEVICE_SQL_COST.with(|cost|cost.set((0,0)));
    if remove_agent {run.agents.clear();store.save_run(&run).unwrap();} else {store.delete_run_retaining_inbox_messages(&run.id,"proj-registry").unwrap();}
    let measured=DEVICE_SQL_COST.with(|cost|cost.get());
    store.writer_for_test().trace_v2(rusqlite::trace::TraceEventCodes::empty(),None);
    measured
}
#[test]
fn device_followup_retained_owner_deletion_has_no_history_dependent_producer_cost() {
    for remove_agent in [false,true] {
        let short=retained_deletion_cost(8,remove_agent);
        let long=retained_deletion_cost(1024,remove_agent);
        assert!(long.0<=short.0+10,"producer statements grew with transcript: {short:?} -> {long:?}");
        assert!(long.1<=short.1*4+100,"producer VM work grew with transcript: {short:?} -> {long:?}");
    }
}

#[test]
fn device_followup_backfill_resumes_after_a_committed_chunk_and_refuses_partial_admission() {
    let directory=tempfile::tempdir().unwrap();let store=Store::new(directory.path()).unwrap();
    let mut run=run_record("backfill-restart");let id=run.agents[0].id.clone();
    for index in 0..270 {run.agents[0].thread.post_agent(format!("answer {index}"),None,AT);}
    store.save_run(&run).unwrap();
    {let conn=store.writer_for_test();conn.execute("DELETE FROM meta WHERE key='device_backfill_position'",[]).unwrap();
    conn.execute("DELETE FROM sync_projection",[]).unwrap();conn.execute("DELETE FROM sync_projection_keys",[]).unwrap();
    conn.execute_batch("CREATE TRIGGER interrupt_device_backfill BEFORE UPDATE OF value ON meta WHEN NEW.key='device_backfill_position' AND json_extract(NEW.value,'$.phase')=2 AND json_extract(NEW.value,'$.seq')>=256 BEGIN SELECT RAISE(ABORT,'synthetic interrupted backfill'); END;").unwrap();}
    drop(store);
    assert!(Store::new(directory.path()).is_err(),"a partial backfill cannot hand out a StoreWriter or admit a lease");
    let conn=rusqlite::Connection::open(directory.path().join("build.db")).unwrap();
    let position:String=conn.query_row("SELECT value FROM meta WHERE key='device_backfill_position'",[],|row|row.get(0)).unwrap();
    assert_eq!(serde_json::from_str::<serde_json::Value>(&position).unwrap()["seq"],128);
    conn.execute_batch("DROP TRIGGER interrupt_device_backfill").unwrap();drop(conn);
    let store=Store::new(directory.path()).unwrap();
    assert_eq!(projection(&store,"inbox_conversation",&id)["unread_count"],270);
    store.writer().admit_device_lease("opening","epoch",NOW).unwrap();
}

#[test]
fn device_followup_retained_summaries_use_saved_facts_after_owner_history_is_removed() {
    let directory=tempfile::tempdir().unwrap();let store=Store::new(directory.path()).unwrap();
    store.writer_for_test().execute("INSERT INTO device_project_ids VALUES('/device-producers','proj-registry')",[]).unwrap();
    let mut run=run_record("retained-summary");
    run.agents[0].thread.post_agent("last answer",None,AT);store.save_run(&run).unwrap();
    let last=projection(&store,"inbox_conversation",&run.agents[0].id)["last_activity_ms"].clone();
    run.agents.clear();store.save_run(&run).unwrap();
    assert_eq!(projection(&store,"workspace_session",&run.id)["retained_last_activity_ms"],last);
    store.delete_run_retaining_inbox_messages(&run.id,"proj-registry").unwrap();
    assert_eq!(projection(&store,"project_session","proj-registry")["retained_last_activity_ms"],last);
    drop(store);let store=Store::new(directory.path()).unwrap();
    assert_eq!(projection(&store,"project_session","proj-registry")["retained_last_activity_ms"],last);
    store.clear_retained_project_messages("proj-registry").unwrap();
    let deleted:bool=store.writer_for_test().query_row("SELECT deleted FROM sync_projection WHERE kind='project_session' AND key='proj-registry'",[],|row|row.get(0)).unwrap();
    assert!(deleted,"clearing retained authority must tombstone its summary");
}

#[test]
fn device_followup_summary_keyset_contains_only_committed_visible_items() {
    let directory=tempfile::tempdir().unwrap();let store=Store::new(directory.path()).unwrap();
    let mut run=run_record("sparse-summary");let id=&run.agents[0].id.clone();
    run.agents[0].thread.post_user("question",None,AT);
    for _ in 0..300 {run.agents[0].thread.post_agent_progress("working",None,AT);}
    run.agents[0].thread.post_agent("answer",None,AT);store.save_run(&run).unwrap();
    let rows:i64=store.writer_for_test().query_row("SELECT COUNT(*) FROM device_summary_items WHERE conversation_id=?1",[id],|row|row.get(0)).unwrap();
    assert_eq!(rows,2,"transient items must never enter the device summary keyset");
    let plan:String=store.writer_for_test().query_row("EXPLAIN QUERY PLAN SELECT sequence,summary FROM device_summary_items WHERE conversation_id=?1 AND generation=0 ORDER BY sequence DESC LIMIT 1",[id],|row|row.get(3)).unwrap();
    assert!(plan.contains("SEARCH") && !plan.contains("TEMP"),"{plan}");
}

#[test]
fn device_followup_long_durable_titles_do_not_fail_canonical_writes() {
    let directory=tempfile::tempdir().unwrap();let store=Store::new(directory.path()).unwrap();
    let mut run=run_record("long-title");run.agents[0].topic=Some("title".repeat(40_000));
    store.save_run(&run).expect("device summary limits must not reject a valid canonical topic");
    let row=projection(&store,"agent",&run.agents[0].id);
    assert!(row["topic"].as_str().unwrap().chars().count()<=256);
    let raw:String=store.writer_for_test().query_row("SELECT record FROM agents WHERE id=?1",[&run.agents[0].id],|row|row.get(0)).unwrap();
    assert_eq!(serde_json::from_str::<serde_json::Value>(&raw).unwrap()["topic"],run.agents[0].topic.as_deref().unwrap());
}

#[test]
fn device_followup_hot_projection_does_not_decode_historical_agent_metadata() {
    let directory=tempfile::tempdir().unwrap();let store=Store::new(directory.path()).unwrap();
    let run=run_record("history-free-facts");store.save_run(&run).unwrap();
    store.writer_for_test().execute("UPDATE agents SET record=json_set(record,'$.thread.sessions',json('[\"opaque historical session\"]')) WHERE id=?1",[&run.agents[0].id]).unwrap();
    let attention=Attention{muted:true,..Default::default()};
    store.save_attention(&HashMap::from([(run.id.clone(),attention)]),&HashSet::from([run.id.clone()])).expect("a device update must consume saved directory facts, not decode historical sessions");
    assert_eq!(projection(&store,"inbox_conversation",&run.agents[0].id)["muted"],true);
}

#[test]
fn device_followup_empty_conversation_reset_publishes_the_durable_generation() {
    let directory=tempfile::tempdir().unwrap();let store=Store::new(directory.path()).unwrap();
    let mut run=run_record("empty-generation");let id=run.agents[0].id.clone();store.save_run(&run).unwrap();
    let before=projection(&store,"inbox_conversation",&id);
    run.agents[0].thread.generation_revision+=1;
    store.reset_conversation(&id,&run.agents,&HashMap::new(),&HashSet::from([run.id.clone()])).unwrap();
    let after=projection(&store,"inbox_conversation",&id);
    assert_eq!(after["thread_generation_revision"],"1");
    assert_ne!(before,after,"reset of an empty conversation still invalidates its generation");
}

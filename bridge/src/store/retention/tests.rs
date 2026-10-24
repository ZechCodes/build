use super::*;
use tempfile::TempDir;

fn fixture(messages: usize) -> (TempDir, Store) {
    let dir = TempDir::new().unwrap();
    let store = Store::new(dir.path()).unwrap();
    store.in_transaction(|tx| {
        tx.execute("INSERT INTO agents VALUES ('a', 'owner', 1, '{\"thread\":{\"id\":\"t\"}}')", [])?;
        for n in 1..=messages {
            let item = serde_json::json!({"type":"message","data":{"created_at":"2026-10-23T12:00:00Z", "role":"assistant"}});
            tx.execute("INSERT INTO thread_items(agent_id,sequence,updated_sequence,message,item) VALUES ('a',?1,?1,1,?2)", rusqlite::params![n as i64, item.to_string()])?;
        }
        Ok(())
    }).unwrap();
    (dir, store)
}

#[test]
fn newest_hundred_and_dry_run() {
    let (_dir, store) = fixture(150);
    let report = store
        .retention_report(RetentionPolicy::default(), "2026-10-24T12:00:00Z")
        .unwrap();
    assert_eq!(report[0].history_start_sequence, 51);
    assert_eq!(report[0].items_to_prune, 50);
    assert_eq!(store.thread_item_count("a").unwrap(), 150);
    let batch = store
        .prune_conversation_batch("a", RetentionPolicy::default(), "2026-10-24T12:00:00Z")
        .unwrap();
    assert_eq!(batch.items_deleted, 50);
    assert_eq!(store.thread_item_count("a").unwrap(), 100);
}

#[test]
fn explicit_zero_minimum_preserves_counter_after_emptying() {
    let (dir, store) = fixture(5);
    let result = store
        .prune_conversation_batch(
            "a",
            RetentionPolicy {
                min_messages: 0,
                ..RetentionPolicy::default()
            },
            "2026-11-24T12:00:00Z",
        )
        .unwrap();
    assert_eq!(result.items_deleted, 5);
    drop(store);
    let store = Store::new(dir.path()).unwrap();
    let boundary = store.thread_retention_boundary("a").unwrap().unwrap();
    assert_eq!(boundary.history_start_sequence, 6);
    assert_eq!(boundary.high_watermark, 5);
    assert_eq!(
        store
            .connection()
            .query_row(super::super::THREAD_LAST_SEQUENCE_SQL, ["a"], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        5
    );
}
#[test]
fn both_limits_and_activity_use_the_oldest_surviving_message() {
    let (_dir, store) = fixture(150);
    store.in_transaction(|tx| {
        tx.execute("UPDATE thread_items SET sequence = -sequence, updated_sequence = -updated_sequence", [])?;
        tx.execute("UPDATE thread_items SET sequence = sequence * -10, updated_sequence = updated_sequence * -10", [])?;
        tx.execute("UPDATE thread_items SET item = json_set(item, '$.data.created_at', '2026-10-01T12:00:00Z') WHERE sequence < 1200", [])?;
        for sequence in [1199,1201,1501] {
            tx.execute("INSERT INTO thread_items(agent_id,sequence,updated_sequence,activity,item) VALUES ('a',?1,?1,1,'{}')", [sequence])?;
        }
        Ok(())
    }).unwrap();
    let batch = store
        .prune_conversation_batch("a", RetentionPolicy::default(), "2026-10-24T12:00:00Z")
        .unwrap();
    assert_eq!(batch.boundary.unwrap().history_start_sequence, 1010);
    assert_eq!(store.thread_item_count("a").unwrap(), 53);
    assert_eq!(
        store
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM thread_items WHERE agent_id = 'a' AND activity = 1",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        3
    );
}

#[test]
fn exact_age_boundary_is_inclusive() {
    let (_dir, store) = fixture(2);
    store.in_transaction(|tx| {
        tx.execute("UPDATE thread_items SET item = json_set(item, '$.data.created_at', '2026-10-17T12:00:00Z') WHERE sequence = 2", [])?;
        tx.execute("UPDATE thread_items SET item = json_set(item, '$.data.created_at', '2026-10-17T11:59:59Z') WHERE sequence = 1", [])?;
        Ok(())
    }).unwrap();
    let batch = store
        .prune_conversation_batch(
            "a",
            RetentionPolicy {
                min_messages: 0,
                ..RetentionPolicy::default()
            },
            "2026-10-24T12:00:00Z",
        )
        .unwrap();
    assert_eq!(batch.items_deleted, 1);
    assert_eq!(batch.boundary.unwrap().history_start_sequence, 2);
}

#[test]
fn operation_and_legacy_pending_deliveries_protect_history() {
    let (_dir, store) = fixture(150);
    for status in ["queued", "claimed", "uncertain"] {
        store.in_transaction(|tx| {
            tx.execute("INSERT OR REPLACE INTO operations(operation_id,method,entity_id,agent_id,conversation_id,choice_revision,request_hash,posted_sequence,message_start_sequence,status,created_at,updated_at) VALUES ('op','thread.post','owner','a','a',0,'hash',1,1,?1,'now','now')", [status])?;
            Ok(())
        }).unwrap();
        let batch = store
            .prune_conversation_batch("a", RetentionPolicy::default(), "2026-10-24T12:00:00Z")
            .unwrap();
        assert_eq!(batch.items_deleted, 0);
    }
    store.in_transaction(|tx| {
        tx.execute("DELETE FROM operations", [])?;
        tx.execute("UPDATE thread_items SET item = json_set(item, '$.data.role', 'user') WHERE sequence = 1", [])?;
        Ok(())
    }).unwrap();
    assert_eq!(
        store
            .prune_conversation_batch("a", RetentionPolicy::default(), "2026-10-24T12:00:00Z")
            .unwrap()
            .items_deleted,
        0
    );
}

#[test]
fn bounded_batches_resume_after_reopen() {
    let (dir, store) = fixture(2500);
    let first = store
        .prune_conversation_batch("a", RetentionPolicy::default(), "2026-10-24T12:00:00Z")
        .unwrap();
    assert_eq!(first.items_deleted, 1000);
    assert!(first.has_more);
    assert_eq!(first.boundary.unwrap().history_start_sequence, 1001);
    drop(store);
    let store = Store::new(dir.path()).unwrap();
    assert_eq!(
        store
            .prune_conversation_batch("a", RetentionPolicy::default(), "2026-10-24T12:00:00Z")
            .unwrap()
            .items_deleted,
        1000
    );
    let last = store
        .prune_conversation_batch("a", RetentionPolicy::default(), "2026-10-24T12:00:00Z")
        .unwrap();
    assert_eq!(last.items_deleted, 400);
    assert!(!last.has_more);
    assert_eq!(store.thread_item_count("a").unwrap(), 100);
}

#[test]
fn stale_resident_saves_cannot_resurrect_pruned_history() {
    let dir = TempDir::new().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut agents = crate::agent::AgentRoster::with_first(
        "owner",
        crate::models::ModelChoice::default(),
        "2026-10-23T12:00:00Z",
    )
    .agents()
    .to_vec();
    for _ in 0..150 {
        agents[0]
            .thread
            .post_agent("hello", None, "2026-10-23T12:00:00Z");
    }
    let id = agents[0].id.clone();
    store
        .in_transaction(|tx| Store::write_agents(tx, "owner", &agents))
        .unwrap();
    store
        .prune_conversation_batch(&id, RetentionPolicy::default(), "2026-10-24T12:00:00Z")
        .unwrap();
    store
        .in_transaction(|tx| Store::write_agents(tx, "owner", &agents))
        .unwrap();
    assert_eq!(store.thread_item_count(&id).unwrap(), 100);
    store
        .prune_conversation_batch(
            &id,
            RetentionPolicy {
                min_messages: 0,
                ..RetentionPolicy::default()
            },
            "2026-11-24T12:00:00Z",
        )
        .unwrap();
    store
        .in_transaction(|tx| Store::write_agents(tx, "owner", &agents))
        .unwrap();
    assert_eq!(store.thread_item_count(&id).unwrap(), 0);
    store.in_transaction(|tx| {
        tx.execute("UPDATE agents SET record = json_set(record, '$.thread.next_sequence', 0) WHERE id = ?1", [&id])?;
        Ok(())
    }).unwrap();
    let loaded = Store::read_agents(
        &store.connection(),
        "owner",
        &crate::models::ModelChoice::default(),
        None,
    )
    .unwrap();
    let mut agent = loaded[0].clone();
    let previous = agent.thread.last_sequence();
    agent.thread.post_agent("new", None, "2026-11-24T12:00:00Z");
    assert!(agent.thread.last_sequence() > previous);
    assert!(previous >= 150);
}

#[tokio::test]
async fn pooled_read_p99_stays_below_100ms_during_pruning() {
    let (_dir, store) = fixture(12_000);
    let mut reader = store.read_pool().acquire().await;
    // Prime the production read connection before measuring contention.
    reader
        .read(|read| read.thread_item_count("a"))
        .unwrap()
        .unwrap();
    let writer = store.clone();
    let start = std::sync::Arc::new(std::sync::Barrier::new(2));
    let writer_start = start.clone();
    let worker = std::thread::spawn(move || {
        writer_start.wait();
        loop {
            if !writer
                .prune_conversation_batch("a", RetentionPolicy::default(), "2026-10-24T12:00:00Z")
                .unwrap()
                .has_more
            {
                break;
            }
        }
    });
    start.wait();
    let mut samples = Vec::new();
    while samples.len() < 200 || !worker.is_finished() {
        let began = std::time::Instant::now();
        reader
            .read(|read| read.thread_item_count("a"))
            .unwrap()
            .unwrap();
        samples.push(began.elapsed());
    }
    worker.join().unwrap();
    samples.sort_unstable();
    let p99 = samples[(samples.len() - 1) * 99 / 100];
    eprintln!(
        "retention pooled read p99={p99:?}; samples={}",
        samples.len()
    );
    assert!(
        p99 < std::time::Duration::from_millis(100),
        "read p99 during pruning: {p99:?}"
    );
}
#[test]
fn settled_operation_retry_and_conflict_evidence_survives_pruning() {
    let (_dir, store) = fixture(150);
    store.in_transaction(|tx| {
        tx.execute("INSERT INTO operations(operation_id,method,entity_id,agent_id,conversation_id,choice_revision,request_hash,posted_sequence,message_start_sequence,status,created_at,updated_at) VALUES ('op','thread.post','owner','a','a',0,'hash',1,1,'delivered','now','now')", [])?;
        Ok(())
    }).unwrap();
    let original = super::super::operations::read_operation(&store.connection(), "op")
        .unwrap()
        .unwrap();
    let batch = store
        .prune_conversation_batch("a", RetentionPolicy::default(), "2026-10-24T12:00:00Z")
        .unwrap();
    assert_eq!(batch.operations_deleted, 0);
    assert_eq!(batch.items_deleted, 50);
    store
        .in_transaction(|tx| {
            let retry =
                super::super::operations::existing_operation_or_conflict(tx, &original)?.unwrap();
            assert_eq!(retry, original);
            let mut conflict = original.clone();
            conflict.request_hash = "different".into();
            assert!(matches!(
                super::super::operations::existing_operation_or_conflict(tx, &conflict),
                Err(StoreError::OperationConflict { .. })
            ));
            Ok(())
        })
        .unwrap();
}
#[test]
fn old_conversations_keep_fifty_and_short_histories_keep_every_message() {
    for (messages, kept) in [(150, 50), (12, 12), (0, 0)] {
        let (_dir, store) = fixture(messages);
        let batch = store
            .prune_conversation_batch("a", RetentionPolicy::default(), "2026-11-24T12:00:00Z")
            .unwrap();
        assert_eq!(store.thread_item_count("a").unwrap(), kept);
        assert_eq!(batch.items_deleted as u64, messages as u64 - kept);
    }
}

#[test]
fn recent_count_between_bounds_selects_that_many_newest_messages() {
    let (_dir, store) = fixture(150);
    store.in_transaction(|tx| {
        tx.execute("UPDATE thread_items SET item = json_set(item, '$.data.created_at', '2026-10-01T12:00:00Z') WHERE sequence <= 70", [])?;
        Ok(())
    }).unwrap();
    let batch = store
        .prune_conversation_batch("a", RetentionPolicy::default(), "2026-10-24T12:00:00Z")
        .unwrap();
    assert_eq!(store.thread_item_count("a").unwrap(), 80);
    assert_eq!(batch.boundary.unwrap().history_start_sequence, 71);
}

#[test]
fn upload_expiry_is_bounded_and_preserves_recent_and_pending_drafts() {
    let (_dir, store) = fixture(1);
    store.in_transaction(|tx| {
        for (id,path,date) in [("a","old","2026-10-01T00:00:00Z"),("a","recent","2026-10-24T00:00:00Z"),("pending","protected","2026-10-01T00:00:00Z")] {
            tx.execute("INSERT INTO conversation_attachment_uploads VALUES (?1,'t',?2,?3)", rusqlite::params![id,path,date])?;
        }
        tx.execute("INSERT INTO operations(operation_id,method,entity_id,agent_id,conversation_id,choice_revision,request_hash,posted_sequence,message_start_sequence,status,created_at,updated_at) VALUES ('op','thread.post','owner','pending','pending',0,'hash',1,1,'queued','now','now')", [])?;
        Ok(())
    }).unwrap();
    assert_eq!(
        store
            .cleanup_conversation_attachment_uploads(7, "2026-10-24T12:00:00Z", 1)
            .unwrap(),
        1
    );
    assert_eq!(
        store
            .cleanup_conversation_attachment_uploads(7, "2026-10-24T12:00:00Z", 1000)
            .unwrap(),
        0
    );
    assert_eq!(
        store
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM conversation_attachment_uploads",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        2
    );
}

#[test]
fn message_commit_consumes_its_reservation_but_later_save_preserves_new_draft() {
    let dir = TempDir::new().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut agents = crate::agent::AgentRoster::with_first(
        "owner",
        crate::models::ModelChoice::default(),
        "2026-10-23T12:00:00Z",
    )
    .agents()
    .to_vec();
    let id = agents[0].id.clone();
    let thread_id = agents[0].thread.id.clone();
    let path = ".build/attachments/shared.png";
    store
        .register_conversation_attachment(&id, &thread_id, path)
        .unwrap();
    agents[0]
        .thread
        .post_agent("with attachment", None, "2026-10-23T12:00:00Z");
    if let Some(crate::thread::ThreadItem::Message(message)) = agents[0].thread.items.last_mut() {
        message.attachments.push(crate::thread::MessageAttachment {
            name: "shared.png".into(),
            path: path.into(),
            mime: "image/png".into(),
            size: 4,
        });
    }
    store
        .in_transaction(|tx| Store::write_agents(tx, "owner", &agents))
        .unwrap();
    assert_eq!(
        store
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM conversation_attachment_uploads",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    store
        .register_conversation_attachment(&id, &thread_id, path)
        .unwrap();
    store
        .in_transaction(|tx| Store::write_agents(tx, "owner", &agents))
        .unwrap();
    assert_eq!(
        store
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM conversation_attachment_uploads",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
}
#[test]
fn recent_count_includes_timestamps_outside_newest_maximum() {
    let (_dir, store) = fixture(150);
    store.in_transaction(|tx| {
        tx.execute("UPDATE thread_items SET item = json_set(item, '$.data.created_at', '2026-10-01T00:00:00Z') WHERE sequence > 80", [])?;
        Ok(())
    }).unwrap();
    let batch = store
        .prune_conversation_batch("a", RetentionPolicy::default(), "2026-10-24T12:00:00Z")
        .unwrap();
    assert_eq!(store.thread_item_count("a").unwrap(), 80);
    assert_eq!(batch.boundary.unwrap().history_start_sequence, 71);
}

#[test]
fn legacy_upload_reservations_get_a_migration_grace_period() {
    let dir = TempDir::new().unwrap();
    let conn = rusqlite::Connection::open(dir.path().join("build.db")).unwrap();
    conn.execute_batch("CREATE TABLE conversation_attachment_uploads (conversation_id TEXT NOT NULL, thread_id TEXT NOT NULL, path TEXT NOT NULL, PRIMARY KEY (conversation_id,thread_id,path)); INSERT INTO conversation_attachment_uploads VALUES ('a','t','draft.png');").unwrap();
    drop(conn);
    let store = Store::new(dir.path()).unwrap();
    let baseline: String = store
        .connection()
        .query_row(
            "SELECT value FROM meta WHERE key = 'retention_uploads_registered_at'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let migrated: jiff::Timestamp = baseline.parse().unwrap();
    let before_expiry = migrated
        .checked_add(jiff::SignedDuration::from_secs(6 * 86_400))
        .unwrap()
        .to_string();
    let after_expiry = migrated
        .checked_add(jiff::SignedDuration::from_secs(8 * 86_400))
        .unwrap()
        .to_string();
    assert_eq!(
        store
            .cleanup_conversation_attachment_uploads(7, &before_expiry, 1000)
            .unwrap(),
        0
    );
    assert_eq!(
        store
            .cleanup_conversation_attachment_uploads(7, &after_expiry, 1000)
            .unwrap(),
        1
    );
}

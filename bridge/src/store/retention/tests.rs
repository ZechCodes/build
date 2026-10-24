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
fn shared_draft_reservation_survives_another_messages_commit_and_pruning() {
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
    // A and B upload identical bytes before A commits. Their content path is
    // identical, so A cannot consume the row that also protects B's draft.
    store
        .register_conversation_attachment(&id, &thread_id, path)
        .unwrap();
    store
        .in_transaction(|tx| {
            tx.execute(
                "UPDATE conversation_attachment_uploads SET uploaded_at = '2026-10-24T13:00:00Z'",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    let home = store.attachments_dir();
    std::fs::create_dir_all(&home).unwrap();
    let file = home.join("shared.png");
    std::fs::write(&file, b"same").unwrap();
    let old = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(100);
    std::fs::File::open(&file)
        .unwrap()
        .set_modified(old)
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
        1
    );
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
    for _ in 0..150 {
        agents[0]
            .thread
            .post_agent("later", None, "2026-10-24T14:00:00Z");
    }
    store
        .in_transaction(|tx| Store::write_agents(tx, "owner", &agents))
        .unwrap();
    store
        .prune_conversation_batch(&id, RetentionPolicy::default(), "2026-10-24T15:00:00Z")
        .unwrap();
    assert!(store
        .thread_items(&id)
        .unwrap()
        .iter()
        .all(|item| match item {
            crate::thread::ThreadItem::Message(message) => message.attachments.is_empty(),
            _ => true,
        }));
    assert_eq!(
        store
            .cleanup_conversation_attachment_uploads(7, "2026-10-25T12:00:00Z", 1000)
            .unwrap(),
        0
    );
    let sweep_cutoff = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(200);
    assert_eq!(
        store
            .sweep_unreferenced_attachments(std::slice::from_ref(&home), sweep_cutoff)
            .unwrap(),
        0
    );
    assert!(
        file.exists(),
        "B's uncommitted draft must survive A's pruning and GC"
    );
    assert_eq!(
        store
            .cleanup_conversation_attachment_uploads(7, "2026-11-02T12:00:00Z", 1000)
            .unwrap(),
        1
    );
    assert_eq!(
        store
            .sweep_unreferenced_attachments(&[home], sweep_cutoff)
            .unwrap(),
        1
    );
    assert!(!file.exists());
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

#[test]
fn routed_pending_delivery_protects_the_canonical_target_but_delivered_does_not() {
    let (_dir, store) = fixture(150);
    store.in_transaction(|tx| {
        tx.execute("INSERT INTO agents VALUES ('alias', 'owner', 2, '{\"conversation_id\":\"a\",\"thread\":{\"id\":\"alias-thread\"}}')", [])?;
        tx.execute("INSERT INTO operations(operation_id,method,entity_id,agent_id,conversation_id,choice_revision,request_hash,posted_sequence,message_start_sequence,status,delivery,created_at,updated_at) VALUES ('routed','thread.post','owner','origin','origin',0,'hash',1,1,'queued','{\"agent_id\":\"alias\"}','now','now')", [])?;
        tx.execute("INSERT INTO conversation_attachment_uploads VALUES ('a','t','draft.png','2026-10-01T00:00:00Z')", [])?;
        Ok(())
    }).unwrap();
    for status in ["queued", "claimed", "uncertain"] {
        store
            .in_transaction(|tx| {
                tx.execute("UPDATE operations SET status = ?1", [status])?;
                Ok(())
            })
            .unwrap();
        let report = store
            .retention_report_conversation("a", RetentionPolicy::default(), "2026-10-24T12:00:00Z")
            .unwrap();
        assert!(
            report.protected,
            "routed {status} operation must protect its canonical delivery target"
        );
        assert_eq!(
            store
                .prune_conversation_batch("a", RetentionPolicy::default(), "2026-10-24T12:00:00Z")
                .unwrap()
                .items_deleted,
            0
        );
        assert_eq!(
            store
                .cleanup_conversation_attachment_uploads(7, "2026-10-24T12:00:00Z", 1000)
                .unwrap(),
            0
        );
    }
    store
        .in_transaction(|tx| {
            tx.execute("UPDATE thread_items SET item = json_set(item, '$.data.role', 'user', '$.data.delivery_status', 'submitted', '$.data.operation_id', 'routed') WHERE agent_id = 'a' AND sequence = 1", [])?;
            tx.execute("UPDATE operations SET status = 'delivered'", [])?;
            Ok(())
        })
        .unwrap();
    assert!(
        !store
            .retention_report_conversation("a", RetentionPolicy::default(), "2026-10-24T12:00:00Z")
            .unwrap()
            .protected
    );
    assert_eq!(
        store
            .prune_conversation_batch("a", RetentionPolicy::default(), "2026-10-24T12:00:00Z")
            .unwrap()
            .items_deleted,
        50
    );
    assert_eq!(
        store
            .cleanup_conversation_attachment_uploads(7, "2026-10-24T12:00:00Z", 1000)
            .unwrap(),
        1
    );
}

#[test]
fn recent_pruned_timestamps_keep_the_rule_stable_across_batches_and_cycles() {
    for (messages, recent, kept) in [(150, 80, 80), (150, 100, 100), (2000, 100, 100)] {
        let (dir, store) = fixture(messages);
        store.in_transaction(|tx| {
            tx.execute("UPDATE thread_items SET item = json_set(item, '$.data.created_at', '2026-10-01T00:00:00Z') WHERE sequence > ?1", [recent])?;
            Ok(())
        }).unwrap();
        while store
            .prune_conversation_batch("a", RetentionPolicy::default(), "2026-10-24T12:00:00Z")
            .unwrap()
            .has_more
        {}
        assert_eq!(
            store.thread_item_count("a").unwrap(),
            kept,
            "first complete cycle must keep{kept} for {messages} messages"
        );
        drop(store);
        let store = Store::new(dir.path()).unwrap();
        let next = store
            .prune_conversation_batch("a", RetentionPolicy::default(), "2026-10-24T12:00:00Z")
            .unwrap();
        assert_eq!(
            next.items_deleted, 0,
            "a restart and another sweep must not shrink the retained count"
        );
        assert_eq!(store.thread_item_count("a").unwrap(), kept);
        let summary_count: i64 = store.connection().query_row("SELECT json_array_length(pruned_message_times) FROM conversation_retention WHERE agent_id = 'a'", [], |row| row.get(0)).unwrap();
        assert!(summary_count <= 100);
        drop(store);
        let copied = TempDir::new().unwrap();
        std::fs::copy(dir.path().join("build.db"), copied.path().join("build.db")).unwrap();
        let store = Store::new(copied.path()).unwrap();
        assert_eq!(
            store
                .prune_conversation_batch("a", RetentionPolicy::default(), "2026-10-24T12:00:00Z")
                .unwrap()
                .items_deleted,
            0
        );
        assert_eq!(
            store.thread_item_count("a").unwrap(),
            kept,
            "a database copy must preserve timestamp provenance"
        );
        let expired = store
            .prune_conversation_batch("a", RetentionPolicy::default(), "2026-11-24T12:00:00Z")
            .unwrap();
        assert_eq!(
            expired.items_deleted as u64,
            kept - 50,
            "expired timestamp summaries must stop contributing to the count"
        );
        assert_eq!(store.thread_item_count("a").unwrap(), 50);
    }
}

#[test]
fn existing_retention_boundaries_gain_timestamp_summary_without_resetting_cursors() {
    let dir = TempDir::new().unwrap();
    let conn = rusqlite::Connection::open(dir.path().join("build.db")).unwrap();
    conn.execute_batch("CREATE TABLE conversation_retention (agent_id TEXT PRIMARY KEY, thread_id TEXT NOT NULL, history_start_sequence INTEGER NOT NULL, high_watermark INTEGER NOT NULL); INSERT INTO conversation_retention VALUES ('a','t',51,150);").unwrap();
    drop(conn);
    let store = Store::new(dir.path()).unwrap();
    let boundary = store.thread_retention_boundary("a").unwrap().unwrap();
    assert_eq!(boundary.history_start_sequence, 51);
    assert_eq!(boundary.high_watermark, 150);
    let summary: String = store
        .connection()
        .query_row(
            "SELECT pruned_message_times FROM conversation_retention WHERE agent_id = 'a'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(summary, "[]");
}

#[test]
fn old_uncertain_span_allows_bounded_pruning_of_its_older_prefix() {
    let (_dir, store) = fixture(2500);
    store.in_transaction(|tx| {
        tx.execute("INSERT INTO operations(operation_id,method,entity_id,agent_id,conversation_id,choice_revision,request_hash,posted_sequence,message_start_sequence,status,created_at,updated_at) VALUES ('old','thread.post','owner','a','a',0,'hash',1510,1501,'uncertain','now','now')", [])?;
        tx.execute("INSERT INTO operations(operation_id,method,entity_id,agent_id,conversation_id,choice_revision,request_hash,posted_sequence,message_start_sequence,status,created_at,updated_at) VALUES ('later','thread.post','owner','a','a',0,'hash',2010,2001,'queued','now','now'), ('settled','thread.post','owner','a','a',0,'hash',10,1,'delivered','now','now')", [])?;
        Ok(())
    }).unwrap();
    let report = store
        .retention_report_conversation("a", RetentionPolicy::default(), "2026-11-24T12:00:00Z")
        .unwrap();
    assert!(
        report.protected,
        "the unresolved span remains an informational fence"
    );
    assert_eq!(report.history_start_sequence, 1501);
    assert_eq!(
        report.items_to_prune, 1500,
        "only the span and newer items require protection"
    );
    let first = store
        .prune_conversation_batch("a", RetentionPolicy::default(), "2026-11-24T12:00:00Z")
        .unwrap();
    assert_eq!(first.items_deleted, 1000);
    assert!(first.has_more);
    let last = store
        .prune_conversation_batch("a", RetentionPolicy::default(), "2026-11-24T12:00:00Z")
        .unwrap();
    assert_eq!(last.items_deleted, 500);
    assert!(!last.has_more);
    assert_eq!(store.thread_item_count("a").unwrap(), 1000);
    assert_eq!(last.boundary.unwrap().history_start_sequence, 1501);
    store
        .in_transaction(|tx| {
            tx.execute("UPDATE operations SET status = 'delivered'", [])?;
            Ok(())
        })
        .unwrap();
    let settled = store
        .prune_conversation_batch("a", RetentionPolicy::default(), "2026-11-24T12:00:00Z")
        .unwrap();
    assert_eq!(
        settled.items_deleted, 950,
        "settling all spans releases their retention fence"
    );
    assert_eq!(store.thread_item_count("a").unwrap(), 50);
}

#[test]
fn forwarded_target_uses_its_payload_span_and_origin_uses_its_receipt_span() {
    let (_dir, store) = fixture(200);
    store.in_transaction(|tx| {
        tx.execute("INSERT INTO agents VALUES ('origin','owner',2,'{\"thread\":{\"id\":\"source\"}}')", [])?;
        tx.execute("INSERT INTO agents VALUES ('alias','owner',3,'{\"conversation_id\":\"a\",\"thread\":{\"id\":\"alias\"}}')", [])?;
        tx.execute("INSERT INTO agents VALUES ('origin-alias','owner',4,'{\"conversation_id\":\"origin\",\"thread\":{\"id\":\"source-alias\"}}')", [])?;
        tx.execute("INSERT INTO thread_items(agent_id,sequence,updated_sequence,message,item) SELECT 'origin',sequence,updated_sequence,message,item FROM thread_items WHERE agent_id = 'a'", [])?;
        tx.execute("INSERT INTO operations(operation_id,method,entity_id,agent_id,conversation_id,choice_revision,request_hash,posted_sequence,message_start_sequence,status,delivery,created_at,updated_at) VALUES ('forward','thread.post','owner','origin-alias','origin-alias',0,'hash',10,5,'queued','{\"agent_id\":\"alias\",\"payload\":{\"start_sequence\":61,\"end_sequence\":70}}','now','now')", [])?;
        Ok(())
    }).unwrap();
    let target = store
        .retention_report_conversation("a", RetentionPolicy::default(), "2026-10-24T12:00:00Z")
        .unwrap();
    assert_eq!(
        target.history_start_sequence, 61,
        "target and origin have independent sequence spaces"
    );
    assert_eq!(target.items_to_prune, 60);
    let origin = store
        .retention_report_conversation("origin", RetentionPolicy::default(), "2026-10-24T12:00:00Z")
        .unwrap();
    assert_eq!(origin.history_start_sequence, 5);
    assert_eq!(origin.items_to_prune, 4);
    assert_eq!(
        store
            .prune_conversation_batch("a", RetentionPolicy::default(), "2026-10-24T12:00:00Z")
            .unwrap()
            .items_deleted,
        60
    );
    assert_eq!(
        store
            .prune_conversation_batch("origin", RetentionPolicy::default(), "2026-10-24T12:00:00Z")
            .unwrap()
            .items_deleted,
        4
    );
}

#[test]
fn unknown_pending_spans_stay_conservative_and_legacy_pending_uses_its_item_sequence() {
    let (_dir, store) = fixture(200);
    store.in_transaction(|tx| {
        tx.execute("INSERT INTO operations(operation_id,method,entity_id,agent_id,conversation_id,choice_revision,request_hash,posted_sequence,message_start_sequence,status,created_at,updated_at) VALUES ('unknown','thread.post','owner','a','a',0,'hash',70,0,'claimed','now','now')", [])?;
        Ok(())
    }).unwrap();
    assert_eq!(
        store
            .prune_conversation_batch("a", RetentionPolicy::default(), "2026-10-24T12:00:00Z")
            .unwrap()
            .items_deleted,
        0,
        "zero start cannot prove an older prefix is safe"
    );
    store.in_transaction(|tx| {
        tx.execute("DELETE FROM operations", [])?;
        tx.execute("UPDATE thread_items SET item = json_set(item, '$.data.role','user','$.data.delivery_status','queued') WHERE agent_id = 'a' AND sequence = 61", [])?;
        Ok(())
    }).unwrap();
    let legacy = store
        .prune_conversation_batch("a", RetentionPolicy::default(), "2026-10-24T12:00:00Z")
        .unwrap();
    assert_eq!(legacy.items_deleted, 60);
    assert_eq!(legacy.boundary.unwrap().history_start_sequence, 61);
}

#[test]
fn same_conversation_compacted_stubs_use_the_earliest_receipt_span() {
    let (_dir, store) = fixture(200);
    store.in_transaction(|tx| {
        tx.execute("INSERT INTO agents VALUES ('alias','owner',2,'{\"conversation_id\":\"a\",\"thread\":{\"id\":\"alias\"}}')", [])?;
        tx.execute("UPDATE thread_items SET message = 0, activity = 1, item = '{}' WHERE sequence IN (30,32)", [])?;
        for (operation,status,start) in [("one","queued",91),("two","claimed",61),("three","uncertain",31)] {
            tx.execute("INSERT INTO operations(operation_id,method,entity_id,agent_id,conversation_id,choice_revision,request_hash,posted_sequence,message_start_sequence,status,delivery,created_at,updated_at) VALUES (?1,'thread.post','owner','a','a',0,'hash',100,?3,?2,'{\"agent_id\":\"alias\",\"payload\":null}','now','now')", rusqlite::params![operation,status,start])?;
        }
        Ok(())
    }).unwrap();
    let report = store
        .retention_report_conversation("a", RetentionPolicy::default(), "2026-10-24T12:00:00Z")
        .unwrap();
    assert!(report.protected);
    assert_eq!(report.history_start_sequence, 31);
    assert_eq!(report.items_to_prune, 30);
    let batch = store
        .prune_conversation_batch("a", RetentionPolicy::default(), "2026-10-24T12:00:00Z")
        .unwrap();
    assert_eq!(batch.items_deleted, 30);
    assert_eq!(
        store
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM thread_items WHERE agent_id = 'a' AND activity = 1",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        1,
        "activity before S is removed and activity after S is retained"
    );
}

#[test]
fn routed_missing_zero_reversed_or_structured_spans_protect_the_whole_target() {
    let (_dir, store) = fixture(200);
    store.in_transaction(|tx| {
        tx.execute("INSERT INTO operations(operation_id,method,entity_id,agent_id,conversation_id,choice_revision,request_hash,posted_sequence,message_start_sequence,status,created_at,updated_at) VALUES ('routed','thread.post','owner','origin','origin',0,'hash',10,5,'uncertain','now','now')", [])?;
        Ok(())
    }).unwrap();
    for payload in [
        serde_json::Value::Null,
        serde_json::json!({"start_sequence":0,"end_sequence":70}),
        serde_json::json!({"start_sequence":61,"end_sequence":0}),
        serde_json::json!({"start_sequence":71,"end_sequence":70}),
        serde_json::json!({"start_sequence":{"value":61},"end_sequence":70}),
        serde_json::json!({"start_sequence":"61","end_sequence":70}),
    ] {
        store
            .in_transaction(|tx| {
                tx.execute(
                    "UPDATE operations SET delivery = ?1",
                    [serde_json::json!({"agent_id":"a","payload":payload}).to_string()],
                )?;
                Ok(())
            })
            .unwrap();
        let report = store
            .retention_report_conversation("a", RetentionPolicy::default(), "2026-10-24T12:00:00Z")
            .unwrap();
        assert!(report.protected);
        assert_eq!(
            report.items_to_prune, 0,
            "unknown routed sequence space must remain conservative"
        );
        assert_eq!(
            store
                .prune_conversation_batch("a", RetentionPolicy::default(), "2026-10-24T12:00:00Z")
                .unwrap()
                .items_deleted,
            0
        );
    }
}

#[test]
fn pending_span_planner_seeks_pending_rows_instead_of_lifetime_history() {
    let (_dir, store) = fixture(150);
    let conn = store.connection();
    let mut statement = conn
        .prepare(&format!("EXPLAIN QUERY PLAN {PENDING_START_SQL}"))
        .unwrap();
    let details = statement
        .query_map(["a"], |row| row.get::<_, String>(3))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    eprintln!("retention pending span query plan: {details:#?}");
    assert!(
        details.iter().any(|detail| detail
            .contains("SEARCH thread_items USING INDEX thread_items_pending_delivery")
            && detail.contains("agent_id=?")),
        "legacy pending lookup must seek only pending rows: {details:#?}"
    );
    assert!(details.iter().any(|detail| detail.contains("SEARCH operations USING INDEX operations_pending_delivery_target") && detail.contains("<expr>=?")),"routed lookup must seek the canonical target, skipping unrelated pending operations: {details:#?}");
}

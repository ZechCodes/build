use super::*;
use crate::store::Store;
use rusqlite::OptionalExtension;
use serde_json::json;

const NOW: i64 = 1_800_000_000_000;
const EPOCH: &str = "synthetic-boot-one";

fn scope() -> Scope {
    Scope::new(Stream::device(), json!({"kind":"device"}))
}

fn append(store: &Store, key: &str, value: i64, streams: &[Stream]) -> Sequence {
    store
        .in_transaction(|tx| {
            let mut batch = PublicationBatch::default();
            batch.upsert(Entity::new("fixture", key), json!({"value":value}), streams);
            Ok(batch
                .publish(tx, NOW)?
                .first()
                .map(|e| e.seq)
                .unwrap_or(Sequence::ZERO))
        })
        .unwrap()
}

fn head(store: &Store, scope: &Scope, cursor: Option<&str>) -> HeadResult {
    let conn = store.connection();
    crate::store::ReadTransaction::new(&conn)
        .sync_head(scope, cursor, EPOCH, NOW)
        .unwrap()
}

fn cursor(store: &Store, scope: &Scope, sequence: Sequence) -> String {
    let conn = store.connection();
    crate::store::ReadTransaction::new(&conn)
        .sync_cursor(scope, sequence)
        .unwrap()
}

#[test]
fn same_transaction_append_commits_and_rolls_back_with_source() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let result: Result<(), crate::store::StoreError> = store.in_transaction(|tx| {
        tx.execute("INSERT INTO meta VALUES('synthetic_source','changed')", [])?;
        let mut batch = PublicationBatch::default();
        batch.upsert(
            Entity::new("fixture", "atomic"),
            json!({"value":1}),
            &[Stream::device()],
        );
        batch.publish(tx, NOW)?;
        Err(crate::store::StoreError::SyncInvalid(
            "synthetic rollback".into(),
        ))
    });
    assert!(result.is_err());
    let conn = store.connection();
    let source: Option<String> = conn
        .query_row(
            "SELECT value FROM meta WHERE key='synthetic_source'",
            [],
            |r| r.get(0),
        )
        .optional()
        .unwrap();
    assert_eq!(source, None);
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM sync_journal", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM sync_revisions", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    drop(conn);
    store
        .writer()
        .sync_transaction(|tx, batch| {
            tx.execute(
                "INSERT INTO meta VALUES('synthetic_source','committed')",
                [],
            )?;
            batch.upsert(
                Entity::new("fixture", "atomic"),
                json!({"value":2}),
                &[Stream::device()],
            );
            Ok(())
        })
        .unwrap();
    assert_eq!(
        store
            .connection()
            .query_row(
                "SELECT value FROM meta WHERE key='synthetic_source'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
        "committed"
    );
    assert!(
        matches!(head(&store, &scope(), None), HeadResult::Ready(h) if h.head == Sequence::new(1).unwrap())
    );
}

#[test]
fn publication_deduplicates_a_transaction_and_unchanged_writes_are_quiet() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    store
        .in_transaction(|tx| {
            let mut batch = PublicationBatch::default();
            for value in 0..1000 {
                batch.upsert(
                    Entity::new("fixture", "one"),
                    json!({"value":value}),
                    &[Stream::device()],
                );
            }
            assert_eq!(batch.publish(tx, NOW)?.len(), 1);
            Ok(())
        })
        .unwrap();
    assert_eq!(
        append(&store, "one", 999, &[Stream::device()]),
        Sequence::ZERO
    );
    assert_eq!(
        store
            .connection()
            .query_row("SELECT COUNT(*) FROM sync_journal", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn reparenting_and_deletion_keep_old_stream_routes_and_tombstone_revision() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    append(&store, "move", 1, &[Stream::project("old")]);
    let moved = append(&store, "move", 2, &[Stream::project("new")]);
    let conn = store.connection();
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sync_routes WHERE seq=?1",
            [moved.get()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 2, "old scope must see the exit");
    drop(conn);
    store
        .in_transaction(|tx| {
            let mut batch = PublicationBatch::default();
            batch.remove(Entity::new("fixture", "move"), &[]);
            let events = batch.publish(tx, NOW)?;
            assert_eq!(events.len(), 1);
            assert_eq!(events[0].operation, Operation::Delete);
            assert!(events[0].seq > moved);
            assert_eq!(batch.publish(tx, NOW)?.len(), 0);
            Ok(())
        })
        .unwrap();
    assert!(store
        .connection()
        .query_row(
            "SELECT deleted FROM sync_revisions WHERE entity_key='move'",
            [],
            |r| r.get::<_, bool>(0)
        )
        .unwrap());
}

#[test]
fn scope_binding_and_authentication_are_required() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let a = Scope::new(Stream::project("p"), json!({"labels":["api"]}));
    let b = Scope::new(Stream::project("p"), json!({"labels":["ui"]}));
    let token = cursor(&store, &a, Sequence::ZERO);
    let conn = store.connection();
    let read = crate::store::ReadTransaction::new(&conn);
    assert!(matches!(
        read.sync_head(&b, Some(&token), EPOCH, NOW),
        Err(SyncError::ScopeMismatch)
    ));
    let mut tampered = token.into_bytes();
    let last = tampered.len() - 2;
    tampered[last] = if tampered[last] == b'A' { b'B' } else { b'A' };
    assert!(matches!(
        read.sync_head(
            &a,
            Some(std::str::from_utf8(&tampered).unwrap()),
            EPOCH,
            NOW
        ),
        Err(SyncError::MalformedCursor)
    ));
}

#[test]
fn malformed_future_and_large_decimal_cursors_are_explicit() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let conn = store.connection();
    let read = crate::store::ReadTransaction::new(&conn);
    for token in ["", "{}", "v9.bad", "00", "invalid"] {
        assert!(matches!(
            read.sync_head(&scope(), Some(token), EPOCH, NOW),
            Err(SyncError::MalformedCursor)
        ));
    }
    let future = read
        .sync_cursor(&scope(), Sequence::new(1).unwrap())
        .unwrap();
    assert!(matches!(
        read.sync_head(&scope(), Some(&future), EPOCH, NOW),
        Err(SyncError::FutureCursor)
    ));
    let large = Sequence::new(9_007_199_254_740_993).unwrap();
    assert_eq!(
        serde_json::to_value(large).unwrap(),
        json!("9007199254740993")
    );
    assert_eq!(
        serde_json::from_value::<Sequence>(json!("9007199254740993")).unwrap(),
        large
    );
    for invalid in [
        json!(1),
        json!("01"),
        json!("-1"),
        json!("9223372036854775808"),
    ] {
        assert!(serde_json::from_value::<Sequence>(invalid).is_err());
    }
}

#[test]
fn logical_retention_floor_is_stream_aware_before_physical_gc() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let token = cursor(&store, &scope(), Sequence::ZERO);
    let other = Scope::new(Stream::project("unaffected"), json!({}));
    let other_token = cursor(&store, &other, Sequence::ZERO);
    append(&store, "lost", 1, &[Stream::device()]);
    store
        .connection()
        .execute("UPDATE sync_state SET floor=1", [])
        .unwrap();
    assert!(matches!(
        head(&store, &scope(), Some(&token)),
        HeadResult::ResyncRequired {
            reason: ResyncReason::RetentionExpired,
            ..
        }
    ));
    assert!(matches!(
        head(&store, &other, Some(&other_token)),
        HeadResult::Ready(_)
    ));
    let boundary = cursor(&store, &scope(), Sequence::new(1).unwrap());
    assert!(matches!(
        head(&store, &scope(), Some(&boundary)),
        HeadResult::Ready(_)
    ));
    assert_eq!(
        store
            .connection()
            .query_row("SELECT COUNT(*) FROM sync_journal", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
    let gc = store.writer().sync_gc_batch(NOW).unwrap();
    assert_eq!(gc.removed_journal, 1);
    assert!(matches!(
        head(&store, &scope(), Some(&token)),
        HeadResult::ResyncRequired {
            reason: ResyncReason::RetentionExpired,
            ..
        }
    ));
}

#[test]
fn retention_edges_for_age_count_and_bytes_are_indexed() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    for value in 0..4 {
        append(&store, &value.to_string(), value, &[Stream::device()]);
    }
    let conn = store.connection();
    let bytes: i64 = conn
        .query_row(
            "SELECT cumulative_bytes FROM sync_journal WHERE seq=2",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let total: i64 = conn
        .query_row("SELECT cumulative_bytes FROM sync_state", [], |r| r.get(0))
        .unwrap();
    let mut policy = retention::Policy {
        max_entries: 4,
        max_bytes: total,
        max_age_ms: 100,
    };
    assert_eq!(
        retention::logical_floor(&conn, NOW + 100, policy).unwrap(),
        0
    );
    policy.max_entries = 3;
    assert_eq!(
        retention::logical_floor(&conn, NOW + 100, policy).unwrap(),
        1
    );
    policy.max_entries = 4;
    policy.max_bytes = total - bytes;
    assert_eq!(retention::logical_floor(&conn, NOW, policy).unwrap(), 2);
    policy.max_bytes -= 1;
    assert_eq!(retention::logical_floor(&conn, NOW, policy).unwrap(), 3);
    policy.max_bytes = total;
    assert_eq!(
        retention::logical_floor(&conn, NOW + 101, policy).unwrap(),
        4
    );
    assert_eq!(retention::MAX_JOURNAL_ENTRIES, 100_000);
    assert_eq!(retention::MAX_JOURNAL_AGE_MS, 7 * 24 * 60 * 60 * 1000);
    assert_eq!(retention::MAX_JOURNAL_BYTES, 64 * 1024 * 1024);
    conn.execute("UPDATE sync_state SET floor=3", []).unwrap();
    drop(conn);
    store.writer().sync_gc_batch(NOW).unwrap();
    policy.max_bytes = total - bytes - 1;
    assert_eq!(
        retention::logical_floor(&store.connection(), NOW, policy).unwrap(),
        3
    );
}

#[test]
fn gc_has_a_physical_row_budget_and_keeps_unexpired_deletions() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    for value in 0..50 {
        append(
            &store,
            &value.to_string(),
            value,
            &[
                Stream::device(),
                Stream::project("p"),
                Stream::task(&value.to_string()),
            ],
        );
    }
    store
        .in_transaction(|tx| {
            let mut batch = PublicationBatch::default();
            batch.remove(Entity::new("fixture", "0"), &[Stream::device()]);
            batch.publish(tx, NOW)?;
            Ok(())
        })
        .unwrap();
    store
        .connection()
        .execute("UPDATE sync_state SET floor=50", [])
        .unwrap();
    let before = store.total_changes();
    let progress = store.writer().sync_gc_batch(NOW).unwrap();
    assert!(progress.pending);
    assert!(store.total_changes() - before <= retention::GC_ROWS as u64);
    assert!(progress.removed_journal > 0);
    assert_eq!(
        store
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM sync_journal WHERE operation='delete'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
}

#[test]
fn ordinary_restart_keeps_cursor_and_changes_only_reply_epoch() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    append(&store, "restart", 1, &[Stream::device()]);
    let token = cursor(&store, &scope(), Sequence::new(1).unwrap());
    let before = head(&store, &scope(), Some(&token));
    drop(store);
    let reopened = Store::new(dir.path()).unwrap();
    let conn = reopened.connection();
    let after = crate::store::ReadTransaction::new(&conn)
        .sync_head(&scope(), Some(&token), "synthetic-boot-two", NOW)
        .unwrap();
    match (before, after) {
        (HeadResult::Ready(a), HeadResult::Ready(b)) => {
            assert_eq!(a.incarnation, b.incarnation);
            assert_eq!(a.head, b.head);
            assert_ne!(a.bridge_epoch, b.bridge_epoch);
        }
        _ => panic!("ordinary restart must allow cursor recovery"),
    }
}

#[test]
fn fresh_store_explicit_restore_and_incompatible_schema_rotate_incarnation() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let token = cursor(&store, &scope(), Sequence::ZERO);
    let foreign_dir = tempfile::tempdir().unwrap();
    let foreign = Store::new(foreign_dir.path()).unwrap();
    assert!(matches!(
        head(&foreign, &scope(), Some(&token)),
        HeadResult::ResyncRequired {
            reason: ResyncReason::IncarnationChanged,
            ..
        }
    ));
    store
        .connection()
        .execute(
            "UPDATE meta SET value='synthetic-old-definition' WHERE key='sync_schema'",
            [],
        )
        .unwrap();
    drop(store);
    let rebuilt = Store::new(dir.path()).unwrap();
    assert!(matches!(
        head(&rebuilt, &scope(), Some(&token)),
        HeadResult::ResyncRequired {
            reason: ResyncReason::IncarnationChanged,
            ..
        }
    ));
    let restored_token = cursor(&rebuilt, &scope(), Sequence::ZERO);
    let backup = dir.path().join("synthetic-backup.db");
    rebuilt.backup_to(&backup).unwrap();
    drop(rebuilt);
    // In-place copying preserves the destination inode: explicit restore is
    // the supported boundary for a restore without atomic replacement.
    std::fs::copy(&backup, dir.path().join("build.db")).unwrap();
    let restored = Store::new_after_restore(dir.path()).unwrap();
    assert!(matches!(
        head(&restored, &scope(), Some(&restored_token)),
        HeadResult::ResyncRequired {
            reason: ResyncReason::IncarnationChanged,
            ..
        }
    ));
}

#[cfg(unix)]
#[test]
fn closed_database_atomic_replacement_is_detected_even_with_copied_meta() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let token = cursor(&store, &scope(), Sequence::ZERO);
    let backup = dir.path().join("synthetic-replacement.db");
    store.backup_to(&backup).unwrap();
    drop(store);
    std::fs::rename(&backup, dir.path().join("build.db")).unwrap();
    let replaced = Store::new(dir.path()).unwrap();
    assert!(matches!(
        head(&replaced, &scope(), Some(&token)),
        HeadResult::ResyncRequired {
            reason: ResyncReason::IncarnationChanged,
            ..
        }
    ));
}

#[test]
fn missing_sync_storage_fails_closed_instead_of_restarting_its_sequence() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    append(&store, "before-loss", 1, &[Stream::device()]);
    store
        .connection()
        .execute_batch("DROP TABLE sync_journal")
        .unwrap();
    drop(store);
    assert!(
        Store::new(dir.path()).is_err(),
        "missing storage needs an intact restore, not silent sequence reuse"
    );
}

#[tokio::test]
async fn sync_head_uses_read_pool_while_writer_is_held() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let pool = store.read_pool();
    let mut reader = pool.acquire_sync().await;
    let _held = store.writer.connection();
    let result = reader.head(&scope(), None, EPOCH, NOW).unwrap();
    assert!(matches!(result,HeadResult::Ready(h) if h.head==Sequence::ZERO));
}

#[test]
fn normalization_preserves_exact_definition_and_opaque_actor_bytes() {
    let a = Scope::new(Stream::project("p"), json!({"a":1,"b":2}));
    let b = Scope::new(Stream::project("p"), json!({"b":2,"a":1}));
    assert_eq!(a.digest(), b.digest());
    let c = Scope::new(
        Stream::project("p"),
        json!({"assignee":"Actual\u{0}suffix"}),
    );
    let d = Scope::new(Stream::project("p"), json!({"assignee":"Actual"}));
    assert_ne!(c.digest(), d.digest());
}

#[test]
fn sync_incarnation_is_separate_and_survives_restart() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let incarnation: Option<String> = store
        .connection()
        .query_row(
            "SELECT value FROM meta WHERE key='sync_incarnation'",
            [],
            |r| r.get(0),
        )
        .optional()
        .unwrap();
    assert!(
        incarnation.is_some(),
        "sync must have its own durable incarnation"
    );
    assert_ne!(incarnation.as_deref(), Some(store.journal_incarnation()));
    drop(store);
    let reopened = Store::new(dir.path()).unwrap();
    let after: String = reopened
        .connection()
        .query_row(
            "SELECT value FROM meta WHERE key='sync_incarnation'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(incarnation.unwrap(), after);
}

#[test]
fn device_334_definition_bump_resets_all_scope_cursors_once() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    use crate::app::TaskSyncScope;
    let scopes = [
        TaskSyncScope::device(),
        TaskSyncScope::task("task-synthetic", "project-synthetic", "/synthetic"),
        TaskSyncScope::project("project-synthetic", "/synthetic", &json!({})).unwrap(),
    ];
    let tokens: Vec<_> = scopes
        .iter()
        .map(|scope| cursor(&store, scope.scope(), Sequence::ZERO))
        .collect();
    store
        .connection()
        .execute(
            "UPDATE meta SET value='sync-journal-2' WHERE key='sync_schema'",
            [],
        )
        .unwrap();
    drop(store);
    let upgraded = Store::new(dir.path()).unwrap();
    for (scope, token) in scopes.iter().zip(&tokens) {
        assert!(
            matches!(
                head(&upgraded, scope.scope(), Some(token)),
                HeadResult::ResyncRequired {
                    reason: ResyncReason::IncarnationChanged,
                    ..
                }
            ),
            "definition3 must reject the previous incarnation"
        );
    }
    let tokens: Vec<_> = scopes
        .iter()
        .map(|scope| cursor(&upgraded, scope.scope(), Sequence::ZERO))
        .collect();
    drop(upgraded);
    let reopened = Store::new(dir.path()).unwrap();
    for (scope, token) in scopes.iter().zip(&tokens) {
        assert!(
            matches!(
                head(&reopened, scope.scope(), Some(token)),
                HeadResult::Ready(_)
            ),
            "normal current-definition reopen must retain cursor validity"
        );
    }
}

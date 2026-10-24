//! Regression and opt-in measurements for off-lock storage maintenance.
use super::*;
use std::time::{Duration, Instant};

#[tokio::test]
async fn maintenance_batches_preserve_independent_read_p99() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    {
        let mut connection = store.connection();
        let tx = connection.transaction().unwrap();
        for index in 0..4_000 {
            tx.execute(
                "INSERT INTO operations (operation_id, method, entity_id, agent_id,
                 conversation_id, choice_revision, request_hash, posted_sequence,
                 message_start_sequence, status, delivery, created_at, updated_at)
                 VALUES (?1, 'thread.post', 'owner', 'agent', 'conversation', 0,
                 'hash', 1, 1, 'delivered', ?2, '', '')",
                rusqlite::params![format!("trim-{index}"), "x".repeat(16_384)],
            )
            .unwrap();
        }
        tx.commit().unwrap();
    }
    // Read a metadata-only sentinel while other, large receipt rows are freed.
    store
        .connection()
        .execute(
            "UPDATE operations SET delivery = NULL WHERE operation_id = 'trim-0'",
            [],
        )
        .unwrap();
    let mut reader = store.read_pool().acquire().await;
    let mut baseline = Vec::new();
    for _ in 0..1_000 {
        baseline.push(measure_read(&mut reader));
    }
    let writer = store.writer();
    let worker = std::thread::spawn(move || {
        let mut cleared = 0;
        loop {
            let progress = writer.maintain_storage_batch().unwrap();
            assert!(progress.scanned_operations <= 1_000);
            assert!(progress.reclaimed_pages <= 32);
            cleared += progress.cleared_deliveries;
            if progress.cleanup_complete && progress.remaining_free_pages == 0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        cleared
    });
    let mut during = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(30);
    while !worker.is_finished() || during.len() < 1_000 {
        assert!(
            Instant::now() < deadline,
            "maintenance must finish promptly"
        );
        during.push(measure_read(&mut reader));
        // Avoid a hot reader hiding realistic writer scheduling on loaded hosts.
        std::thread::sleep(Duration::from_micros(100));
    }
    assert_eq!(worker.join().unwrap(), 3_999);
    let baseline_p99 = p99(&mut baseline);
    let maintenance_p99 = p99(&mut during);
    eprintln!(
        "storage read p99: baseline={baseline_p99:?}, maintenance={maintenance_p99:?}, samples={}",
        during.len()
    );
    assert!(maintenance_p99 < Duration::from_millis(100));
}

fn measure_read(reader: &mut ReadConnection) -> Duration {
    let started = Instant::now();
    reader
        .read(|tx| {
            let receipt = tx.operation("trim-0").unwrap().unwrap();
            assert_eq!(receipt.request_hash, "hash");
            assert_eq!(receipt.status, OperationStatus::Delivered);
        })
        .unwrap();
    started.elapsed()
}

fn p99(samples: &mut [Duration]) -> Duration {
    samples.sort_unstable();
    samples[(samples.len() * 99 / 100).min(samples.len() - 1)]
}

/// Run with BUILD_DB_TRIM_FIXTURE pointing at an offline database copy.
/// The provided fixture is copied again before opening; it is never mutated.
#[test]
#[ignore = "requires explicit offline DB fixture; measures physical reclamation"]
fn maintenance_reclaims_offline_database_copy() {
    let source = std::env::var_os("BUILD_DB_TRIM_FIXTURE").expect("set BUILD_DB_TRIM_FIXTURE");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DB_FILE);
    std::fs::copy(source, &path).unwrap();
    let before = std::fs::metadata(&path).unwrap().len();
    let store = Store::new(dir.path()).unwrap();
    let retained_before = unsettled_payloads(&store);
    let items_before: i64 = store
        .connection()
        .query_row("SELECT count(*) FROM thread_items", [], |r| r.get(0))
        .unwrap();
    let started = Instant::now();
    let mut cleared = 0;
    let mut batches = 0;
    loop {
        let progress = store.writer().maintain_storage_batch().unwrap();
        cleared += progress.cleared_deliveries;
        batches += 1;
        if progress.cleanup_complete && progress.remaining_free_pages == 0 {
            break;
        }
    }
    assert_eq!(unsettled_payloads(&store), retained_before);
    let connection = store.connection();
    let items_after: i64 = connection
        .query_row("SELECT count(*) FROM thread_items", [], |r| r.get(0))
        .unwrap();
    assert_eq!(items_before, items_after);
    let remaining: i64 = connection
        .query_row(
            "SELECT count(*) FROM operations WHERE status = 'delivered' AND delivery IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(remaining, 0);
    let integrity: String = connection
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .unwrap();
    assert_eq!(integrity, "ok");
    drop(connection);
    drop(store);
    let after = std::fs::metadata(&path).unwrap().len();
    eprintln!("offline copy: before={before} after={after} saved={} cleared={cleared} batches={batches} elapsed={:?}; integrity=ok; thread_items={items_after}; unsettled payloads unchanged", before.saturating_sub(after), started.elapsed());
    assert!(after < before);
}

fn unsettled_payloads(store: &Store) -> Vec<(String, Option<String>)> {
    let connection = store.connection();
    let mut statement = connection.prepare("SELECT operation_id, delivery FROM operations WHERE status != 'delivered' ORDER BY operation_id").unwrap();
    statement
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}
